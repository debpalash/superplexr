# Contributing to SuperPlexr

Thanks for helping make parallel agent work easier to understand and control.

## Before you start

Read [CONTEXT.md](CONTEXT.md) before changing domain concepts or product copy.
SuperPlexr distinguishes Missions, Runs, Sessions, and Surfaces deliberately;
using those terms consistently keeps the runtime, clients, and documentation
aligned.

For a larger behavior or architecture change, open an issue first and connect the
proposal to the relevant requirement in [docs/spec](docs/spec/README.md). Small
fixes and focused documentation improvements can go directly to a pull request.

## Development setup

You need Rust 1.97.1 and Zig 0.16.0. The repository pins its Rust toolchain.

```sh
git clone https://github.com/debpalash/superplexr.git
cd superplexr
cargo build --workspace --locked
```

Run the native desktop with:

```sh
cargo run -p superplexr-desktop
```

## Validation

Run the checks relevant to your change. Before requesting review, the normal
workspace gate is:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
node --test crates/superplexr-observer/web/*_tests.mjs \
  ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

Platform, packaging, performance, and soak checks are documented in
[docs/spec/08-quality-and-release.md](docs/spec/08-quality-and-release.md).

## Pull requests

Keep each pull request focused and explain:

- the concrete problem and resulting behavior;
- the affected trust, process-lifetime, or protocol boundaries;
- the checks you ran and their outcome;
- any platform or release gate that remains open.

Do not commit secrets, generated runtime state, temporary Share tokens, or test
access URLs. SuperPlexr state directories are intentionally ignored by Git.

By contributing, you agree that your contribution is licensed under the
repository's [MIT License](LICENSE).
