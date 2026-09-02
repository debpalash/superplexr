//! Model Context Protocol bridge for termi9ne Faults.
//!
//! Agents already run inside termi9ne terminals; this lets them read and act
//! on the failures the runtime recorded instead of re-reading scrollback. It
//! speaks newline-delimited JSON-RPC 2.0 over stdio, the MCP stdio transport.
//!
//! The bridge exposes only Fault operations. It deliberately cannot start
//! terminals, mutate Missions, or read terminal contents: an agent that can
//! debug does not need the authority to do everything else.

use std::io::{BufRead, Write};

use clap::Parser;
use serde_json::{Value, json};
use termi9ne_client::ControlClient;
use termi9ne_core::FaultId;
use termi9ne_protocol::{FaultInput, FaultKind, FaultSummary, default_socket_path};

/// MCP revision this bridge implements.
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Parser)]
#[command(about = "Expose termi9ne Faults to agents over MCP stdio")]
struct Args {
    /// Control socket of the termi9ne runtime.
    #[arg(long, default_value_os_t = default_socket_path())]
    socket: std::path::PathBuf,
}

/// The Fault operations this bridge exposes. Kept behind a trait so the
/// JSON-RPC layer is testable without a running daemon.
trait FaultBackend {
    fn list(&self, include_closed: bool) -> Result<Vec<FaultSummary>, String>;
    fn get(&self, fault_id: FaultId) -> Result<FaultSummary, String>;
    fn reproduce(
        &self,
        fault_id: FaultId,
        timeout_seconds: Option<u16>,
    ) -> Result<FaultSummary, String>;
    fn resolve(&self, fault_id: FaultId, note: String) -> Result<FaultSummary, String>;
    fn dismiss(&self, fault_id: FaultId, note: String) -> Result<FaultSummary, String>;
    fn report(&self, fault: FaultInput) -> Result<FaultSummary, String>;
}

struct ControlBackend {
    client: ControlClient,
}

impl FaultBackend for ControlBackend {
    fn list(&self, include_closed: bool) -> Result<Vec<FaultSummary>, String> {
        self.client
            .list_faults(None, None, include_closed)
            .map_err(|error| error.to_string())
    }

    fn get(&self, fault_id: FaultId) -> Result<FaultSummary, String> {
        self.client.fault(fault_id).map_err(|e| e.to_string())
    }

    fn reproduce(
        &self,
        fault_id: FaultId,
        timeout_seconds: Option<u16>,
    ) -> Result<FaultSummary, String> {
        self.client
            .reproduce_fault(fault_id, timeout_seconds)
            .map_err(|error| error.to_string())
    }

    fn resolve(&self, fault_id: FaultId, note: String) -> Result<FaultSummary, String> {
        self.client
            .resolve_fault(fault_id, note)
            .map_err(|error| error.to_string())
    }

    fn dismiss(&self, fault_id: FaultId, note: String) -> Result<FaultSummary, String> {
        self.client
            .dismiss_fault(fault_id, note)
            .map_err(|error| error.to_string())
    }

    fn report(&self, fault: FaultInput) -> Result<FaultSummary, String> {
        self.client
            .report_fault(fault)
            .map_err(|error| error.to_string())
    }
}

fn main() {
    let args = Args::parse();
    let client = match ControlClient::connect(&args.socket) {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "termi9ne-mcp could not reach the runtime at {}: {error}",
                args.socket.display()
            );
            std::process::exit(1);
        }
    };
    let backend = ControlBackend { client };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_line(&backend, &line) else {
            continue;
        };
        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}

/// Handle one JSON-RPC line, returning the response to write, if any.
/// Notifications (messages without an `id`) produce no response.
fn handle_line(backend: &impl FaultBackend, line: &str) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        // A malformed line has no id, so the only correct reply is a
        // parse error with a null id.
        Err(error) => {
            return Some(
                error_response(Value::Null, -32700, &format!("invalid JSON: {error}")).to_string(),
            );
        }
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    let result = dispatch(backend, method, &params);
    let id = id?; // notifications get no reply
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Err(RpcError { code, message }) => error_response(id, code, &message).to_string(),
    })
}

