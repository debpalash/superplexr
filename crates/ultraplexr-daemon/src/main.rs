//! The runtime alone. The desktop starts this same runtime for itself; a
//! host with no screen — a VPS, a container, a machine under the desk —
//! starts it here, with the gateway on, and every shell reaches it the
//! way it reaches a laptop: pair once, then TUI, web or phone.
//!
//! Nothing in the engine knows the difference. That is the point.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "ultraplexr-daemon",
    about = "Run the ultraplexr runtime without a desktop, for a host that never sleeps"
)]
struct Args {
    /// Where local clients connect. Keep it short: Unix socket paths are
    /// limited to about a hundred characters.
    #[arg(long, default_value = "/tmp/ultraplexr/control.sock")]
    socket: PathBuf,
    /// Journals, Faults, devices, shares, push subscriptions: everything the
    /// runtime keeps. Owner-only.
    #[arg(long, default_value = "/var/lib/ultraplexr")]
    state_dir: PathBuf,
    /// Listen for paired devices and viewer links here, e.g. `0.0.0.0:7373`.
    /// Off when absent: the daemon then serves the socket alone.
    #[arg(long)]
    gateway: Option<String>,
}

fn main() {
    let args = Args::parse();
    if let Some(parent) = args.socket.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("could not create {}: {error}", parent.display());
        std::process::exit(2);
    }
    if let Err(error) = std::fs::create_dir_all(&args.state_dir) {
        eprintln!("could not create {}: {error}", args.state_dir.display());
        std::process::exit(2);
    }
    if let Err(error) = ultraplexr_server::run_blocking(args.socket, args.state_dir, args.gateway) {
        eprintln!("ultraplexr-daemon: {error}");
        std::process::exit(1);
    }
}
