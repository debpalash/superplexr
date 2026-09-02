# Bind agent channels to PTY process groups

Agent channels are authenticated from kernel Unix-socket peer credentials and
must belong to the live PTY process group already bound to the Run. Every request
revalidates Run and Session lifecycle before applying a small allowlist. We chose
this over a bearer token because `portable-pty` deliberately closes inherited
non-stdio descriptors, environment tokens leak into process metadata, and the
runtime already creates a cross-platform session/process-group isolation seam.
This is least-privilege protocol authorization, not containment of arbitrary
same-user code; sandboxing remains a separate adapter.
