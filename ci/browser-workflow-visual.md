# Isolated browser workflow visual QA

The ignored desktop test fixture serves the current embedded observer UI against
a disposable native daemon. It creates one Mission, 35 verifier Runs and two
terminal Sessions, then opens an Observer Share on an ephemeral localhost port.
It never attaches to the desktop's ordinary runtime or changes owner Sessions.

Build the desktop test executable with the repository's current build profile,
then use the exact executable path reported by Cargo:

```sh
visual_fixture_dir=$(mktemp -d /tmp/up-browser-workflow-visual.XXXXXX)
SUPERPLEXR_BROWSER_WORKFLOW_VISUAL_DIR="$visual_fixture_dir" /absolute/path/to/superplexr_desktop-test-binary \
  --exact workspace_lifecycle_tests::browser_control_tests::browser_workflow_tests::browser_workflow_visual_fixture \
  --ignored --nocapture
```

The fixture writes `ready.json` with mode 0600 inside that private directory.
It contains the ephemeral browser URL, Mission/verifier/session IDs, and its
isolated native socket. Keep the URL's access key out of screenshots, logs, and
published reports. Read the private file directly from the browser automation
process; the browser removes the access key from its address after connecting.

Use a dedicated ego-browser task space for visual checks. Exercise:

- 1280 px and 390 px viewports, then a 200% zoom equivalent.
- Manual verifier listing, the 32-entry first page and three-entry continuation,
  direct-ID inspection, and selection from the list.
- Execution outcome, receipt count, and subject disposition as separate facts.
- Session navigation, focus, styled terminal output, literal markup, selection,
  copying, and independent read-only side views.
- No page-level horizontal overflow and visible keyboard focus.

To stop, create a file named `stop` beside `ready.json`. The HTTP server shuts
down, the fixture terminates its terminal Sessions, and `RuntimeFixture` removes
its daemon and generated state. A 30-minute deadline provides a backup stop.
Wait for the test process to exit before deleting private connection metadata.
Screenshots may be retained separately from that metadata.

DOM adapter checks remain available without a browser:

```sh
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```
