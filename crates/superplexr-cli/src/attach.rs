//! `superplexr attach`: a terminal shell for the engine.
//!
//! The daemon owns the PTY, parses every byte and encodes every key; this
//! shell paints the frames it is sent into the terminal it runs in and
//! forwards keystrokes back. It parses nothing itself, which is the point:
//! it works over plain SSH with nothing installed on the far side but the
//! daemon, and it is the smallest proof that engine and shell are separable.
//!
//! Ctrl-] detaches, the way telnet's escape did. The session keeps running.

use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use superplexr_client::{ControlClient, DaemonSession};
use superplexr_core::SessionId;
use superplexr_protocol::ServerEvent;
use superplexr_terminal::{
    CellStyle, FullFrame, GridSize, KeyAction, KeyInput, KeyModifiers, Row, UnderlineStyle,
};

/// The byte that detaches: Ctrl-].
const DETACH: u8 = 0x1d;
/// How long a lone ESC waits for the rest of a sequence before it is Escape.
const ESCAPE_GRACE: Duration = Duration::from_millis(30);
/// How often the local terminal size is re-read.
const RESIZE_POLL: Duration = Duration::from_millis(250);

/// Restores the terminal the shell ran in, whatever happened.
struct RawMode {
    original: libc::termios,
}

impl RawMode {
    fn enter() -> Result<Self, String> {
        let fd = io::stdin().as_raw_fd();
        // SAFETY: termios is plain data; tcgetattr fills it for a valid fd.
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err("stdin is not a terminal".to_owned());
        }
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err("could not put the terminal into raw mode".to_owned());
        }
        Ok(Self { original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let fd = io::stdin().as_raw_fd();
        // Leave the alternate screen, reset attributes, show the cursor.
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        // SAFETY: restoring the termios we read at entry.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &self.original) };
    }
}

fn local_size() -> Option<(u16, u16)> {
    // SAFETY: winsize is plain data; the ioctl fills it for a terminal fd.
    let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
    let ok = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
}

/// What the input thread decoded from the terminal.
enum Input {
    Key(KeyInput),
    Detach,
    Closed,
}

