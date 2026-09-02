# Group Sessions without changing process identity

The sidebar needs one durable named row to collect several independent shells,
while the existing Session contract deliberately assigns one process, PTY,
terminal model, and history to each Session. We therefore introduce an
owner-arranged Session group with its own identity, name, order, and membership;
the user-facing sidebar may continue to say “Sessions,” but runtime and protocol
code must not pretend that several PTYs are one Session. Group changes are
runtime presentation state rather than Mission events because they organize
views without changing Run causality or process identity.

Embedding group metadata independently in every Session was rejected because
partial writes could disagree about name or order. Keeping groups only in the
desktop was rejected because remote attachments and restarted clients would not
converge. A small authoritative runtime store publishes complete group
replacements, while Session lifecycle and terminal frames remain owned by their
existing modules.
