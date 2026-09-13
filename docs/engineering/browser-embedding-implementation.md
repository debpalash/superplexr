# Browser embedding implementation

Status: source implemented; unbuilt and untested. Tests, builds, browser checks,
and security acceptance are deferred at the owner's request. This does not
establish general embedded-platform support or production readiness.

## Configuration

The optional browser gateway accepts repeated `--embed-origin` arguments:

```sh
superplexr-observer --socket /absolute/path/control.sock \
  --share-token-file observer.token --port 7800 \
  --embed-origin http://localhost:3000
```

This is a usage example, not a command run during implementation. The origin is
the **parent application's** origin, not the gateway's address. Give an origin
without a trailing slash, path, query, or fragment. HTTPS origins are supported
by the policy; HTTP parents must use `localhost` or a loopback IP. Up to sixteen
origins of at most 256 ASCII bytes each are accepted. Use ASCII punycode for
internationalized DNS names. Wildcards, CSP keywords, credentials, opaque
origins, ambiguous numeric hosts, and invalid ports are rejected. Host casing,
IP literals, and default ports are normalized; duplicate entries are collapsed.

Without this option, responses keep `frame-ancestors 'none'` and also send
`X-Frame-Options: DENY`. Enabling it replaces only the framing directive with
the configured origins and omits X-Frame-Options, which cannot express the list.

The embedding application can set an iframe's `src` to the gateway URL printed
at startup, including its `#access=...` fragment. Treat that URL as a credential:
never commit it, put it in analytics or logs, or publish it in a shared page.
The parent that receives the URL knows the browser key; it must be trusted.
The gateway's Share token is not sent to the browser. The current UI consumes
and clears the fragment, holds the key only in memory, and does not persist it.
Reloading the frame requires supplying the private URL again.

For a sandboxed cross-origin iframe, the UI needs `allow-scripts` and
`allow-same-origin` so its existing authenticated same-origin requests work.
Do not treat these permissions or an iframe sandbox as a boundary against a
trusted parent that already holds the credential. No top-navigation, popup,
download, or parent-message permissions are needed by this implementation.
Clipboard delegation is a separate browser permission and is not granted here.

## Authority boundaries

- Embedding does not widen the listener: the executable still binds only
  `127.0.0.1`, with no public-listen or reverse-proxy configuration.
- Only a GET of `/` with iframe-navigation fetch metadata receives the
  cross-site navigation exception. CSP evaluates the complete ancestor chain.
  That public shell contains no Session output or credentials.
- Assets and all API routes retain the existing Host, Origin, and fetch-site
  checks. There are no CORS headers, parent-to-frame command handlers, generic
  RPC forwarding, or cookie-based authority added by this feature.
- API reads still need the browser bearer key. POST requests still require the
  gateway's exact Origin, and native operations retain the original Share.
- Default embedding is read-only. A Controller gateway requires **both** the
  existing `--allow-control` and the additional `--allow-embedded-control`, plus
  a Controller Share. Both CLI and library enforce the embedding acknowledgement.
  Every allowed parent must be trusted not to overlay or disguise terminal-input
  controls. This acknowledgement does not claim to prevent clickjacking by a
  trusted-but-compromised parent. The user still explicitly claims Control.
- Configuration errors are rejected before the CLI reads the Share token or
  connects to the runtime. The library exposes a validated `EmbeddingPolicy`
  consumed by `Observer::with_embedding`; no request can change this policy.

## Limits and deferred acceptance

This is a framing policy for the existing browser client, not a web component,
remote hosting service, embedding SDK, or guarantee that every browser allows a
remote HTTPS page to frame local HTTP. Mixed-content and local-network access
rules, sandbox behavior, webview policies, and clipboard permissions can block
an otherwise allowed frame. No browser restrictions have been bypassed. Do not
publish the HTTP gateway to work around them. Authenticated remote exposure,
TLS termination, multi-tenant isolation, and credential provisioning need their
own design and implementation.

Before enabling this outside development, deferred acceptance must cover
origin-parser rejection cases and normalization, standalone anti-framing,
allowed and denied parents including nested ancestors, actual cross-origin
iframe startup/assets/authenticated streams, unchanged cross-origin API and
write rejection, Controller opt-in and lease behavior, token non-disclosure,
revocation, and browser/platform local-network restrictions. Existing browser
evidence predates these source changes and cannot certify them.
