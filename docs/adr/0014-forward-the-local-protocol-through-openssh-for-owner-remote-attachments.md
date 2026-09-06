# Forward the local protocol through OpenSSH for owner remote attachments

The first remote-device transport forwards a new owner-only local Unix socket to
the authoritative runtime's existing Unix socket through OpenSSH StreamLocal
forwarding. The runtime does not listen on TCP, duplicate state, or learn SSH
credentials. OpenSSH supplies host authentication, user authentication,
encryption, jump hosts, key agents, keepalives, and transport rekeying.

This preserves the deep local protocol interface: control requests, independent
subscriptions, frame repair, Mission history, and Control epochs cross the same
socket seam without remote branches in domain or PTY modules. The remote sshd
process connects as the owning OS user, so the runtime's kernel peer-UID check
still applies. The forwarded local socket is created with umask `0177` in a real
owner-only directory, and ultraplexr never enables unlinking an arbitrary
pre-existing path.

The tunnel supervisor owns one exact absent socket path, removes only an
owner-owned socket it created, and can reconnect with bounded backoff. The native
desktop has an explicit connect-only mode so a missing tunnel can never cause it
to spawn a different local runtime at the forwarding path.

This decision covers an owner's remote attachment. It does not implement a
Share: another human must eventually receive a separate revocable identity and
method-level authorization instead of an SSH login to the owner's account.
