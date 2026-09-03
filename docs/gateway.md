# The network gateway

The runtime listens on a Unix socket and trusts the peer's uid. That is the
whole of its local security model, and it is a good one. The gateway is how a
device that is not on this machine — a phone, a laptop across the network, a
TUI over a hostile hotel Wi-Fi — reaches the same runtime without weakening
that model.

## Rules

1. **Off unless asked.** `--gateway host:port` on the runtime (or the desktop,
   which passes it through). Nothing listens on the network otherwise.
2. **Nothing unpaired gets in.** A connection must present a device token or
   a pairing code inside the handshake, or it is closed with
   `gateway_unauthorized` before a single request is read. There is no
   read-only default, no "just list", no unauthenticated anything.
3. **The runtime keeps no secrets it does not need.** A device token is
   returned once, at pairing; the runtime stores its SHA-256 digest. The
   pairing code is likewise stored only as a digest, lives five minutes, and
   is single use. Pairings do not survive a restart.
4. **Identity is a fingerprint, not a CA.** The runtime's certificate is
   self-signed and made once. A device pins `sha256:<hex>` of it, shown beside
   the pairing code. A certificate that does not match is refused by the
   device before any bytes of ours are sent.
5. **Revocation is immediate.** `device-revoke` ends the device's open
   connections at their next request and refuses new ones.
6. **Devices are the owner.** A paired device has the owner's authority — it
   is the owner's device. Scoped, time-bounded access for someone else uses a
   share token over the same gateway, so authorization stays one code path.

## Pairing

On the runtime's host:

```
superplexr device-pair --label phone
{ "code": "K7PM2XQ4", "fingerprint": "sha256:…", "gateway": "0.0.0.0:7373", … }
```

On the device, within five minutes:

```
superplexr pair --gateway host:7373 --fingerprint sha256:… K7PM2XQ4
```

The token lands in `~/.superplexr/devices/host_7373.json` (owner-only). From
then on any command reaches the runtime with `--gateway host:7373`, and
`superplexr attach --gateway host:7373 <session>` is the TUI shell over TLS.
`superplexr forget --gateway host:7373` drops the stored token on the
device; the runtime keeps the device listed until `device-revoke`.

## The browser

The same port serves the web shell. The listener reads the first bytes of a
TLS connection: the wire's magic means a native shell, an HTTP request line
means a browser. The page and its scripts are embedded in the runtime
(`web/`, no build step, no external resources, strict CSP), and
`wss://host:7373/ws` carries wire_v3 frames unchanged inside WebSocket
binary messages — the browser accumulates bytes and reads frames out of the
stream, so message boundaries carry no meaning.

Admission is the same. The page sends a Hello with a pairing code the first
time and keeps the token it gets back in the browser's storage; an unpaired
or revoked browser is closed at the handshake like any other device. A
WebSocket whose `Origin` is not this host is refused before the upgrade, so
a page from elsewhere cannot borrow a browser's reach.

A browser cannot pin a fingerprint, so it meets the self-signed certificate
once and the person accepts it, comparing the fingerprint their browser
shows with the one `device-pair` printed. That acceptance is the browser's
pin; a different certificate at that address is a warning, not a silent
success. Real certificates arrive with the cloud phase.

`ci/web-smoke.sh` walks the whole path from node: the page and its CSP, an
unpaired browser refused, pairing, a shell started from the page,
subscribing, typing and reading the echo out of protobuf frames, and an
off-host origin refused.

## What it is not

- Not a relay or a cloud: the device connects to the runtime directly. NAT
  traversal, rendezvous and hosted runtimes are the cloud phase.
- Not rate-limited per connection yet. The per-subscriber frame rate
  (`max_hz`) bounds the expensive stream; request-level limits are open.
- Not reviewed. Phase 3's gate is an outside review of pairing and token
  scope before any public exposure.
