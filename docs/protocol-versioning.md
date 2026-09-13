# Protocol versioning

The daemon and every client speak `superplexr-protocol`. `PROTOCOL_VERSION`
is a single integer, and it is the whole policy: two sides with different
numbers do not talk. There are no minor versions and no feature flags on the
wire, because a client that half-understands a frame renders it wrong, and a
wrong screen is worse than a refused connection.

## What bumps the version

Anything that changes what bytes mean once they have left the daemon:

- a field added, removed or retyped in a request, response or event;
- a change to the terminal data plane (`proto/superplexr/terminal/v1.proto`
  and the checked-in `terminal_proto.rs`), including a new field that an old
  decoder would silently ignore;
- a change to how a delta is applied, even if the encoding is unchanged.

Adding a request or response variant that old clients never send or receive
still bumps the version: an old daemon answers an unknown request with an
error, and "works until you press that button" is not compatibility.

## What does not

- Behaviour behind the same bytes: how fast the daemon publishes, what it
  logs, what it refuses on authorization grounds.
- New optional fields on the *client* side that default to absent, when the
  daemon already tolerates their absence (`#[serde(default)]`). These are
  compatible in one direction only and must be documented as such.

## What a bump costs, and what it must not cost

The state directory is named by version (`.superplexr-dev/v26`,
`.superplexr/v26`) so an old runtime can run beside a new one. A bump must
never cost a person their sessions, Faults, workspaces or shares: on first
start into a new version's directory, the daemon copies the previous
version's state forward (`carry_forward_state`), leaving the old directory
intact. Sockets and logs are not state and are not copied.

Live PTYs do not survive a daemon restart, whatever the version; a bump is a
restart, and it is announced as one.

## Clients

The desktop, the CLI and the MCP bridge ship with the daemon and always match
it. Anything that does not — a future TUI over SSH, a web or mobile shell
through the gateway — checks the version in the handshake and reports the
mismatch in plain words with both numbers, then stops. It never guesses.

## History

- **26** (2026-09-04): delta rows carry only the changed span of cells;
  subscribers may name a maximum frame rate.
- **25**: the last version before the rename from termi9ne.
