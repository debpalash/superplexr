# Development distribution profiles

These profiles select executables, not a second runtime or security model.
The owner's 2026-09-06 update authorizes testing. Eight Python regressions now
pass for profile plans, output refusal, Cargo-artifact admission, and inherited
metadata. Actual profile builds and package acceptance remain separate pending
steps. Bundles are not certified releases.

| Profile | Executables | Intended use |
| --- | --- | --- |
| `host` | CLI, server | Local/headless execution host; no GUI or HTTP gateway executable. |
| `terminal` | CLI, TUI | Attach to an existing runtime; no bundled server or GPUI desktop. |
| `web` | CLI, observer gateway | Browser access through the existing scoped localhost gateway. No public TLS service implied. |
| `desktop` | CLI, server, desktop | Native client with convenient local-host startup. |
| `automation` | CLI, server, MCP, agent-status plugin | Host plus opt-in agent integrations. Included integrations do not start automatically. |
| `all` | All of the above, plus hosted daemon | Development installation with all eight executables. |

The profiles select Cargo packages explicitly, so terminal/host/web builds do not
select the desktop package. They do not yet feature-gate all libraries inside
the selected packages: CLI retains verification/plugin dependencies, and shared
terminal types retain their native VT dependency. `host` is not a claimed minimum
possible binary. Remote owner attachment continues to use separately installed
OpenSSH; no new transport, listener or credential is configured by packaging.

## Build when ready

Requires Python 3.11+, Cargo/Rust, the repository's native dependencies and a
populated dependency cache. Builds are release, locked and offline by default.
No tests, benchmarks, application launch, installation or signing are performed.

```sh
python3 ci/build-profile.py terminal --output dist/terminal-dev --plan
python3 ci/build-profile.py terminal --output dist/terminal-dev
python3 ci/build-profile.py host --output dist/host-dev
```

`--plan` prints the command without executing Cargo or writing files. Explicit
`--online` permits dependency downloads. `--target TRIPLE` selects a Cargo target
but requires that target's actual toolchain/linker and does not establish platform
support. Existing output paths are refused. Failed copies leave a partial directory
without `bundle.json`; the script never recursively removes it or overwrites an
existing bundle. Use a new output path when retrying.

Cargo must successfully emit every selected executable artifact in that invocation
(including artifacts Cargo explicitly reports as fresh). The builder consumes those
paths, honoring custom Cargo target directories, instead of copying potentially
stale guessed paths. `bundle.json` records profile, version, target, compiler,
source revision/dirty state, lock digest, command, selected feature sets and each
executable's size/SHA-256. These are artifact measurements, not runtime footprint
or latency measurements. Dirty source is recorded, not claimed reproducible.

Each profile also generates `build-inputs.json`, `superplexr.spdx.json` and
`THIRD_PARTY_NOTICES.txt` from the package artifacts observed in that Cargo
invocation. These include build scripts/procedural macros, not unselected workspace
packages. Cached artifacts count when Cargo reports them for the current build.
The generator reads their local manifests, resolving inherited version/license
fields and collecting package/workspace license texts and explicit license-file
paths within their source roots. It does not run another full-workspace dependency
resolution merely to describe a headless build.

The SPDX document describes selected executable packages and contains the observed
build inputs. It deliberately does not invent dependency edges or call this a
runtime-only graph. Native libraries produced inside build scripts, system
libraries and additional distribution obligations still need separate inventory
and review. Metadata-file hashes and byte sizes are included in `bundle.json`.
This generator path is implemented but has not been executed or validated.

## Use and limitations

Executables live in `bin/`. Client-only profiles require a compatible existing
runtime and explicit socket/Share configuration. The observer remains localhost
and scoped according to its existing options. For inspection-only agent access,
start the MCP bridge with `--read-only`. Inclusion does not grant a Share or start
a plugin. Fault-capable MCP mode can replay commands and is not a sandbox.

The bundle includes the project license and terminfo **source**, not a compiled
terminfo database or all system libraries. Install terminfo with the target
system's `tic` if required. Native desktop app bundles, AppImage/.deb integration,
notice/SBOM completeness and validation, signing/notarization, relocatability,
reproducibility and platform/resource acceptance remain release work. Do not
redistribute these folders as completed release artifacts based on the manifest
alone. The existing packaging smoke workflow remains separate.
