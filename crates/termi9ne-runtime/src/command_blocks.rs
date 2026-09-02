//! OSC 133 semantic prompt tracking.
//!
//! Shells that emit OSC 133 mark where a prompt begins, where the typed
//! command begins, where its output begins, and what it exited with. That is
//! exactly the evidence a Fault needs, so the runtime watches the raw PTY
//! stream for those marks rather than guessing from rendered text.
//!
//! The parser is deliberately small and allocation-bounded: it never buffers
//! more than one command and a capped slice of its output, and it tolerates
//! sequences split across reads.
//!
//! Marks (`ST` is either `ESC \` or `BEL`):
//! - `OSC 133 ; A ST` — prompt start
//! - `OSC 133 ; B ST` — command start (typed input begins)
//! - `OSC 133 ; C ST` — command executed (output begins)
//! - `OSC 133 ; D [; exit] ST` — command finished

/// Longest command line retained. Longer input is truncated rather than
/// growing the actor's memory with a pasted payload.
const MAX_COMMAND_BYTES: usize = 4_096;
/// Longest captured output retained per command block.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Longest OSC sequence tolerated before the parser gives up on it.
const MAX_SEQUENCE_BYTES: usize = 1_024;

/// One completed shell command observed through OSC 133 marks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandBlock {
    /// The command line as echoed between the `B` and `C` marks. Empty when
    /// the shell reported no command text.
    pub command: String,
    /// Exit status from the `D` mark, absent when the shell omitted it.
    pub exit_code: Option<i32>,
    /// Plain-text output between `C` and `D`, control sequences removed.
    pub output: String,
}

impl CommandBlock {
    /// True when this block reported a failure worth recording.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.exit_code.is_some_and(|code| code != 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Outside any marked region, or inside a prompt.
    Idle,
    /// Between `B` and `C`: the shell is echoing the typed command.
    ReadingCommand,
    /// Between `C` and `D`: the command is running and producing output.
    ReadingOutput,
}

/// Streaming OSC 133 tracker. Feed it every PTY byte in order.
#[derive(Debug)]
pub struct CommandBlockTracker {
    phase: Phase,
    /// Raw bytes, decoded once at finish so a multi-byte character split
    /// across reads is never turned into replacement characters.
    command: Vec<u8>,
    output: Vec<u8>,
    command_truncated: bool,
    output_truncated: bool,
    /// Partial escape sequence carried across reads.
    pending: Vec<u8>,
    /// True while `pending` holds an incomplete escape sequence.
    in_escape: bool,
    /// Set once a sequence exceeds [`MAX_SEQUENCE_BYTES`], so its remainder is
    /// discarded instead of being treated as output.
    abandoned: bool,
}

