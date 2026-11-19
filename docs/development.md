# Development

Hession is a Tauri desktop app for Linux and macOS, with React in `src/` and
Rust in a shared Cargo workspace. [User behavior and CLI usage](user-guide.md)
and the [sync protocol](sync-protocol.md) have their own references.

## Setup

Use Node.js 20.19+ or 22.12+ and Rust stable ≥ 1.90. Install system libraries
with `make deps` on Debian/Ubuntu, Fedora, or Arch (uses sudo), or install Xcode
command-line tools with `xcode-select --install` on macOS. Run `make dev`.

The Makefile selects `/usr/bin/pkg-config` on Linux so GTK/WebKit are found.
Set `PKG_CONFIG=/usr/bin/pkg-config` when invoking npm/Tauri directly there.
Use `HESSION_DATA_DIR=/tmp/hession-dev make dev` for a scratch library. The GUI
instance guard is app-wide within the login session, independent of library path;
close another desktop instance before testing against a scratch library.

| Task | Command |
| --- | --- |
| Desktop / browser preview | `make dev` / `make preview` |
| Binary / installers | `make build` / `make bundle` |
| Faster native build | `make build-debug` |
| Full checks | `make verify VERSION=x.y.z` |
| JavaScript / Rust tests | `npm test` / `cargo test --locked --workspace` |
| Formatting / lint / unused code | `make fmt-check` / `make lint` / `make unused` |
| Relay container integration | `make test-relay-container` |
| All targets and options | `make help` |

Build output is ignored: `dist/` contains frontend assets, `target/debug/` and
`target/release/` contain binaries, and `target/release/bundle/` holds installers.
On Linux, `make desktop-entry` installs a user launcher/icon for development;
`make desktop-entry-remove` removes it. System packages install their own entry.

Browser preview uses `src/dev/mock-ipc.js` and synthetic `src/dev/fixtures.json`;
production excludes the mock. `make preview-fixtures` regenerates synthetic data.
`make preview-fixtures FROM=/path/to/sessions.sqlite` instead writes private,
git-ignored `src/dev/fixtures.local.json`, which preview prefers. For shareable
screenshots and isolated checks, use a clean source copy without that file or
personal settings. Preview never contacts a relay or launches an agent.

## Architecture

| Location | Responsibility |
| --- | --- |
| `src/` | React views, IPC adapter, transcript presentation, preferences |
| `src/syntax*.js`, `src/components/HighlightedCode.jsx` | Bounded local syntax worker, safe token rendering, plaintext fallback |
| `src-tauri/src/` | Native adapters, imports, observation, SSH, continuation, sync, updates |
| `crates/hession-core/` | SQLite storage, migrations, scoped reads, search, project evidence/export, locks |
| `crates/hession-cli/` | Headless command dispatch and scoped stdio MCP |
| `crates/hession-sync/` | Portable encrypted records and protocol primitives |
| `crates/hession-relay/` | Self-hosted opaque-record HTTP relay |
| `deploy/relay/` | Container and Compose deployment |
| `.github/scripts/`, `.github/workflows/` | Release verification, package checks, CI |

`hession-core` has no Tauri/network dependency. Desktop adapters run blocking
services on workers. `main.rs` dispatches explicit commands before initializing
Tauri. Only bare desktop launches register the single-instance plugin, first in
the builder; later launches restore and focus `main`. macOS reopen events use
the same handler. Do not enable the plugin's version-splitting feature: upgrades
must use the same instance identity. Its normal exit/restart cleanup releases
ownership; CLI/index/MCP processes retain independent lifecycles.

### Storage and coordination

Migrations are transactional and record `application_schema`; SQLite
`user_version` remains the portable-backup format. Newer/malformed schemas fail
without clearing history. `open_read_only` never creates or migrates a library.
Keep backup/sync allowlists aligned with schema changes. Close older binaries
that predate cross-process coordination before sharing an upgraded library.

Writers acquire the kernel library lock, then the in-process priority gate,
then the transaction, using the actual SQLite connection. Initialization uses
the same gate; WAL readers can coexist with writes. Hashed source locks serialize
parsing/checkpoints without disclosing native paths. Original harness files are
never locked or written. An unresolved or non-UTF-8 library filename fails;
only in-memory fixtures use process-local locks. Invalid explicit data-directory
overrides must not fall back to a personal library.

