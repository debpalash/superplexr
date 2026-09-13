use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use superplexr_client::{ControlClient, DaemonSession};
use superplexr_core::SessionId;
use superplexr_protocol::{ShareRole, TerminalSessionSpec, TerminalSessionStatus};
use superplexr_terminal::GridSize;
use superplexr_tui::{FeedStatus, FocusedSession};

#[path = "session_tests/search_tests.rs"]
mod search_tests;

#[path = "session_tests/workflow_tests.rs"]
mod workflow_tests;

#[test]
fn oversized_metadata_ends_tui_feed_without_retrying_or_stopping_pty() {
    let fixture = Fixture::new();
    let source = ControlClient::connect(fixture.root.join("s")).unwrap();
    let mut focused = FocusedSession::attach(&source, fixture.terminal.id()).unwrap();
    let metadata = source.subscribe_missions().unwrap();
    let pid = fixture.terminal.capture().unwrap().terminal.process_id;
    fixture
        .owner
        .create_mission(
            superplexr_core::MissionId::new(),
            "m".repeat(9 * 1024 * 1024),
            superplexr_core::Actor::human("oversized-tui").unwrap(),
        )
        .unwrap();
    assert!(matches!(
        metadata.recv(),
        Err(superplexr_client::ClientError::ReceiveLimit { .. })
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(view) = focused.take_update().unwrap()
            && view.status == FeedStatus::ReceiveLimited
        {
            assert!(view.frame.is_none());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!focused.controlling());
    drop(focused);
    assert_eq!(fixture.terminal.capture().unwrap().terminal.process_id, pid);
}

#[test]
#[ignore = "isolated daemon child entry point"]
fn isolated_daemon() {
    if let Some(root) = std::env::var_os("SUPERPLEXR_TUI_TEST_ROOT") {
        let root = PathBuf::from(root);
        superplexr_server::run_blocking(root.join("s"), root.join("state"), None).expect("runtime");
    }
}

struct Fixture {
    root: PathBuf,
    child: Child,
    owner: ControlClient,
    terminal: DaemonSession,
}
impl Fixture {
    fn new() -> Self {
        Self::with_program("/bin/cat", &[])
    }
    fn with_program(program: &str, args: &[&str]) -> Self {
        let root = PathBuf::from("/tmp").join(format!("up-tui-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("isolated root");
        let log = std::fs::File::create(root.join("daemon.log")).expect("log");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "isolated_daemon", "--ignored", "--nocapture"])
            .env("SUPERPLEXR_TUI_TEST_ROOT", &root)
            .stdout(log.try_clone().expect("log"))
            .stderr(log)
            .spawn()
            .expect("daemon");
        let deadline = Instant::now() + Duration::from_secs(15);
        let owner = loop {
            if let Ok(client) = ControlClient::connect(root.join("s")) {
                break client;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("runtime startup timeout");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let terminal = owner
            .start_terminal(TerminalSessionSpec {
                session_id: SessionId::new(),
                mission_id: None,
                run_id: None,
                program: program.into(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                cwd: root.clone(),
                environment_delta: BTreeMap::new(),
                grid: GridSize {
                    columns: 80,
                    rows: 24,
                },
            })
            .expect("PTY");
        Self {
            root,
            child,
            owner,
            terminal,
        }
    }
    fn text(&self) -> String {
        self.terminal
            .snapshot()
            .expect("snapshot")
            .rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.terminal.claim_control(true);
        let _ = self.terminal.kill();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[track_caller]
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition deadline");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn live(view: &mut FocusedSession) {
    until(|| {
        view.take_update()
            .expect("feed")
            .is_some_and(|view| view.status == FeedStatus::Live)
    });
}

#[test]
fn shared_runtime_enforces_observation_control_handoff_and_detach_identity() {
    let fixture = Fixture::new();
    let id = fixture.terminal.id();
    let before = fixture.owner.list_terminals().expect("list")[0].clone();
    let (share, token) = fixture
        .owner
        .create_share("tui observer", ShareRole::Observer, vec![], vec![id], 60)
        .expect("share");
    let observer =
        ControlClient::connect_with_share(fixture.root.join("s"), token).expect("scoped client");
    let mut readonly = FocusedSession::attach(&observer, id).expect("observe");
    live(&mut readonly);
    assert!(readonly.claim_control().is_err());
    assert!(readonly.paste("UNAUTHORIZED".into(), true).is_err());
    assert!(
        readonly
            .resize(GridSize {
                columns: 90,
                rows: 30
            })
            .is_err()
    );
    assert_eq!(
        fixture.terminal.snapshot().expect("frame").grid,
        GridSize {
            columns: 80,
            rows: 24
        }
    );
    fixture
        .terminal
        .release_control()
        .expect("return initial lease");
    let mut view = FocusedSession::attach(&fixture.owner, id).expect("attach");
    live(&mut view);
    view.claim_control().expect("claim");
    view.resize(GridSize {
        columns: 90,
        rows: 25,
    })
    .expect("resize");
    view.paste("TUI-CONTROL-MARKER\n".into(), true)
        .expect("input");
    until(|| fixture.text().contains("TUI-CONTROL-MARKER"));
    fixture.terminal.claim_control(true).expect("owner handoff");
    assert!(view.paste("STALE-LEASE".into(), true).is_err());
    assert!(!view.controlling());
    drop(view);
    assert_eq!(
        fixture.owner.list_terminals().expect("list")[0].controller_surface_id,
        Some(fixture.terminal.surface_id()),
        "detach cannot release another surface's lease"
    );
    let mut resumed = FocusedSession::attach(&fixture.owner, id).expect("reattach");
    live(&mut resumed);
    let after = fixture.owner.list_terminals().expect("list");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].session_id, before.session_id);
    assert_eq!(after[0].process_id, before.process_id);
    assert!(
        resumed
            .history(0)
            .expect("history")
            .rows
            .iter()
            .any(|row| row.text().contains("TUI-CONTROL-MARKER"))
    );
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    until(|| {
        readonly
            .take_update()
            .expect("feed")
            .is_some_and(|view| view.status == FeedStatus::Rejected && view.frame.is_none())
    });
    assert_eq!(after[0].status, TerminalSessionStatus::Running);
}

struct OuterTerminal {
    pair: portable_pty::PtyPair,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl OuterTerminal {
    fn start(fixture: &Fixture, extra: &[&str]) -> Self {
        Self::start_at(fixture, Some(fixture.terminal.id()), extra)
    }
    fn start_at(fixture: &Fixture, id: Option<SessionId>, extra: &[&str]) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 25,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("outer PTY");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_superplexr-tui"));
        command.args(["--socket", fixture.root.join("s").to_str().expect("socket")]);
        if let Some(id) = id {
            command.arg(id.to_string());
        }
        command.args(extra);
        command.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(command).expect("TUI process");
        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let output = bytes.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut output = output.lock().expect("output");
                if output.len() + n > 2 * 1024 * 1024 {
                    break;
                }
                output.extend_from_slice(&buffer[..n]);
            }
        });
        Self {
            pair,
            child,
            writer,
            bytes,
        }
    }
    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("keys");
        self.writer.flush().expect("flush");
    }
    fn saw(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.bytes.lock().expect("output")).contains(text)
    }
    fn wait_for(&self, text: &str) {
        self.wait_for_since(0, text);
    }
    fn checkpoint(&self) -> usize {
        self.bytes.lock().expect("output").len()
    }
    fn wait_for_since(&self, offset: usize, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !String::from_utf8_lossy(&self.bytes.lock().expect("output")[offset..]).contains(text)
        {
            if Instant::now() >= deadline {
                let bytes = self.bytes.lock().expect("output");
                let output = String::from_utf8_lossy(&bytes).into_owned();
                drop(bytes);
                let statuses = output
                    .split("\x1b[7m")
                    .filter_map(|part| part.split_once("\x1b").map(|(text, _)| text))
                    .collect::<Vec<_>>();
                let recent = &statuses[statuses.len().saturating_sub(12)..];
                panic!("TUI did not display {text:?}; recent highlighted rows: {recent:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn wait_exit(&mut self) -> portable_pty::ExitStatus {
        let mut result = None;
        until(|| {
            result = self.child.try_wait().expect("status");
            result.is_some()
        });
        result.expect("exit")
    }
}

fn second_terminal(fixture: &Fixture) -> DaemonSession {
    fixture
        .owner
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: fixture.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize {
                columns: 80,
                rows: 24,
            },
        })
        .expect("second PTY")
}
fn name_group(fixture: &Fixture, id: SessionId, name: &str) {
    fixture
        .owner
        .create_session_group(superplexr_protocol::SessionGroupSpec {
            group_id: superplexr_protocol::SessionGroupId::new(),
            mission_id: None,
            name: name.into(),
            session_ids: vec![id],
            position: 0,
            pinned: false,
            detached: false,
        })
        .expect("group name");
}
fn attention_mission(fixture: &Fixture) -> (superplexr_core::MissionId, superplexr_core::SignalId) {
    use superplexr_core::{
        Actor, Command as DomainCommand, MissionId, RunId, SignalId, SignalKind,
    };
    let mission = MissionId::new();
    let run = RunId::new();
    let signal = SignalId::new();
    let human = Actor::human("tui-test").expect("actor");
    fixture
        .owner
        .create_mission(mission, "Project Alpha", human.clone())
        .expect("mission");
    for command in [
        DomainCommand::StartSession {
            session_id: fixture.terminal.id(),
            name: "Alpha".into(),
            started_by: human.clone(),
        },
        DomainCommand::StartRun {
            run_id: run,
            parent: None,
            actor: human,
            objective: "Test task".into(),
        },
        DomainCommand::AssignSession {
            session_id: fixture.terminal.id(),
            run_id: run,
        },
        DomainCommand::RaiseSignal {
            signal_id: signal,
            run_id: run,
            kind: SignalKind::InputNeeded {
                question: "Review Alpha output".into(),
            },
        },
    ] {
        fixture
            .owner
            .dispatch(mission, command)
            .expect("domain command");
    }
    (mission, signal)
}

