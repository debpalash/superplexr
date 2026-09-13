mod app;
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use superplexr_client::ControlClient;
use superplexr_core::SessionId;
use superplexr_tui::{
    render,
    workspace::{Open, Workspace},
};

#[derive(Parser)]
#[command(about = "Terminal workspace client; no Session ID opens navigation. Ctrl-] d detaches.")]
struct Args {
    /// Existing Session ID, or omit to browse Missions, Sessions and Attention.
    session: Option<SessionId>,
    #[arg(long,default_value_os_t=superplexr_protocol::default_socket_path())]
    socket: PathBuf,
    #[arg(long, conflicts_with = "session")]
    list: bool,
    #[arg(long)]
    share_token_file: Option<PathBuf>,
    /// Explicit non-forced Control on the initial Session only.
    #[arg(long, requires = "session", conflicts_with = "list")]
    control: bool,
    #[arg(long)]
    osc52: bool,
    /// Enable read-only Mission verifier discovery (w) and UUID inspection (W).
    #[arg(long)]
    workflow_read: bool,
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            EnableBracketedPaste,
            Hide
        )?;
        Ok(guard)
    }
}
fn restore_terminal() {
    let _ = execute!(
        io::stdout(),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Show,
        DisableBracketedPaste,
        EnableLineWrap,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn client(args: &Args) -> Result<ControlClient, Box<dyn std::error::Error>> {
    if let Some(path) = &args.share_token_file {
        let token = superplexr_client::read_share_token_file(path)?;
        Ok(ControlClient::connect_with_share(&args.socket, token)?)
    } else {
        Ok(ControlClient::connect(&args.socket)?)
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let client = client(&args)?;
    if args.list {
        for session in client.list_terminals()? {
            let name = session
                .foreground_process
                .as_ref()
                .map(|p| p.executable.as_str())
                .unwrap_or("terminal");
            println!(
                "{}  {:?}  {}",
                session.session_id,
                session.status,
                render::safe_text(name)
            );
        }
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("TUI requires a terminal on stdin and stdout; use --list for scripts".into());
    }
    let mut workspace = Workspace::default();
    if let Some(id) = args.session {
        workspace.open(&client, id, Open::Replace)?;
        workspace.panes[0].initial_claim = args.control;
    }
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, stop.clone())?;
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));
    let guard = TerminalGuard::enter()?;
    let result = app::run(
        &client,
        &mut workspace,
        args.osc52,
        args.workflow_read,
        stop,
    );
    drop(guard);
    if let Err(error) = workspace.release_active() {
        eprintln!(
            "Control release not confirmed: {}",
            render::safe_text(&error.to_string())
        );
    }
    result?;
    println!("Detached; host Sessions were not stopped.");
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", render::safe_text(&error.to_string()));
            std::process::ExitCode::FAILURE
        }
    }
}
