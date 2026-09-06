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
ultraplexr device-pair --label phone
{ "code": "K7PM2XQ4", "fingerprint": "sha256:…", "gateway": "0.0.0.0:7373", … }
```

On the device, within five minutes:

```
ultraplexr pair --gateway host:7373 --fingerprint sha256:… K7PM2XQ4
```

The token lands in `~/.ultraplexr/devices/host_7373.json` (owner-only). From
then on any command reaches the runtime with `--gateway host:7373`, and
`ultraplexr attach --gateway host:7373 <session>` is the TUI shell over TLS.
`ultraplexr forget --gateway host:7373` drops the stored token on the
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

## Streams

A stream is a Share with a link. `ultraplexr stream <session>` mints an
Observer Share scoped to that one session (`--controller` for one that may
claim control when nobody holds it) and prints
`https://host:7373/#share=<token>`; the token lives after the hash, so it
never reaches a server log, and the page never stores it.

A viewer who opens the link is admitted at the handshake by the Share
token instead of a device token, and the connection is then that Share and
nothing else: every request must carry the same token or it is refused as
`share_token_required` before it can be read with the owner's authority.
The Share's own rules apply on top — an Observer may snapshot, subscribe,
list, search and ask who is watching; a Controller may also type and claim;
neither may reach devices, other sessions, or the owner's requests.
Revoking the Share (`share-revoke`) ends its viewers at their next request
and their live subscriptions at once.

Presence is exact: each subscription registers itself on its terminal for
as long as its task lives, so `terminal-viewers <session>` and the page's
`👁` count show who is watching now, owner screens and links alike.

`ci/stream-smoke.sh` walks it: a link minted on the host, a viewer admitted
and painted, its limits (no keys, no token-less request, no owner request),
presence seen by the owner, fifty viewers on one scrolling session with
bytes per viewer measured, revocation ending the viewer.

## Rooms

A session with several participants — the owner's screens and viewers on
links — is a room, and control in a room is handed, never seized. The
moves are exactly these: a participant who may hold control raises a hand
(`request_terminal_control`); the holder offers control to one hand or to
anyone who asked (`offer_terminal_control`); the one it was offered to
accepts (`accept_terminal_control`); either side withdraws
(`withdraw_terminal_control`). Only the owner may still take by force, as
before. Observer links can do none of it.

An offer is bound to the control epoch it was made in, so it cannot
outlive the control it was about: any claim, release, accept or exit
clears it. Accepting is the only transfer without force and is refused as
"not offered to this participant" for anyone the offer did not name.

Identity is a participant — client, surface, Share — with a label people
can read; `terminal-viewers` shows one per viewer so an offer can name the
screen that asked. Hands and offers ride the session summary on the
terminal index stream, so every screen sees the room change as it
happens. The browser's control panel shows only the moves open to this
participant now; the CLI has `terminal-control-request`, `-offer`,
`-accept` and `-withdraw`.

`ci/rooms-smoke.sh` walks it: an owner screen and a pair-programmer link,
a hand raised, an observer refused, an accept before any offer refused,
control handed to the hand and back again with the owner's keys refused in
between, and one command typed by two participants across two hand-offs
arriving as one line.

## What it is not

- Not a relay or a cloud: the device connects to the runtime directly. NAT
  traversal, rendezvous and hosted runtimes are the cloud phase.
- Not rate-limited per connection yet. The per-subscriber frame rate
  (`max_hz`) bounds the expensive stream; request-level limits are open.
- Not reviewed. Phase 3's gate is an outside review of pairing and token
  scope before any public exposure.
