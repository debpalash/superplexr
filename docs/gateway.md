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

## What it is not

- Not a relay or a cloud: the device connects to the runtime directly. NAT
  traversal, rendezvous and hosted runtimes are the cloud phase.
- Not rate-limited per connection yet. The per-subscriber frame rate
  (`max_hz`) bounds the expensive stream; request-level limits are open.
- Not reviewed. Phase 3's gate is an outside review of pairing and token
  scope before any public exposure.