struct RpcError {
    code: i64,
    message: String,
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn dispatch(backend: &impl FaultBackend, method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "termi9ne-faults", "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => call_tool(backend, params),
        other => Err(RpcError {
            code: -32601,
            message: format!("unknown method {other}"),
        }),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "fault_list",
            "description": "List recorded failures, newest first. Each Fault carries the exact command, its directory, the failing output, and whether the latest replay still fails.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "include_closed": {
                        "type": "boolean",
                        "description": "Include Faults already resolved or dismissed. Defaults to false."
                    }
                }
            }
        }),
        json!({
            "name": "fault_show",
            "description": "Read one Fault by id, including its full failing output.",
            "inputSchema": {
                "type": "object",
                "properties": {"fault_id": {"type": "string"}},
                "required": ["fault_id"]
            }
        }),
        json!({
            "name": "fault_repro",
            "description": "Replay a Fault's exact command in its recorded directory and return the receipt. Use this to check whether a fix worked; the result is measured, not asserted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fault_id": {"type": "string"},
                    "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": 900}
                },
                "required": ["fault_id"]
            }
        }),
        json!({
            "name": "fault_resolve",
            "description": "Close a Fault as fixed. Refused unless its latest replay passed, so run fault_repro first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fault_id": {"type": "string"},
                    "note": {"type": "string", "description": "What fixed it."}
                },
                "required": ["fault_id", "note"]
            }
        }),
        json!({
            "name": "fault_dismiss",
            "description": "Close a Fault without replay evidence, recording why. Use for known flakes and intentional failures.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fault_id": {"type": "string"},
                    "note": {"type": "string"}
                },
                "required": ["fault_id", "note"]
            }
        }),
        json!({
            "name": "fault_report",
            "description": "Record a failure you observed, with the command needed to replay it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "cwd": {"type": "string", "description": "Absolute directory the command ran in."},
                    "summary": {"type": "string"},
                    "output": {"type": "string"},
                    "exit_code": {"type": "integer"},
                    "kind": {
                        "type": "string",
                        "enum": ["command_failed", "test_failed", "build_failed", "panic", "crashed"]
                    }
                },
                "required": ["command", "cwd", "summary"]
            }
        }),
    ]
}

fn call_tool(backend: &impl FaultBackend, params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "tools/call requires a name".to_owned(),
        })?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // A failed tool is reported inside the result as `isError`, per MCP, so
    // the agent can read and act on the reason instead of seeing a transport
    // failure.
    let outcome = run_tool(backend, name, &arguments);
    Ok(match outcome {
        Ok(value) => json!({
            "content": [{"type": "text", "text": to_pretty(&value)}],
            "isError": false,
        }),
        Err(message) => json!({
            "content": [{"type": "text", "text": message}],
            "isError": true,
        }),
    })
}

