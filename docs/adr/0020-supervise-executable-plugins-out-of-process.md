# Supervise executable plugins out of process

Executable plugins run as owner-installed children of the durable runtime, not
as dynamic libraries inside GPUI or terminal workers. The runtime grants a
versioned, capability-filtered semantic event stream through bounded IPC and
accepts only bounded declared contributions; terminal frames, PTY bytes, Mission
mutation, and rendering are absent from the plugin interface. This costs one
process and two blocked I/O threads per running plugin, but a crash, protocol
violation, or blocked reader can be restarted or shed events without corrupting
the host or delaying terminal input and paint.

Themes remain a separate data-only desktop adapter because they need no process
authority. WebAssembly was considered for stronger portability, and in-process
Rust dynamic libraries for lower call overhead, but both are deferred: the
initial semantic event rate does not justify ABI coupling to the host, while OS
process isolation works identically on macOS and Linux and leaves room for a
future WASM adapter behind the same manifest and capability model.
