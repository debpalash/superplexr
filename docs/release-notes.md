# SuperPlexr — GPUI experiment

Experimental native agent multiplexer built with Rust and GPUI.

Each `.tar.gz` contains all eight executables in `bin/`: `superplexr` (CLI),
`superplexr-server`, `superplexr-daemon`, `superplexr-desktop`, `superplexr-tui`,
`superplexr-observer`, `superplexr-mcp`, and `superplexr-agent-status-plugin`.
Keep the binaries together so clients can find their local runtime.
The agent-status plugin is launched by the runtime; it is not an interactive CLI.

| Target | System |
| --- | --- |
| `aarch64-apple-darwin` | macOS 13+, Apple Silicon |
| `x86_64-apple-darwin` | macOS 13+, Intel |
| `aarch64-unknown-linux-gnu` | Linux ARM64, Ubuntu 24.04 or compatible |
| `x86_64-unknown-linux-gnu` | Linux Intel/AMD, Ubuntu 24.04 or compatible |

Windows native binaries are not available: the runtime currently requires Unix
APIs. WSL2 users can try the Linux build; the desktop also needs WSLg. WSL is not
part of the release validation matrix. Android, iOS, BSD, and musl are not supported.

Extract the archive and run `bin/superplexr-desktop`, or add `bin/` to your PATH
for the CLI and TUI. The archive includes a compiled terminfo database; set
`TERMINFO` to the extracted `share/terminfo` directory if your system does not
already provide `xterm-ghostty`.

On macOS, the optional `.app.zip` contains a desktop application with all helper
binaries. Move `SuperPlexr.app` to Applications. It is ad-hoc signed, not Developer
ID signed or notarized; macOS may require approval in Privacy & Security.

Linux builds link system libraries dynamically. On Ubuntu 24.04, install:

```sh
sudo apt-get install libfontconfig1 libglib2.0-0t64 libssl3t64 libvulkan1 \
  libwayland-client0 libwayland-cursor0 libwayland-egl1 libx11-xcb1 \
  libxkbcommon0 libxkbcommon-x11-0 libxcb1 libxcb-shape0 libxcb-xfixes0
```

The desktop also needs a working X11/Wayland session and Vulkan driver. These
archives are not static binaries, AppImages, or OS installers.

`SHA256SUMS` covers every downloadable archive. Verify with `sha256sum -c
SHA256SUMS` on Linux or `shasum -a 256 -c SHA256SUMS` on macOS after downloading
the listed files. Each bundle includes per-file checksums, source revision,
the project license, third-party notices, and an SPDX inventory of observed
Cargo build inputs. This inventory does not include all system libraries.

The release action builds all targets and checks CLI startup and bundle contents
before publishing a prerelease. These checks are not full interactive desktop
qualification. This is an experiment with rough edges, not a production release.

Maintainers: run the `release` workflow from `main`, or push `vVERSION` matching
`workspace.package.version` in `Cargo.toml`. Publication waits for all four native
builds. The action assembles a draft and publishes it only after every upload
succeeds. Published versions are never overwritten; bump the version for a new
release. Account billing restrictions must be resolved before runners can start.