fn run_tool(backend: &impl FaultBackend, name: &str, arguments: &Value) -> Result<Value, String> {
    match name {
        "fault_list" => {
            let include_closed = arguments
                .get("include_closed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let faults = backend.list(include_closed)?;
            Ok(json!({"faults": faults, "open": faults_open(&faults)}))
        }
        "fault_show" => {
            let fault_id = fault_id_argument(arguments)?;
            backend.get(fault_id).map(|fault| json!(fault))
        }
        "fault_repro" => {
            let fault_id = fault_id_argument(arguments)?;
            let timeout_seconds = arguments
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok());
            let fault = backend.reproduce(fault_id, timeout_seconds)?;
            Ok(json!({
                "fault": fault,
                "still_fails": !fault.repro_passes(),
            }))
        }
        "fault_resolve" => {
            let fault_id = fault_id_argument(arguments)?;
            let note = note_argument(arguments)?;
            backend.resolve(fault_id, note).map(|fault| json!(fault))
        }
        "fault_dismiss" => {
            let fault_id = fault_id_argument(arguments)?;
            let note = note_argument(arguments)?;
            backend.dismiss(fault_id, note).map(|fault| json!(fault))
        }
        "fault_report" => {
            let fault = fault_input(arguments)?;
            backend.report(fault).map(|fault| json!(fault))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

fn faults_open(faults: &[FaultSummary]) -> usize {
    faults.iter().filter(|fault| fault.is_open()).count()
}

fn fault_id_argument(arguments: &Value) -> Result<FaultId, String> {
    let raw = arguments
        .get("fault_id")
        .and_then(Value::as_str)
        .ok_or("fault_id is required")?;
    raw.parse::<FaultId>()
        .map_err(|_| format!("{raw} is not a Fault id"))
}

fn note_argument(arguments: &Value) -> Result<String, String> {
    let note = arguments
        .get("note")
        .and_then(Value::as_str)
        .ok_or("note is required")?;
    if note.trim().is_empty() {
        return Err("note must not be blank".to_owned());
    }
    Ok(note.to_owned())
}

fn fault_input(arguments: &Value) -> Result<FaultInput, String> {
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .filter(|command| !command.trim().is_empty())
        .ok_or("command is required")?
        .to_owned();
    let cwd = arguments
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or("cwd is required")?;
    let cwd = std::path::PathBuf::from(cwd);
    if !cwd.is_absolute() {
        return Err("cwd must be an absolute path".to_owned());
    }
    let summary = arguments
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .ok_or("summary is required")?
        .to_owned();
    let kind = match arguments.get("kind").and_then(Value::as_str) {
        Some("test_failed") => FaultKind::TestFailed,
        Some("build_failed") => FaultKind::BuildFailed,
        Some("panic") => FaultKind::Panic,
        Some("crashed") => FaultKind::Crashed,
        Some("command_failed") | None => FaultKind::CommandFailed,
        Some(other) => return Err(format!("unknown kind {other}")),
    };
    Ok(FaultInput {
        kind,
        command,
        cwd,
        exit_code: arguments
            .get("exit_code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        revision: None,
        summary,
        output: arguments
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        session_id: None,
        mission_id: None,
        run_id: None,
    })
}

fn to_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, path::PathBuf};
    use termi9ne_protocol::{FaultSource, FaultState, ReproReceipt};

    fn sample(open: bool) -> FaultSummary {
        FaultSummary {
            fault_id: FaultId::new(),
            kind: FaultKind::TestFailed,
            command: "cargo test".to_owned(),
            cwd: PathBuf::from("/work"),
            exit_code: Some(101),
            revision: None,
            summary: "tests failed".to_owned(),
            output: "assertion failed".to_owned(),
            session_id: None,
            mission_id: None,
            run_id: None,
            source: FaultSource::TerminalExit,
            observed_at_unix_micros: 1,
            state: if open {
                FaultState::Open
            } else {
                FaultState::Dismissed {
                    note: "flake".to_owned(),
                    at_unix_micros: 2,
                }
            },
            repro: None,
            repro_attempts: 0,
            fix_run_id: None,
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        faults: Vec<FaultSummary>,
        calls: RefCell<Vec<String>>,
        fail_with: Option<String>,
    }

    impl FaultBackend for FakeBackend {
        fn list(&self, include_closed: bool) -> Result<Vec<FaultSummary>, String> {
            self.calls
                .borrow_mut()
                .push(format!("list:{include_closed}"));
            Ok(self
                .faults
                .iter()
                .filter(|fault| include_closed || fault.is_open())
                .cloned()
                .collect())
        }

        fn get(&self, fault_id: FaultId) -> Result<FaultSummary, String> {
            self.calls.borrow_mut().push("get".to_owned());
            self.faults
                .iter()
                .find(|fault| fault.fault_id == fault_id)
                .cloned()
                .ok_or_else(|| format!("no Fault {fault_id}"))
        }

        fn reproduce(
            &self,
            fault_id: FaultId,
            timeout_seconds: Option<u16>,
        ) -> Result<FaultSummary, String> {
            self.calls
                .borrow_mut()
                .push(format!("repro:{timeout_seconds:?}"));
            let mut fault = self.get(fault_id)?;
            fault.repro = Some(ReproReceipt {
                attempted_at_unix_micros: 3,
                reproduced: false,
                exit_code: Some(0),
                output: String::new(),
                duration_ms: 10,
                revision: None,
                error: None,
            });
            fault.repro_attempts = 1;
            Ok(fault)
        }

        fn resolve(&self, fault_id: FaultId, _note: String) -> Result<FaultSummary, String> {
            self.calls.borrow_mut().push("resolve".to_owned());
            if let Some(message) = &self.fail_with {
                return Err(message.clone());
            }
            self.get(fault_id)
        }

        fn dismiss(&self, fault_id: FaultId, _note: String) -> Result<FaultSummary, String> {
            self.calls.borrow_mut().push("dismiss".to_owned());
            self.get(fault_id)
        }

        fn report(&self, fault: FaultInput) -> Result<FaultSummary, String> {
            self.calls
                .borrow_mut()
                .push(format!("report:{}", fault.command));
            let mut recorded = sample(true);
            recorded.command = fault.command;
            recorded.cwd = fault.cwd;
            recorded.kind = fault.kind;
            Ok(recorded)
        }
    }

    fn call(backend: &FakeBackend, name: &str, arguments: Value) -> Value {
        let line = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string();
        let response = handle_line(backend, &line).expect("a request always gets a response");
        serde_json::from_str(&response).expect("responses are JSON")
    }

    fn tool_text(response: &Value) -> String {
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool results carry text")
            .to_owned()
    }

    #[test]
    fn initialize_and_tools_list_describe_only_fault_operations() {
        let backend = FakeBackend::default();
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}).to_string();
        let response: Value =
            serde_json::from_str(&handle_line(&backend, &line).expect("response")).unwrap();
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(response["result"]["capabilities"]["tools"].is_object());

        let line = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string();
        let response: Value =
            serde_json::from_str(&handle_line(&backend, &line).expect("response")).unwrap();
        let names = response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "fault_list",
                "fault_show",
                "fault_repro",
                "fault_resolve",
                "fault_dismiss",
                "fault_report"
            ]
        );
        // Nothing here can start terminals or touch Missions.
        assert!(!names.iter().any(|name| name.contains("terminal")));
    }

    #[test]
    fn notifications_get_no_response_and_unknown_methods_do() {
        let backend = FakeBackend::default();
        let notification =
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
        assert!(handle_line(&backend, &notification).is_none());

        let unknown = json!({"jsonrpc": "2.0", "id": 9, "method": "nope"}).to_string();
        let response: Value =
            serde_json::from_str(&handle_line(&backend, &unknown).expect("response")).unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn malformed_input_produces_a_parse_error_rather_than_a_crash() {
        let backend = FakeBackend::default();
        let response: Value =
            serde_json::from_str(&handle_line(&backend, "{not json").expect("response")).unwrap();
        assert_eq!(response["error"]["code"], -32700);
        assert_eq!(response["id"], Value::Null);
    }

    #[test]
    fn listing_hides_closed_faults_unless_asked() {
        let backend = FakeBackend {
            faults: vec![sample(true), sample(false)],
            ..FakeBackend::default()
        };
        let response = call(&backend, "fault_list", json!({}));
        assert_eq!(response["result"]["isError"], false);
        let text = tool_text(&response);
        let payload: Value = serde_json::from_str(&text).expect("payload");
        assert_eq!(payload["faults"].as_array().expect("faults").len(), 1);
        assert_eq!(payload["open"], 1);

        let response = call(&backend, "fault_list", json!({"include_closed": true}));
        let payload: Value = serde_json::from_str(&tool_text(&response)).expect("payload");
        assert_eq!(payload["faults"].as_array().expect("faults").len(), 2);
        assert_eq!(payload["open"], 1);
    }

    #[test]
    fn replaying_reports_whether_the_failure_still_happens() {
        let fault = sample(true);
        let fault_id = fault.fault_id;
        let backend = FakeBackend {
            faults: vec![fault],
            ..FakeBackend::default()
        };
        let response = call(
            &backend,
            "fault_repro",
            json!({"fault_id": fault_id.to_string(), "timeout_seconds": 30}),
        );
        let payload: Value = serde_json::from_str(&tool_text(&response)).expect("payload");
        assert_eq!(payload["still_fails"], false);
        assert!(
            backend
                .calls
                .borrow()
                .contains(&"repro:Some(30)".to_owned()),
            "the timeout reaches the backend: {:?}",
            backend.calls.borrow()
        );
    }

    #[test]
    fn a_refused_resolve_is_returned_as_a_readable_tool_error() {
        let fault = sample(true);
        let fault_id = fault.fault_id;
        let backend = FakeBackend {
            faults: vec![fault],
            fail_with: Some("cannot be resolved until a replay passes".to_owned()),
            ..FakeBackend::default()
        };
        let response = call(
            &backend,
            "fault_resolve",
            json!({"fault_id": fault_id.to_string(), "note": "fixed"}),
        );
        assert_eq!(response["result"]["isError"], true);
        assert!(tool_text(&response).contains("replay passes"));
    }

    #[test]
    fn bad_arguments_are_rejected_before_reaching_the_runtime() {
        let backend = FakeBackend::default();
        for (tool, arguments, expected) in [
            ("fault_show", json!({}), "fault_id is required"),
            (
                "fault_show",
                json!({"fault_id": "not-a-uuid"}),
                "is not a Fault id",
            ),
            (
                "fault_resolve",
                json!({"fault_id": FaultId::new().to_string(), "note": "  "}),
                "note must not be blank",
            ),
            (
                "fault_report",
                json!({"command": "x", "cwd": "relative", "summary": "s"}),
                "cwd must be an absolute path",
            ),
            (
                "fault_report",
                json!({"command": "  ", "cwd": "/tmp", "summary": "s"}),
                "command is required",
            ),
            ("unknown_tool", json!({}), "unknown tool"),
        ] {
            let response = call(&backend, tool, arguments);
            assert_eq!(response["result"]["isError"], true, "{tool}");
            assert!(
                tool_text(&response).contains(expected),
                "{tool}: {}",
                tool_text(&response)
            );
        }
        assert!(
            backend.calls.borrow().is_empty(),
            "invalid calls never reach the backend"
        );
    }

    #[test]
    fn reporting_maps_kinds_and_defaults_to_a_plain_command_failure() {
        let backend = FakeBackend::default();
        let response = call(
            &backend,
            "fault_report",
            json!({
                "command": "cargo build",
                "cwd": "/work",
                "summary": "build broke",
                "kind": "build_failed",
                "exit_code": 101
            }),
        );
        assert_eq!(response["result"]["isError"], false);
        let payload: Value = serde_json::from_str(&tool_text(&response)).expect("payload");
        assert_eq!(payload["kind"], "build_failed");
        assert_eq!(payload["command"], "cargo build");

        let response = call(
            &backend,
            "fault_report",
            json!({"command": "x", "cwd": "/work", "summary": "s"}),
        );
        let payload: Value = serde_json::from_str(&tool_text(&response)).expect("payload");
        assert_eq!(payload["kind"], "command_failed");

        let response = call(
            &backend,
            "fault_report",
            json!({"command": "x", "cwd": "/work", "summary": "s", "kind": "nonsense"}),
        );
        assert_eq!(response["result"]["isError"], true);
    }
}
