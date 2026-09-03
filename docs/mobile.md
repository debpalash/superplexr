# Mobile

The plan's phase 6 said: wrap the web shell for phones first, native only
where it pays. On this machine there is no Xcode and no Apple developer
account, so the wrapper here is the one every phone already ships — the
browser's own "add to home screen" — and the parts that make a phone
useful are built into the web shell itself.

## Installed to the home screen

The shell is a progressive web app: a manifest, an icon, and a service
worker that keeps the shell's files for offline starts and shows
notifications when no tab is open. On iOS 16.4+ and Android, open the
gateway's address, accept its certificate once, and add it to the home
screen; it launches full-screen, signed in as the device it paired as.

## Told when something needs you

The runtime speaks Web Push (RFC 8291 encryption and RFC 8292 VAPID, on
ring; delivery by the system's `curl`). Three things are worth a phone's
attention and nothing else is sent: a Fault opened, an approval waiting
(once per approval), a Run finished. A tap on the notification opens the
shell on the thing itself.

`🔔 notify` in the page asks the browser's permission and hands the
subscription to the runtime, which keeps it under `push/` in the state
dir, owner-only, beside the application server key it made once. A push
service that answers 404 or 410 has dropped the subscription and it is
forgotten. `superplexr push-test` sends one to prove the path. Viewer links
cannot subscribe; notifications are the owner's.

`ci/push-smoke.sh` proves the cryptography against an independent
implementation: node plays the push service (TLS, VAPID signature
verification, RFC 8291 decryption with the browser's private key) and the
browser (a P-256 key pair and an auth secret); the runtime encrypts, signs
and delivers with its own code, and what node decrypts must be what was
sent, with the envelope's every field checked.

## A phone's keyboard

A canvas cannot summon a soft keyboard, so on touch screens a hidden
input takes the keys, including what the keyboard composes on its own
(autocorrect, swipe, emoji), and a key bar supplies what phones lack: Esc,
Tab, Ctrl and Alt as one-shot modifiers, arrows, ^C, ^D, `-`, `/`, `|`.
The grid refits when the keyboard opens or closes.

## What needs an Apple developer account

A store listing, a WKWebView wrapper with APNs push (Web Push works in
Safari without it once installed), background reconnect beyond what the
browser allows, and share sheets. None of that changes the engine or the
shell; it is packaging, and it is the owner's call when to pay for it.