impl Default for CommandBlockTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandBlockTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            command: Vec::new(),
            output: Vec::new(),
            command_truncated: false,
            output_truncated: false,
            pending: Vec::new(),
            in_escape: false,
            abandoned: false,
        }
    }

    /// Consume PTY output, returning every command block that completed.
    ///
    /// Shells without OSC 133 produce no blocks at all, which is the intended
    /// behaviour: silence is better than a guessed command.
    pub fn consume(&mut self, bytes: &[u8]) -> Vec<CommandBlock> {
        let mut finished = Vec::new();
        for byte in bytes {
            self.push_byte(*byte, &mut finished);
        }
        finished
    }

    fn push_byte(&mut self, byte: u8, finished: &mut Vec<CommandBlock>) {
        if self.in_escape {
            self.pending.push(byte);
            if self.pending.len() > MAX_SEQUENCE_BYTES {
                // Runaway sequence: stop buffering, but keep swallowing until a
                // terminator so its body never lands in captured output.
                self.pending.clear();
                self.abandoned = true;
                self.in_escape = false;
                return;
            }
            if let Some(complete) = self.take_complete_sequence() {
                self.in_escape = false;
                self.handle_sequence(&complete, finished);
            }
            return;
        }
        if self.abandoned {
            // Discard the tail of an abandoned sequence up to its terminator.
            if byte == 0x07 || byte == b'\\' {
                self.abandoned = false;
            }
            return;
        }
        if byte == 0x1b {
            self.in_escape = true;
            self.pending.clear();
            self.pending.push(byte);
            return;
        }
        self.record_text_byte(byte);
    }

    /// Return the buffered sequence once it is terminated.
    fn take_complete_sequence(&mut self) -> Option<Vec<u8>> {
        let bytes = &self.pending;
        if bytes.len() < 2 {
            return None;
        }
        // OSC: ESC ] ... (BEL | ESC \)
        if bytes[1] == b']' {
            let terminated = bytes.last() == Some(&0x07)
                || (bytes.len() >= 4 && bytes[bytes.len() - 2..] == [0x1b, b'\\']);
            return terminated.then(|| std::mem::take(&mut self.pending));
        }
        // CSI: ESC [ ... final byte in 0x40..=0x7e
        if bytes[1] == b'[' {
            let final_byte = *bytes.last().unwrap_or(&0);
            return (bytes.len() > 2 && (0x40..=0x7e).contains(&final_byte))
                .then(|| std::mem::take(&mut self.pending));
        }
        // Any other two-byte escape (including ESC \) ends immediately.
        Some(std::mem::take(&mut self.pending))
    }

    fn handle_sequence(&mut self, sequence: &[u8], finished: &mut Vec<CommandBlock>) {
        let Some(body) = osc_133_body(sequence) else {
            // Every other escape is display formatting; it never reaches the
            // captured text.
            return;
        };
        let mut parts = body.splitn(2, ';');
        match parts.next() {
            Some("A") => {
                // A new prompt cancels any half-read command.
                self.phase = Phase::Idle;
                self.reset_buffers();
            }
            Some("B") => {
                self.phase = Phase::ReadingCommand;
                self.reset_buffers();
            }
            Some("C") => {
                self.phase = Phase::ReadingOutput;
                self.output.clear();
                self.output_truncated = false;
            }
            Some("D") => {
                if self.phase != Phase::Idle {
                    // Shells append extra fields after the status, such as
                    // `D;3;aid=7`; only the first field is the exit code.
                    let exit_code = parts
                        .next()
                        .and_then(|rest| rest.split(';').next())
                        .map(str::trim)
                        .filter(|code| !code.is_empty())
                        .and_then(|code| code.parse::<i32>().ok());
                    finished.push(self.finish_block(exit_code));
                }
                self.phase = Phase::Idle;
                self.reset_buffers();
            }
            _ => {}
        }
    }

    fn finish_block(&mut self, exit_code: Option<i32>) -> CommandBlock {
        let mut command = String::from_utf8_lossy(&std::mem::take(&mut self.command))
            .trim()
            .to_owned();
        if self.command_truncated {
            command.push('…');
        }
        let mut output = String::from_utf8_lossy(&std::mem::take(&mut self.output))
            .trim_end()
            .to_owned();
        if self.output_truncated {
            output.insert_str(0, "… earlier output omitted …\n");
        }
        CommandBlock {
            command,
            exit_code,
            output,
        }
    }

    fn reset_buffers(&mut self) {
        self.command.clear();
        self.output.clear();
        self.command_truncated = false;
        self.output_truncated = false;
    }

    fn record_text_byte(&mut self, byte: u8) {
        match self.phase {
            Phase::Idle => {}
            Phase::ReadingCommand => {
                if !is_text_byte(byte) {
                    return;
                }
                if self.command.len() < MAX_COMMAND_BYTES {
                    self.command.push(byte);
                } else {
                    self.command_truncated = true;
                }
            }
            Phase::ReadingOutput => {
                if !is_text_byte(byte) {
                    return;
                }
                self.output.push(byte);
                if self.output.len() > MAX_OUTPUT_BYTES {
                    // Keep the tail: the failing assertion is usually last.
                    let excess = self.output.len() - MAX_OUTPUT_BYTES;
                    self.output.drain(..excess);
                    self.output_truncated = true;
                }
            }
        }
    }
}

