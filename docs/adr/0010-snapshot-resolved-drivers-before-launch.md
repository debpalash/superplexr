# Snapshot resolved drivers before launch

Every agent launch records an immutable `RunDriverSnapshot` in the same durable
batch that starts the Run and binds its Session. The snapshot contains the
driver ID and profile version, SHA-256 of the complete process specification
before runtime identity/channel injection, argument count, and sorted
environment key names. It deliberately excludes argv and environment values;
those remain in the owner-only terminal launch specification.

When an OS adapter wraps the resolved process, the snapshot also carries the
bounded sandbox backend/profile and an explicit network-isolation boolean. These
fields come from the adapter result, not driver-authored display metadata. A
backend without its profile (or vice versa) is invalid durable history.

This gives Mission history a stable audit identity for both direct commands and
configured engines without turning the graph into a secret-bearing command log.
Resolution is allowed only while the Run is pending and only once. Failed
resolution commits nothing. A materially different retry must be a new Run,
preserving the existing immutable-attempt model.