/// Attach to a session and stay until Ctrl-] or the session ends.
pub(crate) fn run(
    socket: &Path,
    session_id: SessionId,
    max_hz: Option<u16>,
    observe: bool,
    take: bool,
) -> Result<(), String> {
    let client = ControlClient::connect(socket).map_err(|error| error.to_string())?;
    let mut session = client.terminal(session_id);
    if let Some(hz) = max_hz {
        session = session.with_max_hz(hz);
    }
    // Control is asked for, and seized only when told to: an agent or another
    // person who holds it keeps it, and this shell watches — and says so.
    let writable = !observe && session.claim_control(take).is_ok();

    let mut frame = session.snapshot().map_err(|error| error.to_string())?;
    let events = session.subscribe().map_err(|error| error.to_string())?;
    let _raw = RawMode::enter()?;
    let mut out = io::stdout();
    out.write_all(b"\x1b[?1049h\x1b[2J")
        .map_err(|e| e.to_string())?;

    let mut local = local_size();
    if writable {
        fit_session(&session, local, &frame);
    }
    paint_all(&mut out, &frame, local)?;

    if !writable {
        // Where a status line would be: the last row, until the next paint.
        let (_, rows) = local.unwrap_or((frame.grid.columns, frame.grid.rows));
        let notice = if observe {
            "watching only (--observe)"
        } else {
            "watching only: another client holds control; rerun with --take to take it"
        };
        let _ = out.write_all(format!("\x1b[{rows};1H\x1b[7m {notice} \x1b[0m").as_bytes());
        let _ = out.flush();
    }
    let input = spawn_input_reader();
    let mut last_resize_poll = Instant::now();
    loop {
        // Frames first: a screen that lags its keystrokes feels broken.
        match events.recv_timeout(Duration::from_millis(8)) {
            Ok(ServerEvent::TerminalFrame { frame: next, .. }) => {
                frame = *next;
                paint_all(&mut out, &frame, local)?;
            }
            Ok(ServerEvent::TerminalDelta { delta, .. }) => match delta.apply_to(&mut frame) {
                Ok(()) => {
                    for changed in &delta.changed_rows {
                        if let Some(row) = frame.rows.get(usize::from(changed.index)) {
                            paint_row(&mut out, usize::from(changed.index), row, &frame, local)?;
                        }
                    }
                    paint_cursor(&mut out, &frame, local)?;
                }
                Err(_) => {
                    // Out of step: take a fresh full frame rather than guess.
                    frame = session.snapshot().map_err(|error| error.to_string())?;
                    paint_all(&mut out, &frame, local)?;
                }
            },
            Ok(ServerEvent::TerminalExited { code, .. }) => {
                drop(_raw);
                eprintln!("session exited with {code}");
                return Ok(());
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                drop(_raw);
                eprintln!("connection to the daemon closed");
                return Ok(());
            }
        }

        match input.try_recv() {
            Ok(Input::Detach) | Ok(Input::Closed) => {
                drop(_raw);
                if writable {
                    eprintln!("detached; the session keeps running");
                } else {
                    eprintln!("detached; you were watching only (control was held elsewhere)");
                }
                return Ok(());
            }
            Ok(Input::Key(key)) => {
                if writable {
                    // A lost key is reported once, not silently dropped.
                    if let Err(error) = session.send_key(key) {
                        drop(_raw);
                        return Err(format!("could not send input: {error}"));
                    }
                }
            }
            Err(_) => {}
        }

        if last_resize_poll.elapsed() >= RESIZE_POLL {
            last_resize_poll = Instant::now();
            let now = local_size();
            if now != local {
                local = now;
                if writable {
                    fit_session(&session, local, &frame);
                }
                out.write_all(b"\x1b[2J").map_err(|e| e.to_string())?;
                paint_all(&mut out, &frame, local)?;
            }
        }
    }
}

/// Size the session to this terminal, the way tmux follows its client.
fn fit_session(session: &DaemonSession, local: Option<(u16, u16)>, frame: &FullFrame) {
    let Some((columns, rows)) = local else { return };
    if frame.grid.columns == columns && frame.grid.rows == rows {
        return;
    }
    if let Ok(grid) = GridSize::new(columns, rows) {
        let _ = session.resize(grid, 0, 0);
    }
}