/// True for bytes worth keeping in a stored record: printable text, plus
/// newlines and tabs. Other control bytes (including carriage returns, which
/// are echo artefacts) are dropped.
fn is_text_byte(byte: u8) -> bool {
    byte == b'\n' || byte == b'\t' || byte >= 0x20 && byte != 0x7f
}

/// Extract the body of an `OSC 133 ; …` sequence, if this is one.
fn osc_133_body(sequence: &[u8]) -> Option<String> {
    if sequence.len() < 3 || sequence[0] != 0x1b || sequence[1] != b']' {
        return None;
    }
    let mut end = sequence.len();
    if sequence.last() == Some(&0x07) {
        end -= 1;
    } else if sequence.len() >= 2 && sequence[sequence.len() - 2..] == [0x1b, b'\\'] {
        end -= 2;
    }
    let body = std::str::from_utf8(&sequence[2..end]).ok()?;
    let rest = body.strip_prefix("133;")?;
    Some(rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(sequence: &str) -> Vec<u8> {
        format!("\x1b]133;{sequence}\x07").into_bytes()
    }

    fn feed(tracker: &mut CommandBlockTracker, text: &[u8]) -> Vec<CommandBlock> {
        tracker.consume(text)
    }

    #[test]
    fn a_failing_command_produces_one_block_with_its_output() {
        let mut tracker = CommandBlockTracker::new();
        assert!(feed(&mut tracker, &marks("A")).is_empty());
        assert!(feed(&mut tracker, &marks("B")).is_empty());
        assert!(feed(&mut tracker, b"cargo test\r\n").is_empty());
        assert!(feed(&mut tracker, &marks("C")).is_empty());
        assert!(feed(&mut tracker, b"assertion failed\r\n").is_empty());
        let blocks = feed(&mut tracker, &marks("D;101"));
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.command, "cargo test");
        assert_eq!(block.exit_code, Some(101));
        assert_eq!(block.output, "assertion failed");
        assert!(block.failed());
    }

    #[test]
    fn a_successful_command_is_reported_but_not_a_failure() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume(b"true");
        tracker.consume(&marks("C"));
        let blocks = tracker.consume(&marks("D;0"));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].exit_code, Some(0));
        assert!(!blocks[0].failed());

        // A `D` with no status is not a failure either.
        tracker.consume(&marks("B"));
        tracker.consume(b"ls");
        tracker.consume(&marks("C"));
        let blocks = tracker.consume(&marks("D"));
        assert_eq!(blocks[0].exit_code, None);
        assert!(!blocks[0].failed());
    }

    #[test]
    fn sequences_split_across_reads_still_parse() {
        let mut tracker = CommandBlockTracker::new();
        let all = [
            marks("B"),
            b"make".to_vec(),
            marks("C"),
            b"boom\n".to_vec(),
            marks("D;2"),
        ]
        .concat();
        // Feed one byte at a time, the worst case for a streaming parser.
        let mut blocks = Vec::new();
        for byte in &all {
            blocks.extend(tracker.consume(&[*byte]));
        }
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command, "make");
        assert_eq!(blocks[0].exit_code, Some(2));
        assert_eq!(blocks[0].output, "boom");
    }

    #[test]
    fn escape_terminated_sequences_are_equivalent_to_bel() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(b"\x1b]133;B\x1b\\");
        tracker.consume(b"cmd");
        tracker.consume(b"\x1b]133;C\x1b\\");
        tracker.consume(b"out\n");
        let blocks = tracker.consume(b"\x1b]133;D;7\x1b\\");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command, "cmd");
        assert_eq!(blocks[0].exit_code, Some(7));
        assert_eq!(blocks[0].output, "out");
    }

    #[test]
    fn display_escapes_never_reach_captured_text() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume(b"\x1b[1mcargo\x1b[0m test");
        tracker.consume(&marks("C"));
        tracker.consume(b"\x1b[31merror\x1b[0m: broke\n");
        tracker.consume(b"\x1b]0;title\x07");
        let blocks = tracker.consume(&marks("D;1"));
        assert_eq!(blocks[0].command, "cargo test");
        assert_eq!(blocks[0].output, "error: broke");
    }

    #[test]
    fn a_shell_without_osc_133_produces_nothing() {
        let mut tracker = CommandBlockTracker::new();
        let blocks = tracker.consume(b"$ cargo test\r\nassertion failed\r\n$ ");
        assert!(blocks.is_empty(), "no marks means no guessed commands");
    }

    #[test]
    fn a_new_prompt_discards_an_unfinished_command() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume(b"half typed");
        // Interrupted: the shell redraws a prompt instead of running it.
        assert!(tracker.consume(&marks("A")).is_empty());
        tracker.consume(&marks("B"));
        tracker.consume(b"real");
        tracker.consume(&marks("C"));
        let blocks = tracker.consume(&marks("D;1"));
        assert_eq!(blocks[0].command, "real");
    }

    #[test]
    fn a_finish_mark_without_a_start_is_ignored() {
        let mut tracker = CommandBlockTracker::new();
        assert!(tracker.consume(&marks("D;1")).is_empty());
    }

    #[test]
    fn oversized_command_and_output_stay_bounded() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume(&vec![b'x'; MAX_COMMAND_BYTES * 2]);
        tracker.consume(&marks("C"));
        tracker.consume(b"HEAD\n");
        tracker.consume(&vec![b'y'; MAX_OUTPUT_BYTES * 2]);
        tracker.consume(b"\nTAIL\n");
        let blocks = tracker.consume(&marks("D;1"));
        let block = &blocks[0];
        assert!(block.command.len() <= MAX_COMMAND_BYTES + 4);
        assert!(block.command.ends_with('…'));
        assert!(block.output.len() <= MAX_OUTPUT_BYTES + 64);
        assert!(block.output.ends_with("TAIL"), "the tail is kept");
        assert!(block.output.contains("earlier output omitted"));
        assert!(!block.output.contains("HEAD"), "the head is dropped first");
    }

    #[test]
    fn a_runaway_sequence_does_not_grow_memory_or_leak_into_output() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume(b"cmd");
        tracker.consume(&marks("C"));
        // An OSC that never terminates, longer than the cap.
        tracker.consume(b"\x1b]");
        tracker.consume(&vec![b'z'; MAX_SEQUENCE_BYTES * 2]);
        tracker.consume(b"\x07");
        tracker.consume(b"after\n");
        let blocks = tracker.consume(&marks("D;1"));
        assert_eq!(blocks[0].output, "after");
        assert!(tracker.pending.is_empty());
    }

    #[test]
    fn multibyte_output_survives_byte_at_a_time_delivery() {
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(&marks("B"));
        tracker.consume("é".as_bytes());
        tracker.consume(&marks("C"));
        for byte in "héllo 界".as_bytes() {
            tracker.consume(&[*byte]);
        }
        let blocks = tracker.consume(&marks("D;1"));
        assert_eq!(blocks[0].command, "é");
        assert_eq!(blocks[0].output, "héllo 界");
    }

    #[test]
    fn several_commands_in_one_read_each_produce_a_block() {
        let mut tracker = CommandBlockTracker::new();
        let stream = [
            marks("B"),
            b"first".to_vec(),
            marks("C"),
            b"one\n".to_vec(),
            marks("D;1"),
            marks("B"),
            b"second".to_vec(),
            marks("C"),
            b"two\n".to_vec(),
            marks("D;0"),
        ]
        .concat();
        let blocks = tracker.consume(&stream);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].command, "first");
        assert!(blocks[0].failed());
        assert_eq!(blocks[1].command, "second");
        assert!(!blocks[1].failed());
    }

    #[test]
    fn extra_mark_parameters_are_tolerated() {
        // Real shells append fields such as `aid=…` after the exit status.
        let mut tracker = CommandBlockTracker::new();
        tracker.consume(b"\x1b]133;B;aid=7\x07");
        tracker.consume(b"cmd");
        tracker.consume(b"\x1b]133;C;\x07");
        tracker.consume(b"x\n");
        let blocks = tracker.consume(b"\x1b]133;D;3;aid=7\x07");
        assert_eq!(blocks[0].exit_code, Some(3));
    }
}
