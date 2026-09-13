# Security policy

## Supported versions

SuperPlexr is currently pre-release. Security fixes are applied to the latest
commit on `main`; older commits and local development snapshots are not supported
as separate release lines.

## Reporting a vulnerability

Please report vulnerabilities privately through the repository's **Security →
Report a vulnerability** flow on GitHub. Include the affected component, impact,
reproduction steps, and any suggested mitigation. Do not open a public issue for
an unpatched vulnerability.

You should receive an acknowledgment within seven days. We will validate the
report, coordinate a fix and disclosure timeline, and credit reporters who want
to be named.

## Scope reminders

The default runtime is local-first, but SuperPlexr controls persistent processes,
terminal input, scoped Shares, remote attachments, and agent launch boundaries.
Reports involving authentication bypass, scope expansion, secret disclosure,
unsafe path handling, terminal-control theft, or sandbox escape are especially
useful.

See [docs/spec/07-security-and-reliability.md](docs/spec/07-security-and-reliability.md)
for the documented trust model and current security claims.
