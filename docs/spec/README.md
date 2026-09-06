# ultraplexr v1 product and engineering specification

Status: implementation contract  
Version: 1.0  
Last revised: 2026-09-01  
Targets: macOS and Linux

This directory is the normative specification for ultraplexr v1. It turns the
product thesis and accepted architecture decisions into contracts that can be
implemented and tested independently. When this specification conflicts with a
design exploration, README prose, or an older prototype, this specification
wins. Accepted ADRs still govern *why* a hard-to-reverse choice was made.

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are
requirements. A requirement can change only through a reviewed specification
change. Every normative heading has a stable identifier; individual clauses are
cited as that identifier plus their bullet or paragraph number when necessary.

## Product in one paragraph

ultraplexr is a local-first, agent-native terminal multiplexer for developers who
supervise parallel human and agent work. A browser-style Mission tab contains a
Session sidebar and a responsive waterfall of terminal Surfaces. Durable work is
represented as an event-sourced graph of Runs, dependencies, Signals, Artifacts,
and Interventions. A background runtime owns PTYs and canonical terminal state,
so closing the desktop destroys no work. Humans can see why attention is needed,
take terminal control without contention, return it to an agent, and audit what
happened afterward.

## Document map

1. [Product](01-product.md) — audience, jobs, principles, v1 scope, and success.
2. [Domain and execution graph](02-domain-and-graph.md) — entities, graph rules,
   state machines, commands, events, and invariants.
3. [System architecture](03-system-architecture.md) — process model, deep
   modules, interfaces, threading, storage, and platform contract.
4. [Terminal and Session contract](04-terminal-and-sessions.md) — PTY semantics,
   Ghostty seam, frames, input, resize, control, detach, and history.
5. [Local protocol](05-local-protocol.md) — transport, framing, messages,
   sequencing, errors, compatibility, and agent capabilities.
6. [Desktop experience](06-desktop-experience.md) — information architecture,
   interaction, responsive layout, accessibility, and visual system.
7. [Security and reliability](07-security-and-reliability.md) — threat model,
   authorization, persistence, recovery, limits, privacy, and observability.
8. [Quality and release](08-quality-and-release.md) — performance budgets, test
   matrix, release criteria, packaging, and traceability.
9. [Delivery plan](09-delivery-plan.md) — vertical milestones, dependencies,
   risks, and exit gates.
10. [Dependency baseline](10-dependency-baseline.md) — upstream facts, pinning,
    audits, and fallback triggers.
11. [Control and event catalog](11-control-and-event-catalog.md) — exact control
    records, methods, commands, events, and error mapping.

Supporting material:

- [`docs/ultraplexr-unified-roadmap.md`](../ultraplexr-unified-roadmap.md) is the
  consolidated status and planning view across research, implementation, and
  the remaining v1 delivery gates.
- [`proto/ultraplexr/terminal/v1.proto`](../../proto/ultraplexr/terminal/v1.proto)
  is the normative terminal data-plane schema.
- [`CONTEXT.md`](../../CONTEXT.md) is the canonical glossary and contains no
  implementation policy.
- [`docs/adr`](../adr) records accepted architectural trade-offs.
- [`docs/product/north-star.md`](../product/north-star.md) records the product
  thesis and competitive evidence bar.
- [`docs/design/browser-waterfall-v1.md`](../design/browser-waterfall-v1.md) is
  the original detailed design resolution. Section 06 is normative where their
  wording differs.

## Conformance

A v1 build conforms only when all release requirements in section 08 pass on
both supported operating systems. A partially implemented build MUST identify
itself as `dev`, `prototype`, or `alpha`; it MUST NOT silently weaken a MUST.

Specifications describe the intended v1 even when release evidence has not
caught up. The current implementation advertises preview protocol version 25 and
includes the Mission graph, durable PTYs, native terminal projection, scheduler,
agent channel, owner Remote attachment, scoped Observer/Controller Shares, and
durable Run checkouts. It also exposes structured terminal captures and bounded,
event-driven text/quiet/exit waits to owners, scoped Shares, and the owning agent;
NDJSON push events; and expiring provider facts beneath an explainable Run
Activity projection.
It also includes the first out-of-process executable-plugin slice: strict local
discovery, capability-filtered semantic events, bounded non-blocking delivery,
health listing, restart isolation, and a first-party Agent Status proof plugin.
The negotiated header, protobuf terminal payloads, multiplexing, and zstd
profile in section 05 are implemented. The preview version counter must not be
confused with that wire-major number.

## Change discipline

Every specification change MUST:

- preserve the language in `CONTEXT.md` or update that glossary first;
- state affected requirement identifiers;
- state persistence and protocol compatibility impact;
- add or update an acceptance test;
- create an ADR only when the choice is hard to reverse, surprising without
  context, and the result of a genuine trade-off.

There are no intentionally unresolved product semantics in this pack. Upstream
feasibility risks are handled with explicit spike gates and fallbacks rather
than hidden TODOs.
