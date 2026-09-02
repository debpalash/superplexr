use std::io;

use termi9ne_plugin::{
    HostMessage, PluginActivityState, PluginConnection, PluginEvent, PluginMessage,
    PluginStatusLevel,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("agent status plugin failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut connection = PluginConnection::new(stdin.lock(), stdout.lock());
    let hello = connection.receive()?;
    let HostMessage::Hello { plugin_id, .. } = hello.message else {
        return Err("host did not begin with a plugin handshake".into());
    };
    connection.send(PluginMessage::Ready {
        plugin_id: plugin_id.clone(),
    })?;

    loop {
        match connection.receive()?.message {
            HostMessage::Event {
                event: PluginEvent::RunActivityChanged { run_id, state },
            } => {
                let (level, text) = activity_status(state);
                connection.send(PluginMessage::Status {
                    key: format!("run:{run_id}"),
                    level,
                    text: text.to_owned(),
                })?;
            }
            HostMessage::Event {
                event:
                    PluginEvent::TerminalChanged {
                        session_id,
                        state,
                        archived,
                    },
            } => {
                connection.send(PluginMessage::Status {
                    key: format!("session:{session_id}"),
                    level: PluginStatusLevel::Info,
                    text: format!("terminal {state:?}; archived={archived}"),
                })?;
            }
            HostMessage::Shutdown => return Ok(()),
            HostMessage::Hello { .. } => return Err("host repeated the plugin handshake".into()),
        }
    }
}

fn activity_status(state: PluginActivityState) -> (PluginStatusLevel, &'static str) {
    match state {
        PluginActivityState::Working => (PluginStatusLevel::Success, "agent working"),
        PluginActivityState::WaitingInput => (PluginStatusLevel::Warning, "agent needs input"),
        PluginActivityState::WaitingApproval => {
            (PluginStatusLevel::Warning, "agent needs approval")
        }
        PluginActivityState::Blocked | PluginActivityState::Failed => {
            (PluginStatusLevel::Error, "agent blocked")
        }
        PluginActivityState::Completed => (PluginStatusLevel::Success, "agent completed"),
        PluginActivityState::Cancelled => (PluginStatusLevel::Warning, "agent cancelled"),
        PluginActivityState::Pending
        | PluginActivityState::Idle
        | PluginActivityState::Paused
        | PluginActivityState::Unknown => (PluginStatusLevel::Info, "agent idle"),
    }
}
