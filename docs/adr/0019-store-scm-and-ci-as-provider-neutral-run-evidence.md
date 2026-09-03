# Store SCM and CI as provider-neutral Run evidence

SCM reviews and CI checks are durable evidence about a Run, but they are not the
Run's lifecycle or a human's Mission decision. superplexr records them in a
bounded owner-only runtime store keyed by Run, provider, and provider-scoped
evidence key. A newer observation replaces the same key atomically while the
daemon authors its timestamp and authenticated source.

The protocol uses provider-neutral `Check` and `ChangeRequest` evidence kinds.
Adapters may represent GitHub, GitLab, a local test runner, or a future system
without introducing provider-specific domain types. Each observation carries a
revision so a passing check for an old commit cannot be mistaken for evidence
about the current checkout.

Evidence never implicitly succeeds or fails a Run, accepts a result, raises or
resolves a Signal, or retires a checkout. Failed checks and requested changes
may be projected into attention UI, but an explicit owner or agent action must
perform any Mission mutation. This keeps external integrations useful without
letting delayed webhooks rewrite authoritative graph history.

The store is separate from Mission history because external systems can update
frequently and replace state. It has strict text/count/file bounds, atomic
replacement, directory fsync, owner-only permissions, symlink rejection, and
server-authored provenance. Agent-channel clients may report and read only their
own Run; owner clients may inspect any Run.