Maintenance advances a transactionally checked epoch. Local-only `index_*`
tables are excluded from sync and emptied in portable backups. Automatic imports
respect source exclusions after deletion; explicit imports may clear them.

### Local indexing

The native notify observer registers bounded known directories, with recovery
reconciliation for missing roots, queue gaps, and filesystem errors. Discovery
does not follow symlinks below a configured root; the root itself may be a link.
Unsupported/unreadable areas are reported as potentially stale.

| Bound | Value |
| --- | --- |
| Notification queue / debounce sources | 256 / 512 |
| Debounce / maximum continuous-write delay | 400 ms / 1 s |
| Degraded polling / healthy metadata safety audit | 15 s / 10 min |
| Discovery depth / directories / source files | 24 / 4,096 / 100,000 |
| Temporary staging / native line / session metadata | 4 GiB / 8 MiB / 16 MiB |
| Delta page | 100 entries, 1 MiB |
| Durable change references | 10,000, without transcript bodies |

Claude/Codex append preparation stages bounded normalized records in private
temporary SQLite. SHA256 verifies the complete committed prefix and the snapshot
prefix again before commit. Normalization covers new records; validation I/O
still grows with existing source size. Late tool results, usage, analytics, FTS,
and checkpoints commit atomically. Other adapters use source-scoped rebuilds;
SQLite discovery tracks WAL fingerprints and reads source snapshots.

Delta cursors bind library identity, epoch, session, and sequence. Older tool
updates are included. Rewrites, deletion, and retention gaps return a bounded
tail with `reset: true`. Desktop invalidations contain only revision and capped
session IDs; clients coalesce refreshes.

### Search and scoped reads