fn spawn_input_reader() -> Receiver<Input> {
    let (send, receive) = mpsc::channel();
    thread::Builder::new()
        .name("superplexr-attach-input".to_owned())
        .spawn(move || {
            let mut stdin = io::stdin();
            let mut buffer = [0_u8; 256];
            let mut pending: Vec<u8> = Vec::new();
            loop {
                let read = match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        let _ = send.send(Input::Closed);
                        return;
                    }
                    Ok(read) => read,
                };
                pending.extend_from_slice(&buffer[..read]);
                loop {
                    let (input, used) = decode(&pending);
                    if used == 0 {
                        break;
                    }
                    pending.drain(..used);
                    match input {
                        Some(Input::Detach) => {
                            let _ = send.send(Input::Detach);
                            return;
                        }
                        Some(input) => {
                            if send.send(input).is_err() {
                                return;
                            }
                        }
                        None => {}
                    }
                }
                // A lone ESC is Escape once nothing follows it.
                if pending == [0x1b] {
                    thread::sleep(ESCAPE_GRACE);
                    let mut more = [0_u8; 64];
                    // Nothing more can be read without blocking on a plain
                    // stdin; treat the ESC as the key itself.
                    let _ = &mut more;
                    pending.clear();
                    if send
                        .send(Input::Key(key("escape", None, KeyModifiers::default())))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .expect("input reader thread should start");
    receive
}

fn key(name: &str, text: Option<&str>, modifiers: KeyModifiers) -> KeyInput {
    KeyInput {
        physical_key: name.to_owned(),
        logical_key: name.to_owned(),
        text: text.map(str::to_owned),
        modifiers,
        consumed_modifiers: KeyModifiers::default(),
        action: KeyAction::Press,
        composing: false,
        unshifted_codepoint: (name.chars().count() == 1)
            .then(|| name.chars().next())
            .flatten(),
    }
}

/// Decode one key from the front of `bytes`. Returns the key (or none for a
/// sequence that is deliberately ignored) and how many bytes it used; zero
/// means more bytes are needed.
fn decode(bytes: &[u8]) -> (Option<Input>, usize) {
    let Some(&first) = bytes.first() else {
        return (None, 0);
    };
    let plain = KeyModifiers::default();
    let ctrl = KeyModifiers {
        control: true,
        ..plain
    };
    match first {
        DETACH => (Some(Input::Detach), 1),
        b'\r' | b'\n' => (Some(Input::Key(key("enter", None, plain))), 1),
        b'\t' => (Some(Input::Key(key("tab", None, plain))), 1),
        0x7f | 0x08 => (Some(Input::Key(key("backspace", None, plain))), 1),
        0x1b => decode_escape(bytes),
        0x01..=0x1a => {
            let letter = (b'a' + first - 1) as char;
            let name = letter.to_string();
            (Some(Input::Key(key(&name, None, ctrl))), 1)
        }
        0x00 | 0x1c..=0x1f => (None, 1),
        _ => {
            // A UTF-8 scalar; wait for the rest of it if it is incomplete.
            let width = match first {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return (None, 1),
            };
            if bytes.len() < width {
                return (None, 0);
            }
            match std::str::from_utf8(&bytes[..width]) {
                Ok(text) => {
                    let modifiers = KeyModifiers {
                        shift: text.chars().next().is_some_and(char::is_uppercase),
                        ..plain
                    };
                    let name = text.to_lowercase();
                    (Some(Input::Key(key(&name, Some(text), modifiers))), width)
                }
                Err(_) => (None, width),
            }
        }
    }
}

fn decode_escape(bytes: &[u8]) -> (Option<Input>, usize) {
    // ESC alone: need more, or it is Escape (handled by the grace timer).
    let Some(&second) = bytes.get(1) else {
        return (None, 0);
    };
    let plain = KeyModifiers::default();
    if second != b'[' && second != b'O' {
        // ESC x: Alt+x.
        let (inner, used) = decode(&bytes[1..]);
        return match inner {
            Some(Input::Key(mut key)) => {
                key.modifiers.alt = true;
                (Some(Input::Key(key)), used + 1)
            }
            other => (other, used + 1),
        };
    }
    // CSI: parameters then a final byte in 0x40..=0x7e.
    let mut index = 2;
    while let Some(&byte) = bytes.get(index) {
        if (0x40..=0x7e).contains(&byte) {
            let params = std::str::from_utf8(&bytes[2..index]).unwrap_or("");
            let mut fields = params.split(';');
            let number = fields.next().unwrap_or("");
            let modifiers = fields
                .next()
                .and_then(|m| m.parse::<u8>().ok())
                .map_or(plain, |m| {
                    let bits = m.saturating_sub(1);
                    KeyModifiers {
                        shift: bits & 1 != 0,
                        alt: bits & 2 != 0,
                        control: bits & 4 != 0,
                        ..plain
                    }
                });
            let name = match (byte, number) {
                (b'A', _) => "up",
                (b'B', _) => "down",
                (b'C', _) => "right",
                (b'D', _) => "left",
                (b'H', _) => "home",
                (b'F', _) => "end",
                (b'~', "1") | (b'~', "7") => "home",
                (b'~', "4") | (b'~', "8") => "end",
                (b'~', "2") => "insert",
                (b'~', "3") => "delete",
                (b'~', "5") => "pageup",
                (b'~', "6") => "pagedown",
                (b'Z', _) => {
                    return (
                        Some(Input::Key(key(
                            "tab",
                            None,
                            KeyModifiers {
                                shift: true,
                                ..plain
                            },
                        ))),
                        index + 1,
                    );
                }
                _ => return (None, index + 1),
            };
            return (Some(Input::Key(key(name, None, modifiers))), index + 1);
        }
        index += 1;
        if index > 32 {
            return (None, index);
        }
    }
    (None, 0)
}

fn paint_all(
    out: &mut impl Write,
    frame: &FullFrame,
    local: Option<(u16, u16)>,
) -> Result<(), String> {
    for (index, row) in frame.rows.iter().enumerate() {
        paint_row(out, index, row, frame, local)?;
    }
    paint_cursor(out, frame, local)
}

fn paint_row(
    out: &mut impl Write,
    index: usize,
    row: &Row,
    frame: &FullFrame,
    local: Option<(u16, u16)>,
) -> Result<(), String> {
    let (columns, rows) = local.unwrap_or((frame.grid.columns, frame.grid.rows));
    if index >= usize::from(rows) {
        return Ok(());
    }
    let mut line = format!("\x1b[{};1H\x1b[?25l", index + 1);
    let mut current: Option<u32> = None;
    let mut column = 0_u16;
    for cell in &row.cells {
        if column >= columns {
            break;
        }
        if cell.width == 0 {
            continue;
        }
        if current != Some(cell.style_index) {
            if let Some(style) = frame.styles.get(cell.style_index as usize) {
                line.push_str(&sgr(style));
            }
            current = Some(cell.style_index);
        }
        if cell.grapheme.is_empty() {
            line.push(' ');
        } else {
            line.push_str(&cell.grapheme);
        }
        column = column.saturating_add(u16::from(cell.width.max(1)));
    }
    line.push_str("\x1b[0m\x1b[K");
    out.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

fn paint_cursor(
    out: &mut impl Write,
    frame: &FullFrame,
    local: Option<(u16, u16)>,
) -> Result<(), String> {
    let (columns, rows) = local.unwrap_or((frame.grid.columns, frame.grid.rows));
    let text = match frame.cursor {
        Some(cursor) if cursor.row < rows && cursor.column < columns => {
            format!("\x1b[{};{}H\x1b[?25h", cursor.row + 1, cursor.column + 1)
        }
        _ => "\x1b[?25l".to_owned(),
    };
    out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// The SGR sequence that selects `style`, from a reset.
fn sgr(style: &CellStyle) -> String {
    let mut parts = vec!["0".to_owned()];
    if style.bold {
        parts.push("1".into());
    }
    if style.faint {
        parts.push("2".into());
    }
    if style.italic {
        parts.push("3".into());
    }
    match style.underline {
        UnderlineStyle::None => {}
        UnderlineStyle::Double => parts.push("21".into()),
        UnderlineStyle::Curly => parts.push("4:3".into()),
        UnderlineStyle::Dotted => parts.push("4:4".into()),
        UnderlineStyle::Dashed => parts.push("4:5".into()),
        UnderlineStyle::Single | UnderlineStyle::Unknown => parts.push("4".into()),
    }
    if style.blink {
        parts.push("5".into());
    }
    if style.inverse {
        parts.push("7".into());
    }
    if style.invisible {
        parts.push("8".into());
    }
    if style.strikethrough {
        parts.push("9".into());
    }
    if style.overline {
        parts.push("53".into());
    }
    parts.push(format!(
        "38;2;{};{};{}",
        style.foreground.red, style.foreground.green, style.foreground.blue
    ));
    parts.push(format!(
        "48;2;{};{};{}",
        style.background.red, style.background.green, style.background.blue
    ));
    format!("\x1b[{}m", parts.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use superplexr_terminal::{Cell, Rgb};

    fn key_of(bytes: &[u8]) -> KeyInput {
        match decode(bytes) {
            (Some(Input::Key(key)), used) => {
                assert_eq!(used, bytes.len(), "the whole sequence is consumed");
                key
            }
            other => panic!("expected a key from {bytes:?}, got used={}", other.1),
        }
    }

    #[test]
    fn plain_and_control_keys_decode_the_way_the_desktop_sends_them() {
        let a = key_of(b"a");
        assert_eq!(
            (a.physical_key.as_str(), a.text.as_deref()),
            ("a", Some("a"))
        );
        assert!(!a.modifiers.shift);
        let upper = key_of(b"A");
        assert_eq!(
            (upper.physical_key.as_str(), upper.text.as_deref()),
            ("a", Some("A"))
        );
        assert!(upper.modifiers.shift);
        let ctrl_c = key_of(b"\x03");
        assert_eq!(ctrl_c.physical_key, "c");
        assert!(ctrl_c.modifiers.control && ctrl_c.text.is_none());
        assert_eq!(key_of(b"\r").physical_key, "enter");
        assert_eq!(key_of(b"\x7f").physical_key, "backspace");
        let wide = key_of("日".as_bytes());
        assert_eq!(wide.text.as_deref(), Some("日"));
    }

    #[test]
    fn escape_sequences_become_named_keys_with_modifiers() {
        assert_eq!(key_of(b"\x1b[A").physical_key, "up");
        assert_eq!(key_of(b"\x1b[3~").physical_key, "delete");
        let ctrl_right = key_of(b"\x1b[1;5C");
        assert_eq!(ctrl_right.physical_key, "right");
        assert!(ctrl_right.modifiers.control && !ctrl_right.modifiers.shift);
        let alt_x = key_of(b"\x1bx");
        assert_eq!(alt_x.physical_key, "x");
        assert!(alt_x.modifiers.alt);
        assert!(key_of(b"\x1b[Z").modifiers.shift, "back-tab is shift+tab");
    }

    #[test]
    fn detach_and_incomplete_input_are_recognised() {
        assert!(matches!(decode(&[DETACH]), (Some(Input::Detach), 1)));
        assert_eq!(decode(b"\x1b[").1, 0, "an unfinished CSI waits for more");
        assert_eq!(
            decode(&[0xe6, 0x97]).1,
            0,
            "a split UTF-8 scalar waits for more"
        );
    }

    #[test]
    fn a_row_paints_its_styles_once_per_run_and_skips_spacers() {
        let style = |bg: u8| CellStyle {
            foreground: Rgb {
                red: 200,
                green: 200,
                blue: 200,
            },
            background: Rgb {
                red: bg,
                green: 0,
                blue: 0,
            },
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: UnderlineStyle::None,
        };
        let cell = |g: &str, w: u8, s: u32| Cell {
            grapheme: g.to_owned(),
            width: w,
            style_index: s,
            hyperlink: None,
        };
        let frame = FullFrame {
            sequence: 1,
            grid: GridSize::new(6, 1).expect("grid"),
            rows: vec![std::sync::Arc::new(Row {
                wrapped: false,
                cells: vec![
                    cell("a", 1, 0),
                    cell("b", 1, 0),
                    cell("日", 2, 1),
                    cell("", 0, 1),
                    cell("c", 1, 0),
                ],
            })],
            styles: vec![style(0), style(255)],
            cursor: None,
            default_foreground: Rgb {
                red: 200,
                green: 200,
                blue: 200,
            },
            default_background: Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            mouse_tracking: false,
            title: None,
            current_directory: None,
        };
        let mut out = Vec::new();
        paint_row(&mut out, 0, &frame.rows[0], &frame, None).expect("paint");
        let text = String::from_utf8(out).expect("utf8");
        assert_eq!(
            text.matches("48;2;255;0;0").count(),
            1,
            "the red run is selected once"
        );
        assert_eq!(
            text.matches("48;2;0;0;0").count(),
            2,
            "back to the first style after it"
        );
        assert!(
            text.contains("ab\x1b[") && text.contains("日") && text.ends_with("c\x1b[0m\x1b[K")
        );
        assert_eq!(
            text.matches('日').count(),
            1,
            "the spacer after a wide glyph paints nothing"
        );
    }
}
