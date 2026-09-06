# A daemon that never sleeps

Phase 7 of the platform plan: host the same runtime somewhere that stays
on, so streams, rooms and notifications stop depending on a laptop lid.
Nothing in the engine changes. The gateway already speaks to it.

## The binary

`ultraplexr-daemon` is the runtime alone — no desktop linked in — with
three flags: `--socket`, `--state-dir`, `--gateway`. It is what the
desktop starts for itself, packaged for a host with no screen.

```
ultraplexr-daemon --socket /tmp/ultraplexr/control.sock \
    --state-dir /var/lib/ultraplexr --gateway 0.0.0.0:7373
```

## Hosting it

- **Linux, systemd:** `packaging/linux/ultraplexr-daemon.service` runs it
  as its own user with a private state directory, restarts it, and locks
  the filesystem down. Pair from the host as that user:
  `ultraplexr --socket /run/ultraplexr/control.sock device-pair --label laptop`.
- **A Mac that stays on:** `packaging/macos/com.ultraplexr.daemon.plist`
  for launchd.
- **A container:** `ci/daemon.Dockerfile` builds the daemon and the CLI
  with the same toolchain CI uses and runs them as an unprivileged user
  with the state on a volume. `docker exec` into it to pair.

Then, from anywhere that can reach port 7373: `ultraplexr pair`, and the
TUI (`attach --gateway`), the web shell (`https://host:7373/`) and a phone
with the shell installed all work as they do against a laptop. Viewer
links (`ultraplexr stream`) carry the host's address.

## What the host must know

- The gateway's certificate is self-signed and pinned by fingerprint on
  every native device; browsers accept it once. A real certificate for a
  real name is the remaining piece for public streams, and it slots in
  where `gateway/identity.crt` lives today.
- Put the port behind a firewall or a VPN until the outside security
  review of pairing and token scope, listed since phase 3, has happened.
  Nothing unpaired gets in, but that claim has not yet been tested by
  someone trying to break it.
- Push notifications leave the host through the system's `curl` to the
  browsers' push services; the host needs outbound HTTPS and nothing else.
- The state directory holds journals, Faults, device and Share digests
  and push subscriptions. Back it up as one unit; it is the whole runtime.

## The gate, as met here

`ci/daemon-smoke.sh` starts the headless daemon with the gateway on and,
with no desktop anywhere, pairs a laptop, starts a session on the host,
attaches the TUI under a pty, runs the browser smoke against it, and
mints a viewer link. The phone is the same web shell; it joins through
the link or its own pairing.

Not built here: the multi-machine runner fleet the roadmap lists as P2
(control plane, execution plane and data plane kept separate), and a
rendezvous or relay for hosts behind NAT. Both are packaging and
infrastructure around an engine that does not change.