`hession_core::search` serves desktop Developer search, CLI, MCP, and export
adapters. Legacy search retains its own behavior. Query syntax is documented
once in the [user guide](user-guide.md#find-and-read-sessions). Search uses quoted
FTS expressions or literal punctuation matching, bounded plain-text excerpts,
and Unicode scalar highlight offsets. Render excerpts as text, never HTML.

An immutable workspace allowlist applies in SQL outside every OR before content
is fetched; an empty allowlist denies all. Unrestricted local access is an
explicit API. Scope captures library identity and exact workspace-ID/path pairs;
replacement or ID reuse fails closed. Cursors bind revision, scope, exposure,
operation, arguments, and page size. Session/message matching and metadata-only
visibility must remain consistent across adapters. Scoped desktop BM25 can use
library-wide statistics, but those statistics never expand returned scope.

| Read bound | Value |
| --- | --- |
| Query bytes / terms / page rows / workspaces | 4,096 / 64 / 100 / 512 |
| Selected metadata row / aggregate | 64 KiB / 16 MiB |
| Eligible transcript cell / response | 1 MiB / 1 MiB |
| Visible text per entry | 2,048 characters from 16 parts, at most 8,192 source bytes |
| Desktop search workers / SQLite budget | 2 / 3 s |

Size preflights use SQLite byte lengths before materializing strings. Visible
text reads use bounded incremental BLOB prefixes. Oversized eligible cells fail
rather than truncating the approved scope. Metadata-only reads never inspect
transcript cells. Snapshot adapters own their interrupt/cancellation deadline.

### CLI and MCP

The CLI crate's `hession-headless` development binary runs the same reader,
export, and MCP dispatcher without GUI/index adapters. The installed binary's
index command delegates to the existing importer/observer. Follow/watch support
SIGINT/SIGTERM; bounded stdout workers retain permits until OS writes actually
finish, and shutdown joins catalog workers with a bounded runtime timeout.

MCP scope and exposure are fixed by server startup, never request arguments.
The projection excludes tool bodies, free-form evidence, source paths, Git
inspection, and raw provider/context payloads. Transcript consent admits only
explicit user/assistant text parts; unsafe legacy combined text has no fallback.
There are seven read tools, stdio only, with diagnostics exclusively on stderr.
See [owner configuration](user-guide.md#mcp-server-permissions).

| MCP bound | Value |
| --- | --- |
| Input line / arguments / tool result | 128 KiB / 16 KiB / 512 KiB |
| Cursor / simultaneous requests / database workers | 4,096 bytes / 16 / 2 |
| SQLite progress / outer deadline | 3 s / 5 s |

Worker exhaustion rejects immediately. Cancellation interrupts reads and retains
permits until workers exit; pipe backpressure also retains request permits.
Duplicate pending IDs close the malformed stream. History is untrusted data and
must never be interpreted as instructions. CLI export alone reaches the shared
writer; MCP has no export, indexing, filesystem, or launch capability.

### Project archives and SSH

Project export shares immutable prepare/write services between desktop and CLI.
Preview writes nothing. Validate the library/project identity again before
publication; temporary writes, cancellation, checksums, and atomic no-clobber
publication must preserve existing destinations. Privacy defaults omit tool
bodies, machine paths, Git metadata, and statistics and redact known secrets.

SSH probes use POSIX `sh` over stdin and `BatchMode=yes`, respecting the remote
environment. A single filtered rsync mirrors supported stores under
`<data>/remotes/<host>/root/<absolute path>`, retaining mtimes. Imported sessions
carry host-qualified workspace identities. Resume wraps commands in `ssh -t`;
unreachable hosts are reported individually without discarding offline copies.

## UI and branding

`brand.json` owns product identity. `make brand icons` updates generated assets;
frontend `brand.js` and Rust's generated `brand` module expose it. Theme tokens
live in `src/styles/tokens.css`; bundled Geist fonts need no network. Use text-size
and theme tokens so preferences apply throughout the app. Shared filters use
`OptionFilter.jsx`/`useDropdown.js`; native dialogs use `useModalDialog.js`.

Syntax highlighting runs in a lazy bounded worker, rendering validated text/span
trees instead of HTML. Unknown languages, large blocks, timeouts, and worker
failures fall back to plaintext. Copy always uses original text. Preserve the
production CSP's `worker-src 'self' blob:` for the self-contained inline worker.

## Tests

`make verify VERSION=x.y.z` builds the frontend and runs formatting, ESLint,
ShellCheck, Clippy, unused-code checks, Node/React/Rust tests, and version checks.
Component tests use real controls with only native boundaries mocked. Targeted
frontend coverage is available with `npm run test:coverage`; it is not whole-app
or Rust coverage. `cargo test --locked -p hession-core` needs no GUI libraries.

Use scratch data, synthetic agent roots, loopback servers, and injected sync
credentials. Never use personal histories or real OS credentials in fixtures.
Native subprocess/signal/rsync/relay tests need working local sockets and process
permissions; a sandbox timeout is not a pass. The remote integration fixture
clears inherited environment before invoking its stand-in SSH and real rsync.

Regression suites cover large Unicode/NUL histories, chunked sync and restart,
SQLite migrations and cross-process locks, bounded search/MCP cancellation,
Git limits and dirty submodules, path quoting, cursor invalidation, and atomic
archives. Check the [protocol](sync-protocol.md) and [security boundary](user-guide.md#security-and-privacy)
before changing enrollment or replication.

For native review, use isolated configuration/source directories: verify tool
panels, syntax, copy, wrapping, minimum width, all themes, full-log jumps, Live
append/scroll behavior, and stale continuation consent. Launch twice, verify the
second process exits and the same window returns, then test CLI while open and
close/relaunch. The candidate workflow runs `native-instance-smoke.py` against
installed packages. On Linux it uses Openbox inside one private D-Bus/Xvfb session
with service activation disabled; macOS uses a disposable CI login session.

## Performance measurements

Run benchmarks serially after compilation, with synthetic data and idle build/test
workers. Keep the baseline and candidate binaries: rebuilding between timed runs
changes cache/load conditions. Record CPU time alongside wall time, retain raw
samples, and alternate baseline/candidate order on the same catalog. Warm-cache
results do not establish cold-start or native WebKit performance.

```sh
mkdir -p target/performance
npm run benchmark
node scripts/benchmark-tool-presentation.mjs
node scripts/benchmark-browser.mjs --ref BASELINE_COMMIT --output target/performance/browser-before.json
node scripts/benchmark-browser.mjs --ref working --output target/performance/browser-after.json
python3 scripts/benchmark-native.py --binary /absolute/path/to/hession --idle --output target/performance/native.json
python3 scripts/benchmark-export.py --seed-binary /absolute/path/to/hession --before /absolute/path/to/baseline-cli --after /absolute/path/to/candidate-cli --output target/performance/exports.json
```

Reading and refresh probes use only synthetic history:

```sh
node scripts/benchmark-browser.mjs --reading --output target/performance/reading-sync/reading-browser.json
PKG_CONFIG=/usr/bin/pkg-config cargo test --release --locked -p app --lib --no-run
HESSION_PRIVATE_SYNC_BENCH_OUTPUT=target/performance/reading-sync/private-after.json target/release/deps/app_lib-<hash> --exact device_sync::performance::private_sync_performance --ignored --nocapture --test-threads=1
HESSION_REFRESH_BENCH_OUTPUT=target/performance/reading-sync/refresh.json target/release/deps/app_lib-<hash> --exact commands::query_tests::conditional_transcript_performance --ignored --nocapture --test-threads=1
```

Use the executable path printed by Cargo. Preserve that executable and the
baseline before timing; run these probes alone. The reading browser probe mounts
the production App with synthetic IPC, verifies focus/scroll/DOM preservation,
100 keyboard width adjustments without transcript mutations or reloads, and
390 px conversation/list navigation. It saves normal, focused light/dark, and
phone screenshots beside the report. It makes no native WebKit timing claim.
Private-sync timings cover projection, initial/edited encryption capture, and
quiet capture at 1k/10k events, with digest parity and outbox assertions. They
exclude network latency. The refresh probe compares a full 500-entry projection
and its unchanged-token response against the same 10k-entry library.

The browser probe needs Chrome (`--chrome` selects its executable). It builds the
production tool component from repository source in a temporary directory, uses a
fresh browser profile, and measures collapsed-card React commits and additional
retained heap after GC. Input strings are excluded from that heap difference.
The native probes isolate HOME, XDG, and agent roots; they generate 1k/10k/100k
histories, check source preservation and catalog integrity, and remove fixtures.
Export comparisons verify ZIP checksums and equivalent archive contents, allowing
only generated timestamps to differ. `--samples` and `--sizes` shorten local runs.
The optional idle probe measures one Linux watcher's CPU over 30 seconds.

Use `node scripts/benchmark-unhappy.mjs [ARCHIVED_SRC_DIRECTORY]` for malformed
arguments, truncated prompt wrappers, repeated fallback results, duplicate
highlighting, worker failures, and cancellation. Its timeout is scaled to 20 ms; operation counts show repeated
work independently of timer noise. Add `--unhappy --worker-failures` to the browser
probe to measure failed/fallback cards and eight queued blocks against a stuck
Web Worker using the production 2.5-second timeout. A failed worker drains queued work to plain text;
new requests can retry after a five-second cooldown. Canceled active work releases
its worker, and replacement jobs share any pending lazy startup. Add
`--prompt-failures` to measure repeated unclosed wrappers in the browser.

The ignored `project_export` test `benchmark_failure_paths` measures export
cancellation/cleanup, entry and field limits, invalid search, and oversized
metadata. Set `HESSION_FAILURE_BENCH_OUTPUT` and run it in release mode with
`--ignored --exact --nocapture --test-threads=1`. Source preservation and absence
of partial destinations are checked; no latency threshold is imposed in CI.

For public search API comparisons, build `search_performance` with `cargo test
--release --locked -p hession-core --test search_performance --no-run`. Preserve
that test executable for each revision (copy the probe into older revisions when
needed). Set `HESSION_SEARCH_BENCH_DB` to a new synthetic catalog path, then run
`benchmark_shared_catalog --ignored --exact --nocapture --test-threads=1` with
`HESSION_SEARCH_BENCH_CREATE=1` once to seed it. Subsequent runs require
`HESSION_SEARCH_BENCH_OUTPUT` and reuse that database without the create flag.
Compare result hashes as well as CPU/wall samples across all visibility modes.
The ignored core test `search::performance::profile_search_stages_10k` separates
size-preflight and SQL execution time; `HESSION_SEARCH_PROFILE_OUTPUT` saves it.

`node scripts/benchmark-live.mjs [ARCHIVED_LIVE_STATE_MODULE]` compares live
merging, serialization counts, duplicate IDs, and oversized replays. The browser
probe's `--live` mode uses production Live components with synthetic IPC. It
measures 300-row parent refreshes, empty/changed/replayed pages, and indexing
startup/notification counts. Byte sizes are checked on each nonempty merge;
serialized values are reused only within that merge. Events are revision-check
hints, and indexing refresh callbacks are awaited to prevent overlapping work.

## Dependency security

Run `npm audit` and `cargo audit` against current advisory databases before
publishing. Vulnerability results do not clear soundness/maintenance notices or
constitute a security audit. Tauri's GTK dependency tree includes the
[glib VariantStrIter soundness notice](https://rustsec.org/advisories/RUSTSEC-2024-0429.html);
check the resolved graph and upstream fixes rather than forcing incompatible
versions. Dependency versions and features belong in manifests/lockfiles, not
copied scan totals or dated claims in this guide.

## Releases

1. Add `## [x.y.z]` notes to `CHANGELOG.md`, then run
   `python3 bump_version.py x.y.z --yes` to align manifests, locks, and Compose.
2. Run `make verify VERSION=x.y.z`. Push a release PR for candidate validation.
3. Review native behavior and candidate diagnostics on macOS universal and
   Linux x64/ARM64. Candidates use disposable keys, verify all seven installers
   and updater signatures, exercise actual CLI/MCP and installation/removal,
   and publish no tag, release, or feed. Short GUI startup checks complement
   manual visual review; they do not prove native UI parity.
4. After approval, merge and push `vX.Y.Z`. `release.yml` builds a draft,
   signs/verifies seven installers and the eight-target `latest.json`, and
   publishes only after verification. Both macOS architectures share a universal
   updater archive; Linux targets retain their package format.

The committed updater public key must match the repository secret
`TAURI_SIGNING_PRIVATE_KEY`; password-protected keys also require
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep a secure backup. Changing the public
key breaks existing installations' updates. Missing keys stop production builds.
Never commit private keys, including local ignored `.updater/` files.

Normal development builds need no signing key. Signed local builds use
`src-tauri/tauri.release.conf.json`. Signatures bind the release version;
`.github/scripts/verify-updater-artifacts.mjs` checks cryptography and
`.github/scripts/update-manifest.mjs` checks package/feed completeness. Candidate
keys and files must never replace production artifacts. The installer workflow
checks published downloads; do not rerun the publishing workflow over an already
published release to retry only an installer check.

### Background indexing performance gate for v0.6.0

Use optimized binaries with other builds/tests stopped. Build the candidate with
`make build` so the GUI embeds production assets; a bare `cargo build --release`
uses the development URL and is unsuitable for the GUI probe. The Linux-only probe
creates 160 synthetic Codex transcripts (about 251 MiB), waits for indexing to
settle, samples 35 seconds of idle operation, appends to one transcript three
times, verifies the saved library, and removes its disposable data. It records
observer scheduler CPU, context switches, process read volume, RSS, and PSS.
The benchmark never reads personal conversations.

```sh
python3 scripts/benchmark-idle.py --binary /path/to/v0.5.4/hession \
  --output target/performance-v0.6.0/baseline.json
python3 scripts/benchmark-idle.py --binary target/release/hession \
  --baseline target/performance-v0.6.0/baseline.json \
  --output target/performance-v0.6.0/candidate.json --require-improvement 50 \
  --max-idle-rss-mib 64
```

The comparison requires identical corpus and sampling arguments. The 50× gate
covers observer idle CPU, observer idle context switches, and append read
amplification. Append CPU and RAM improvements are reported separately. Total
measured background CPU must also stay below 0.07% of one logical CPU (the
reported 3.5% divided by 50). This is an indexing performance gate, not a claim
that every operation or the complete GUI becomes 50× faster or uses 50× less RAM.
Memory budgets can be enforced independently with `--max-idle-rss-mib` and
`--max-idle-pss-mib`. Observer wakeups are the energy-use proxy; whole-machine
power and CPU C-state residency are not measured by this probe.

For a full minimized-GUI measurement including WebKit children, add `--gui` to
both runs inside a **private** Xvfb display and D-Bus session with Openbox
available. Set `HESSION_NATIVE_SMOKE_ISOLATED=1` to acknowledge that isolation;
reuse `.github/scripts/native-session-bus.conf`. Extracted window-manager tools
can be configured with `HESSION_TEST_WM`, `HESSION_TEST_WM_LD_LIBRARY_PATH`, and
`HESSION_TEST_WM_XDG_DATA_DIRS`. This mode isolates every source/config directory,
starts and minimizes the real app, and includes its descendant processes. Add
`--hermes-package-dirs 5000` to both full-corpus runs to reproduce an installed
Hermes environment exceeding the old watch budget. The fixed GTK/WebKit footprint is measured separately with a tiny-library control
(`--files 1 --entries 1 --text-bytes 1 --samples 1 --idle-seconds 30`). Use
`--empty-gui-report CONTROL.json --max-native-overhead-mib 16
--max-gui-overhead-mib 96` on the full corpus to gate added native RSS and total
GUI PSS. The control must use the same production binary. These reference budgets
include the selected transcript and library metadata; they separate application
data from the engine's fixed footprint. RSS sums count shared mappings in each process; use PSS when assessing total physical
memory. GUI comparisons must use the same display/compositing setup. Use paired
`--idle-seconds 120` runs for the final GUI gate so it spans engine maintenance
and minute timers; the default 35-second interval is a shorter diagnostic.
Per-thread rows and new/retired thread counts identify remaining work. Observer
CPU uses high-resolution scheduler runtime. The whole-GUI gate checks both those
persistent-thread sums and process CPU-clock totals, which retain work by exited
helper threads. POSIX process CPU clocks avoid coarse `/proc/stat` tick rounding.
Any process churn during the idle interval requires another settled measurement.

Native regression tests cross the former 15-second scan boundary, assert zero
extra discovery/fingerprint/import work while quiet, and assert that a one-file
append touches exactly one source. They also exercise nested source creation,
atomic replacement, directory permission changes, watch-budget exhaustion without
a busy retry loop, failed-source recovery,
pause/resume, native watcher failure, and prompt shutdown. Event paths account for
macOS directory aliases; source-root identity checks reattach replaced watches.
Symlinked source files retain fallback polling because watching their logical
parent does not observe every external target write. Periodic safety
reconciliation compares device, inode,
size, mtime, and ctime; explicit imports and native-gap recovery retain content
verification. No source history is modified by automatic indexing.

On Linux/glibc, completed transcript reads also schedule one coalesced idle
memory reclamation, including when automatic indexing is disabled. The bounded
memory worker blocks without a timer when there is no request. Three seconds of
quiet triggers cleanup; a 30-second maximum delay prevents continuous imports or
reads from postponing reclamation indefinitely. This returns freed
native reply buffers after the final UI refresh; it does not remove displayed
conversation content or change saved history.

Desktop reading queries omit duplicate search text, native message payloads
already represented by visible text/tools, and duplicate plain-text JSON parts
before loading them from SQLite. Bookkeeping and empty-text fallbacks retain
their native payloads. Full-fidelity queries and exports retain original fields;
a regression checks both representations and confirms stored history is unchanged.

Observation scopes follow each agent's history layout. Hermes environments and
package installations, OpenCode caches, and Antigravity artifact trees are not
recursively watched. Explicit source/SQLite companion parents are watched too.
Discovery rechecks directory metadata and source membership after subscribing to
close the list-before-watch race. The ignored metadata-only diagnostic
`installed_observation_watch_plan` compares broad and history-scoped watch plans
without opening transcripts or changing the library. On the development host,
the old plan hit 4,096 directories and truncated; the scoped plan used 354
directories without truncation. This avoids perpetual degraded polling caused by
an agent installation's unrelated packages.

The optimized v0.6.0 candidate passed the synthetic Linux gate against the
published v0.5.4 binary: headless observer idle CPU 93× lower, context switches
175× lower, append reads 160× lower, and retained RSS 49 MiB. With 5,000 synthetic
Hermes package directories, paired 120-second minimized-GUI runs measured observer
CPU 417× lower, context switches 155× lower, append reads 160× lower, and append
CPU 53× lower. Measured GUI CPU was 0.052% of one logical core and PSS 263 MiB;
added native RSS was 7.2 MiB and added GUI PSS 51.3 MiB above the same-binary tiny
control. A preceding 35-second GUI sample measured 0.084% CPU and failed the
0.07% budget; keep that diagnostic alongside the longer passing run. These are
synthetic steady-state results, not measurements of personal-library startup,
whole-machine energy, or a 50× improvement to the GUI's fixed engine footprint.
