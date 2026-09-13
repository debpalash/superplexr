use clap::Parser;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};
use superplexr_client::ControlClient;
use superplexr_observer::{EmbeddingPolicy, Observer};

#[derive(Parser)]
#[command(about = "Local scoped browser client; read-only unless --allow-control is explicit")]
struct Args {
    #[arg(long, default_value_os_t = superplexr_protocol::default_socket_path())]
    socket: PathBuf,
    #[arg(long)]
    share_token_file: PathBuf,
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Enable terminal Control with a Controller Share; never grants owner access.
    #[arg(long)]
    allow_control: bool,
    /// Enable read-only verification inspection within the Share's Mission scope.
    #[arg(long)]
    workflow_read: bool,
    /// Trusted parent origin allowed to frame this UI; repeat for each origin.
    /// HTTPS or HTTP loopback only. Does not grant API access or enable CORS.
    #[arg(long)]
    embed_origin: Vec<String>,
    /// Acknowledge clickjacking risk when trusted parents frame a Controller UI.
    #[arg(long, requires_all = ["allow_control", "embed_origin"])]
    allow_embedded_control: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let embedding = if args.embed_origin.is_empty() {
        None
    } else {
        if args.allow_control && !args.allow_embedded_control {
            return Err(
                "--allow-control with --embed-origin also requires --allow-embedded-control".into(),
            );
        }
        Some(EmbeddingPolicy::new(args.embed_origin)?)
    };
    let client = tokio::task::spawn_blocking(move || {
        let token = superplexr_client::read_share_token_file(&args.share_token_file)?;
        ControlClient::connect_as_observer(args.socket, token.trim())
    })
    .await??;
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, args.port))).await?;
    let address = listener.local_addr()?;
    let key = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let observer = if args.allow_control {
        Observer::with_control(client, address.to_string(), key.clone())?
    } else {
        Observer::new(client, address.to_string(), key.clone())?
    };
    let observer = if args.workflow_read {
        observer.with_workflow_inspection()
    } else {
        observer
    };
    let observer = if let Some(policy) = embedding {
        observer.with_embedding(policy, args.allow_embedded_control)?
    } else {
        observer
    };
    let mode = if args.allow_control {
        "Controller-capable browser"
    } else {
        "Read-only observer"
    };
    println!("{mode}: http://{address}/#access={key}");
    println!("Keep this URL private. Ctrl-C stops the observer, not the runtime.");
    if args.allow_embedded_control {
        eprintln!(
            "Embedded Control enabled: every allowed parent must be trusted with terminal input."
        );
    }
    axum::serve(listener, observer.clone().router())
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            observer.shutdown();
        })
        .await?;
    Ok(())
}