#[test]
fn navigator_uses_scoped_runtime_names_and_unresolved_attention_without_writes() {
    use superplexr_tui::navigator::{Navigator, Tab, Target};
    let fixture = Fixture::new();
    let other = second_terminal(&fixture);
    name_group(&fixture, fixture.terminal.id(), "Alpha named session");
    name_group(&fixture, other.id(), "PRIVATE-BETA");
    let (mission, signal) = attention_mission(&fixture);
    use superplexr_core::{Actor, Command as DomainCommand, Risk, RunId, SignalId, SignalKind};
    let unbound = RunId::new();
    fixture
        .owner
        .dispatch(
            mission,
            DomainCommand::StartRun {
                run_id: unbound,
                parent: None,
                actor: Actor::human("test").expect("actor"),
                objective: "No terminal".into(),
            },
        )
        .expect("unbound run");
    fixture
        .owner
        .dispatch(
            mission,
            DomainCommand::RaiseSignal {
                run_id: unbound,
                signal_id: SignalId::new(),
                kind: SignalKind::ApprovalNeeded {
                    operation: "Do not approve automatically".into(),
                    risk: Risk::High,
                },
            },
        )
        .expect("high risk");
    let before = fixture.owner.get_mission(mission).expect("mission").version;
    let mut nav = Navigator::default();
    nav.refresh(&fixture.owner).expect("sessions");
    assert_eq!(nav.entries.len(), 2);
    assert!(
        nav.entries
            .iter()
            .any(|entry| entry.label == "Alpha named session")
    );
    nav.query = "Alpha".into();
    assert_eq!(nav.visible().len(), 1);
    assert_eq!(
        nav.current().expect("selection").target,
        Target::Session(fixture.terminal.id())
    );
    nav.query.clear();
    nav.tab = Tab::Attention;
    nav.refresh(&fixture.owner).expect("Attention");
    assert_eq!(nav.entries.len(), 2);
    assert_eq!(
        nav.entries[0].target,
        Target::Unavailable,
        "high-risk items without a primary Session must not open an arbitrary shell"
    );
    assert_eq!(nav.entries[1].key, signal.to_string());
    assert_eq!(
        nav.entries[1].target,
        Target::Session(fixture.terminal.id())
    );
    assert_eq!(
        fixture
            .owner
            .get_mission(mission)
            .expect("unchanged")
            .version,
        before
    );
    let (_, token) = fixture
        .owner
        .create_share(
            "session only",
            ShareRole::Observer,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("share");
    let scoped = ControlClient::connect_with_share(fixture.root.join("s"), token).expect("scoped");
    nav.tab = Tab::Sessions;
    nav.refresh(&scoped).expect("scoped list");
    assert_eq!(nav.entries.len(), 1);
    assert!(
        !nav.entries[0].label.contains("named session"),
        "owner-only group name must not leak"
    );
    nav.tab = Tab::Attention;
    nav.refresh(&scoped).expect("scoped Attention");
    assert!(nav.entries.is_empty());
    let (share, token) = fixture
        .owner
        .create_share(
            "mission observer",
            ShareRole::Observer,
            vec![mission],
            vec![],
            60,
        )
        .expect("mission share");
    let scoped =
        ControlClient::connect_with_share(fixture.root.join("s"), token).expect("mission observer");
    nav.refresh(&scoped).expect("visible Attention");
    assert_eq!(nav.entries.len(), 2);
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    assert!(nav.refresh(&scoped).is_err());
    assert!(nav.entries.is_empty());
    other.claim_control(false).expect("cleanup lease");
    other.kill().expect("cleanup");
}

#[test]
fn workspace_switch_and_split_do_not_duplicate_processes_or_retain_background_control() {
    use superplexr_tui::workspace::{Axis, Open, Workspace};
    let fixture = Fixture::new();
    let other = second_terminal(&fixture);
    fixture.terminal.release_control().expect("release");
    other.release_control().expect("release");
    let before = fixture.owner.list_terminals().expect("list");
    let mut workspace = Workspace::default();
    workspace
        .open(&fixture.owner, fixture.terminal.id(), Open::Replace)
        .expect("open");
    live(&mut workspace.panes[0].session);
    workspace.panes[0].session.claim_control().expect("claim");
    workspace
        .open(&fixture.owner, other.id(), Open::Split(Axis::Vertical))
        .expect("split");
    assert_eq!(workspace.panes.len(), 2);
    assert_eq!(workspace.active, 1);
    assert!(!workspace.panes[0].session.controlling());
    assert!(!workspace.panes[1].session.controlling());
    assert!(
        fixture
            .owner
            .list_terminals()
            .expect("list")
            .iter()
            .all(|terminal| terminal.controller_surface_id.is_none())
    );
    live(&mut workspace.panes[1].session);
    workspace.panes[1]
        .session
        .claim_control()
        .expect("second control");
    workspace.next_pane().expect("focus");
    assert_eq!(workspace.active, 0);
    assert!(!workspace.panes[1].session.controlling());
    workspace
        .open(
            &fixture.owner,
            fixture.terminal.id(),
            Open::Split(Axis::Horizontal),
        )
        .expect("existing focuses without duplication");
    assert_eq!(workspace.panes.len(), 2);
    assert!(
        workspace
            .open(
                &fixture.owner,
                SessionId::new(),
                Open::Split(Axis::Horizontal)
            )
            .is_err()
    );
    workspace.close_active().expect("close view");
    assert_eq!(workspace.panes.len(), 1);
    assert_eq!(workspace.panes[0].id, other.id());
    drop(workspace);
    let after = fixture.owner.list_terminals().expect("list");
    assert_eq!(after.len(), 2);
    for terminal in before {
        assert_eq!(
            after
                .iter()
                .find(|item| item.session_id == terminal.session_id)
                .expect("retained")
                .process_id,
            terminal.process_id
        );
    }
    other.claim_control(false).expect("cleanup lease");
    other.kill().expect("cleanup");
}

#[test]
fn real_tui_navigates_missions_attention_and_two_panes_with_safe_focus() {
    let fixture = Fixture::new();
    let other = second_terminal(&fixture);
    name_group(&fixture, fixture.terminal.id(), "Alpha");
    name_group(&fixture, other.id(), "Beta");
    let (mission, signal) = attention_mission(&fixture);
    fixture.terminal.release_control().expect("release");
    other.release_control().expect("release");
    let before = fixture.owner.list_terminals().expect("before");
    let mut tui = OuterTerminal::start_at(&fixture, None, &[]);
    until(|| tui.saw("Superplexr / Sessions"));
    let missions = tui.checkpoint();
    tui.send(b"\t");
    until(|| tui.saw("Superplexr / Missions"));
    tui.wait_for_since(missions, "> Project Alpha");
    let sessions = tui.checkpoint();
    tui.send(b"\r");
    tui.wait_for("Mission filtered");
    tui.wait_for_since(sessions, "> Alpha");
    tui.send(b"\r");
    tui.wait_for("Observing");
    tui.send(b"\x1dc");
    until(|| {
        fixture
            .owner
            .list_terminals()
            .expect("list")
            .iter()
            .find(|item| item.session_id == fixture.terminal.id())
            .expect("first")
            .controller_surface_id
            .is_some()
    });
    let attention = tui.checkpoint();
    tui.send(b"\x1da");
    until(|| tui.saw("Superplexr / Attention"));
    tui.wait_for_since(attention, "Review Alpha output");
    assert!(
        fixture
            .owner
            .list_terminals()
            .expect("list")
            .iter()
            .all(|item| item.controller_surface_id.is_none())
    );
    let observing = tui.checkpoint();
    tui.send(b"\r");
    tui.wait_for_since(observing, "OBSERVE |");
    assert!(
        fixture.owner.get_mission(mission).expect("signal").signals[&signal]
            .resolution
            .is_none()
    );
    let split = tui.checkpoint();
    tui.send(b"\x1dv");
    until(|| tui.saw("choose second pane"));
    tui.wait_for_since(split, "> Alpha");
    tui.send(b"/Beta\r\r");
    tui.wait_for(&format!("> {} OBSERVE", &other.id().to_string()[..8]));
    tui.send(b"\x1dc");
    until(|| {
        other.snapshot().expect("grid").grid
            == GridSize {
                columns: 40,
                rows: 24,
            }
    });
    tui.send(b"ONLY-BETA\r");
    until(|| {
        other
            .snapshot()
            .expect("frame")
            .rows
            .iter()
            .any(|row| row.text().contains("ONLY-BETA"))
    });
    assert!(!fixture.text().contains("ONLY-BETA"));
    let focused = tui.checkpoint();
    tui.send(b"\x1do");
    until(|| {
        fixture
            .owner
            .list_terminals()
            .expect("list")
            .iter()
            .all(|item| item.controller_surface_id.is_none())
    });
    tui.wait_for_since(
        focused,
        &format!("> {} OBSERVE", &fixture.terminal.id().to_string()[..8]),
    );
    tui.send(b"\x1dc");
    until(|| {
        fixture.terminal.snapshot().expect("grid").grid
            == GridSize {
                columns: 39,
                rows: 24,
            }
    });
    tui.send(b"ONLY-ALPHA\r");
    until(|| fixture.text().contains("ONLY-ALPHA"));
    assert!(
        !other
            .snapshot()
            .expect("frame")
            .rows
            .iter()
            .any(|row| row.text().contains("ONLY-ALPHA"))
    );
    let closed = tui.checkpoint();
    tui.send(b"\x1dx");
    until(|| {
        fixture
            .owner
            .list_terminals()
            .expect("list")
            .iter()
            .all(|item| item.controller_surface_id.is_none())
    });
    tui.wait_for_since(closed, "\x1b[7mOBSERVE |");
    let stack = tui.checkpoint();
    tui.send(b"\x1ds");
    tui.wait_for_since(stack, "> Alpha");
    let attached = tui.checkpoint();
    tui.send(b"/Alpha\r\r");
    tui.wait_for_since(
        attached,
        &format!("> {} OBSERVE", &fixture.terminal.id().to_string()[..8]),
    );
    tui.send(b"\x1dc");
    until(|| {
        fixture.terminal.snapshot().expect("stack grid").grid
            == GridSize {
                columns: 80,
                rows: 11,
            }
    });
    tui.pair
        .master
        .resize(PtySize {
            cols: 80,
            rows: 5,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("compact outer terminal");
    until(|| {
        fixture
            .terminal
            .snapshot()
            .expect("compact focused grid")
            .grid
            == GridSize {
                columns: 80,
                rows: 4,
            }
    });
    assert_eq!(
        other.snapshot().expect("background grid").grid,
        GridSize {
            columns: 40,
            rows: 24
        }
    );
    tui.pair
        .master
        .resize(PtySize {
            cols: 80,
            rows: 25,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("restore outer size");
    until(|| {
        fixture.terminal.snapshot().expect("stack returns").grid
            == GridSize {
                columns: 80,
                rows: 11,
            }
    });
    tui.send(b"\x1dd");
    assert!(tui.wait_exit().success());
    let after = fixture.owner.list_terminals().expect("after");
    assert_eq!(after.len(), 2);
    for terminal in before {
        assert_eq!(
            after
                .iter()
                .find(|item| item.session_id == terminal.session_id)
                .expect("retained")
                .process_id,
            terminal.process_id
        );
    }
    other.claim_control(false).expect("cleanup lease");
    other.kill().expect("cleanup");
}

#[test]
fn navigator_without_attached_panes_exits_on_share_revocation() {
    use std::os::unix::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let (share, token) = fixture
        .owner
        .create_share(
            "navigator",
            ShareRole::Observer,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("share");
    let path = fixture.root.join("nav.token");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .expect("private file");
    file.write_all(token.as_bytes()).expect("token");
    let mut tui = OuterTerminal::start_at(
        &fixture,
        None,
        &["--share-token-file", path.to_str().expect("path")],
    );
    until(|| tui.saw("Superplexr / Sessions"));
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    assert!(!tui.wait_exit().success());
    until(|| tui.saw("\x1b[?1049l"));
    assert!(
        format!(
            "{:?}",
            tui.pair.master.get_termios().expect("termios").local_flags
        )
        .contains("ICANON")
    );
    assert_eq!(
        fixture.owner.list_terminals().expect("host retained")[0].status,
        TerminalSessionStatus::Running
    );
}
impl Drop for OuterTerminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn real_tui_routes_input_resize_paste_confirmation_and_restores_outer_terminal() {
    let fixture = Fixture::new();
    fixture.terminal.release_control().expect("release");
    let before = fixture.owner.list_terminals().expect("list")[0].clone();
    let mut tui = OuterTerminal::start(&fixture, &["--control"]);
    until(|| tui.saw("CONTROL"));
    std::thread::sleep(Duration::from_millis(150));
    let idle_bytes = tui.bytes.lock().expect("output").len();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(
        tui.bytes.lock().expect("output").len(),
        idle_bytes,
        "idle TUI must not repaint"
    );
    tui.send(b"TUI-KEYS\r");
    until(|| fixture.text().contains("TUI-KEYS"));
    tui.send(b"\x1dr");
    until(|| {
        fixture.owner.list_terminals().expect("list")[0]
            .controller_surface_id
            .is_none()
    });
    tui.send(b"BLOCKED-INPUT\r");
    tui.pair
        .master
        .resize(PtySize {
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("outer resize");
    until(|| tui.saw("Observe mode"));
    assert!(!fixture.text().contains("BLOCKED-INPUT"));
    assert_eq!(
        fixture.terminal.snapshot().expect("observe grid").grid,
        GridSize {
            columns: 80,
            rows: 24
        }
    );
    tui.send(b"\x1dc");
    until(|| {
        fixture.terminal.snapshot().expect("controlled grid").grid
            == GridSize {
                columns: 100,
                rows: 29,
            }
    });
    tui.send(b"\x1b[200~REJECTED-PASTE\n\x1b[201~");
    tui.wait_for("y confirms");
    tui.send(b"n");
    until(|| tui.saw("Paste cancelled"));
    assert!(!fixture.text().contains("REJECTED-PASTE"));
    tui.send(b"\x1b[200~ACCEPTED-PASTE\n\x1b[201~");
    std::thread::sleep(Duration::from_millis(100));
    tui.send(b"y");
    until(|| fixture.text().contains("ACCEPTED-PASTE"));
    tui.send(b"\x1dd");
    assert!(tui.wait_exit().success());
    until(|| tui.saw("\x1b[?1049l"));
    let termios = tui.pair.master.get_termios().expect("outer termios");
    assert!(format!("{:?}", termios.local_flags).contains("ICANON"));
    assert!(format!("{:?}", termios.local_flags).contains("ECHO"));
    let after = fixture.owner.list_terminals().expect("list");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].process_id, before.process_id);
    assert_eq!(after[0].status, TerminalSessionStatus::Running);
    assert!(after[0].controller_surface_id.is_none());
    let mut resumed = OuterTerminal::start(&fixture, &[]);
    until(|| resumed.saw("OBSERVE"));
    resumed.send(b"\x1dd");
    assert!(resumed.wait_exit().success());
    assert_eq!(
        fixture.owner.list_terminals().expect("list")[0].process_id,
        before.process_id
    );
}

#[test]
fn real_tui_restores_terminal_on_signal_and_scope_revocation() {
    let fixture = Fixture::new();
    let mut tui = OuterTerminal::start(&fixture, &[]);
    until(|| tui.saw("Observing"));
    let pid = tui.child.process_id().expect("pid");
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .status()
            .expect("signal own child")
            .success()
    );
    assert!(tui.wait_exit().success());
    until(|| tui.saw("\x1b[?1049l"));
    assert!(
        format!(
            "{:?}",
            tui.pair.master.get_termios().expect("termios").local_flags
        )
        .contains("ICANON")
    );

    let (share, token) = fixture
        .owner
        .create_share(
            "tui",
            ShareRole::Observer,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("share");
    use std::os::unix::fs::OpenOptionsExt;
    let path = fixture.root.join("share.token");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .expect("private token file");
    file.write_all(token.as_bytes()).expect("token");
    let mut tui = OuterTerminal::start(
        &fixture,
        &["--share-token-file", path.to_str().expect("path")],
    );
    until(|| tui.saw("Observing"));
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    assert!(!tui.wait_exit().success());
    until(|| tui.saw("\x1b[?1049l"));
    assert!(tui.saw("Session access ended"));
    assert_eq!(
        fixture.owner.list_terminals().expect("live")[0].status,
        TerminalSessionStatus::Running
    );
}

#[test]
fn real_shell_and_vi_work_through_canonical_key_and_frame_projection() {
    let fixture = Fixture::with_program("/bin/sh", &["-i"]);
    fixture.terminal.release_control().expect("release");
    let mut tui = OuterTerminal::start(&fixture, &["--control"]);
    until(|| tui.saw("Control acquired"));
    tui.send(b"printf 'SHELL-%s\\n' 'OK'\r");
    until(|| fixture.text().contains("SHELL-OK"));
    let path = fixture.root.join("note.txt");
    tui.send(format!("vi -u NONE -i NONE {}\r", path.display()).as_bytes());
    until(|| fixture.text().contains("[New File]") || fixture.text().contains("[New]"));
    tui.send("iwide 界 and café\x1b".as_bytes());
    // Separate Escape from ':' so a legacy terminal does not encode it as Alt-:.
    std::thread::sleep(Duration::from_millis(100));
    tui.send(b":wq\r");
    until(|| std::fs::read_to_string(&path).is_ok_and(|text| text == "wide 界 and café\n"));
    tui.send(b"\x1dd");
    assert!(tui.wait_exit().success());
    assert_eq!(
        fixture.owner.list_terminals().expect("list")[0].status,
        TerminalSessionStatus::Running
    );
}

/// Fault-injectable local adapter; only this fixture's native socket is cut.
struct CutConnector {
    socket: PathBuf,
    online: std::sync::atomic::AtomicBool,
    sockets: Mutex<Vec<std::os::unix::net::UnixStream>>,
}
impl superplexr_client::transport::Connector for CutConnector {
    fn connect(
        &self,
        id: uuid::Uuid,
    ) -> Result<superplexr_client::transport::Connection, superplexr_client::ClientError> {
        use superplexr_protocol::{
            ProtocolError,
            wire_v3::{Hello, SyncWire},
        };
        if !self.online.load(std::sync::atomic::Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "fixture offline",
            )
            .into());
        }
        let socket = std::os::unix::net::UnixStream::connect(&self.socket)?;
        self.sockets
            .lock()
            .expect("sockets")
            .push(socket.try_clone()?);
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut wire = SyncWire::new(socket);
        wire.client_handshake(&Hello::new(id, "tui-test", id))
            .map_err(ProtocolError::from)?;
        wire.get_ref().set_read_timeout(None)?;
        let shutdown = wire.get_ref().try_clone()?;
        superplexr_client::transport::Connection::from_negotiated(
            wire,
            |socket| Ok((socket.try_clone()?, socket)),
            move || {
                let _ = shutdown.shutdown(std::net::Shutdown::Both);
            },
        )
    }
}

#[test]
fn dropped_link_repairs_frame_but_never_automatically_resumes_input() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new();
    let id = fixture.terminal.id();
    fixture.terminal.release_control().expect("release");
    let before = fixture.owner.list_terminals().expect("list")[0].clone();
    let connector = Arc::new(CutConnector {
        socket: fixture.root.join("s"),
        online: AtomicBool::new(true),
        sockets: Mutex::new(vec![]),
    });
    let client = ControlClient::connect_with_connector(connector.clone(), None).expect("client");
    let mut view = FocusedSession::attach(&client, id).expect("view");
    live(&mut view);
    view.claim_control().expect("claim");
    connector.online.store(false, Ordering::Release);
    for socket in connector.sockets.lock().expect("sockets").drain(..) {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
    until(|| {
        view.take_update()
            .expect("update")
            .is_some_and(|update| update.status == FeedStatus::Reconnecting)
    });
    assert!(!view.controlling());
    assert!(view.paste("MUST-NOT-REPLAY".into(), true).is_err());
    fixture
        .terminal
        .claim_control(true)
        .expect("host takes control");
    fixture
        .terminal
        .paste(b"DURING-OUTAGE\n".to_vec(), true)
        .expect("host output");
    fixture.terminal.release_control().expect("return control");
    connector.online.store(true, Ordering::Release);
    until(|| {
        view.take_update().expect("update").is_some_and(|update| {
            update.status == FeedStatus::Live
                && update.frame.is_some_and(|frame| {
                    frame
                        .rows
                        .iter()
                        .any(|row| row.text().contains("DURING-OUTAGE"))
                })
        })
    });
    assert!(!view.controlling());
    assert!(view.paste("MUST-NOT-REPLAY".into(), true).is_err());
    view.claim_control()
        .expect("explicit claim after reconnect");
    view.paste("RESUMED-EXPLICITLY\n".into(), true)
        .expect("new input");
    until(|| fixture.text().contains("RESUMED-EXPLICITLY"));
    assert!(!fixture.text().contains("MUST-NOT-REPLAY"));
    let after = fixture.owner.list_terminals().expect("list");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].session_id, before.session_id);
    assert_eq!(after[0].process_id, before.process_id);
}
