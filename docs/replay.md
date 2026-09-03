# Replay

Every session is journaled already — that is what recovers a crashed
daemon and answers scrollback. Replay plays that journal back as a
terminal, at the session's own pace or faster, from any chapter.

## What is recorded

Beside each session's `output.raw` the runtime keeps two small files:

- `timing.bin` — one twelve-byte entry per PTY read: the journal offset
  after it and milliseconds since the session started. This is what lets
  a replay keep the session's own rhythm. When the journal is compacted the
  offsets move, so the timing file starts over; bytes before that point
  play at a steady rate instead.
- `events.jsonl` — one line per event with the journal offset at that
  moment: the session started, control changed hands (and to whom),
  a Fault was recorded, the session exited. These are the chapters.

Nothing else is stored; a replay is the journal and these two files.

## Playing

`superplexr chapters <session>` lists the chapters with their offsets.
`superplexr replay <session> [--speed 4] [--from-chapter N | --from-offset B]`
plays into the current terminal: frames only, no keys go back, `q` or
Ctrl-] stops, the end of the recording waits for a key. It works over
`--gateway` like everything else.

In the browser, ▶ replay on any open session shows the chapters, a speed
and a progress bar; a chapter starts the replay there, ■ live returns to
the running session, Esc stops.

On the wire, `subscribe_terminal_replay { session_id, from_offset,
speed_percent, max_hz }` opens a stream that carries the same full frames
and deltas a live subscription does, then closes when the recording ends.
Everything before `from_offset` is fast-forwarded within the history
budget; after it the recording is paced by `timing.bin`, divided by the
speed, with any gap longer than five seconds shortened to five so a
recording with a long idle stretch stays watchable. A Share may replay
what it may watch.

`ci/replay-smoke.sh` records a session with three beats of output and a
control change between them, checks the chapters and the recorded
duration, replays at ×4 and checks the screen and the time it took,
replays from the control chapter and checks it starts past the first
beat, and runs the TUI player under a pty.
