# Development

Hession is a [Tauri 2](https://tauri.app) app: a React (JSX) frontend in `src/`
and a Rust backend in `src-tauri/src/`. The two communicate over Tauri IPC.
It supports Linux and macOS.

## Setup

Local observation uses pinned `notify` 8.2.0 with platform-native notifications,
nonrecursive registrations over bounded known store directories, a 256-slot
callback queue, and a 512-source debounce map. Debounce is 400 ms with a one-second
maximum delay under continuous writes. Missing roots, native errors, or queue
gaps recover through bounded reconciliation; metadata polling runs every 15
seconds and complete-prefix JSONL audits every ten minutes. Directory discovery
does not follow symlinks found below a configured root and caps depth at 24,
directories at 4,096, and discovered source files at 100,000. A configured root
can itself be a directory symlink. Unsupported/unreadable areas are reported as
potentially stale rather than described as continuously live.

Application schema 2 adds local-only `index_*` tables. They are excluded from the
history/sync allowlists, contain no portable sync protocol changes, and are
emptied in portable SQLite backups. Maintenance increments a transactionally
checked epoch. Automatic imports respect hashed source exclusions after deletion;
explicit imports can clear those exclusions. Close v0.4 before using the same
library with v0.5; old binaries do not participate in these locks/checkpoints.

Claude/Codex append preparation stages one bounded normalized record at a time
in a private temporary SQLite file (4 GiB staged-text cap, 8 MiB per native line,
16 MiB session metadata cap). Native files are never locked or written. Every
append verifies SHA256 over the entire committed prefix, then verifies the
snapshot prefix again before committing; normalization work covers new records,
but validation I/O remains proportional to existing source bytes. Usage and late
tool-result reconciliation, analytics, FTS, and the checkpoint share one main
transaction. Other adapters retain source-scoped rebuilds; SQLite discovery
includes WAL fingerprints and uses the existing read-only source snapshots.

`session_delta` returns at most 100 entries and 1 MiB from a read-only WAL
snapshot. Its durable change cursor includes library identity, maintenance epoch,
session ID and sequence. Updates to older tool results are included. Rebuilds,
deletions, or retention gaps return `reset: true` with a bounded tail. The change
log retains 10,000 references and no transcript bodies. Desktop invalidation
events carry only revision and capped session IDs; clients coalesce refreshes.

You need Node.js 20.19+ or 22.12+, Rust stable ≥ 1.90, and Tauri's system
libraries:

| OS | Command |
| --- | --- |
| Debian/Ubuntu, Fedora, Arch | `make deps` (uses sudo) |
| macOS | `xcode-select --install` |

Then run `make dev`.

The Makefile uses `/usr/bin/pkg-config` on Linux, because Homebrew/Linuxbrew's
`pkg-config` can't find the system GTK/WebKit libraries. If you run `npm`
directly, set `PKG_CONFIG=/usr/bin/pkg-config` yourself.

## Make targets

| Target | What it does |
| --- | --- |
| `make dev` | Run the app with hot reload |
| `make preview` | The UI in a browser with sample data, without building Rust |
| `make preview-fixtures` | Regenerate that sample data from the real backend |
| `make desktop-entry` | Per-user launcher and dock icon for `make dev` (Linux) |
| `make test` | Frontend build, Rust tests, and JS tests |
| `make docker-test` | The same tests in a container, with no system libraries needed |
| `make test-relay-container` | Build the relay Docker image and check authentication, encryption, limits, persistence, and backup/restore with disposable data |
| `make check` | Frontend build + `cargo check` |
| `make lint` | ESLint + ShellCheck + Clippy; any warning fails (`make lint-js`, `make lint-sh` alone) |
| `make unused` | Unused files, exports, and npm dependencies (knip) |
| `make fmt` / `make fmt-check` | Format the Rust code / fail if it isn't formatted |
| `make build` | Release binary only; `make build-debug` for faster testing; `make bundle` for installers |
| `make run` | Run the release binary |
| `make icons` | Regenerate icons and brand assets from `branding/` |
| `make brand` | Apply `brand.json` to the Tauri, npm, and Cargo manifests |
| `make release-check VERSION=x.y.z` | Check that versions agree before tagging |
| `make clean` | Remove build outputs |

On Linux the dock icon comes from the `.desktop` entry whose `StartupWMClass`
matches the window's app_id: the binary name, which `brand.mjs` sets to the
slug in dev too. The `.deb`/`.rpm` install one; for `make dev`, run
`make desktop-entry` once (`make desktop-entry-remove` undoes it).

`make preview-fixtures FROM=~/.hession/sessions.sqlite` previews your own
library instead (a snapshot of the newest sessions, written to the git-ignored
`src/dev/fixtures.local.json`, which the preview prefers when present).

`make preview` replaces the Tauri runtime with a mock (`src/dev/mock-ipc.js`)
serving `src/dev/fixtures.json`: synthetic Claude Code and Codex sessions on
this machine and two SSH hosts, imported by the real backend and dumped through
the real commands. It's the fastest way to work on the UI; the mock is excluded
from production builds.

To develop against a scratch library, set
`HESSION_DATA_DIR=/tmp/hession-dev make dev`.

## Layout

Standard Tauri layout: the frontend in `src/`, the Rust app in `src-tauri/`.
A Cargo workspace at the root keeps one `Cargo.lock` and one `target/`, shared
by the app, shared history services, the encrypted record library, and the optional relay server.

```
Cargo.toml, Cargo.lock  Cargo workspace and shared dependency lockfile
package.json            frontend dependencies and scripts
brand.json              product identity (the only place the name is defined)
branding/               logo sources (mark, icon, favicon) and generated assets
scripts/                brand.mjs, icons.mjs, check.Dockerfile
src/                    React UI: main.jsx, Analytics.jsx, components/, api.js (IPC)
  appearance.js         validated appearance settings, legacy theme migration
  useAppearance.js      preference persistence and device theme changes
  styles/               theme tokens, base, app, components, analytics
  dev/                  browser preview: mock IPC and fixtures
src-tauri/src/
  importer.rs           source discovery, incremental import (+ import_*.rs formats)
  remote.rs             SSH hosts: probe, rsync mirror, host suggestions, ssh commands
  resume.rs             resume in a terminal (local or ssh), open in VS Code
  updates.rs            installation metadata, safe restart, release-page fallback
  commands.rs           IPC commands for search, sessions, facets, and export
  db.rs                 compatibility facade for shared SQLite storage
  analytics.rs, pricing.rs, data_management.rs, sync.rs
src-tauri/tauri.conf.json, capabilities/, icons/   Tauri app config and icons
crates/hession-core/    storage, migrations, transcript reads, revisions and lock primitives
crates/hession-sync/    portable encrypted snapshots and protocol primitives
crates/hession-relay/   standalone self-hosted opaque-record HTTP relay
deploy/                relay container and Compose deployment examples
```

`hession-core` has no Tauri or network dependency. Desktop IPC runs its blocking
services on workers and retains the existing transcript response shape. Run
`cargo test --locked -p hession-core` for migration, snapshot, pagination and
cross-process lock checks without building GTK/WebKit. The fixtures use frozen
v0.4 schema DDL and synthetic history, not a developer's native stores.
The Makefile defaults to four Rust test threads and Vitest uses two workers;
override `RUST_TEST_THREADS` when investigating concurrent regressions.

Application migrations run transactionally and record their version in
`application_schema`. SQLite's `user_version` remains the portable-backup
format version. Newer or malformed application versions fail without clearing
history. `db::open_read_only` never creates or migrates a library; callers must
explicitly initialize it through the desktop before querying an older schema.
Keep backup/sync allowlists and release-version tooling in step with future
schema or workspace changes.

`service::Library` bounds page sizes, offsets and response bytes, and returns a
library identity/revision from the same WAL snapshot as its transcript. These
are internal adapter APIs; transcript payloads require a separate allowlist and
project-scope check before exposing them through external tools.

Writers receive their actual SQLite connection and acquire the kernel library
lock before the in-process priority gate and transaction. Acquiring a write
guard after starting a read transaction returns an error. Initialization uses
the same kernel gate; established desktop connections and read-only services
can read WAL snapshots while another process writes. A separate source lock
serializes parsing and checkpoint decisions for each source, with hashed lock
names that do not disclose native store paths. These gates protect Hession's
library and do not write or lock the original harness files. Kernel ownership
ends when the guard closes or its process exits; the empty private lock files
remain in the library directory and are outside portable SQLite backups.

Build output (all ignored by git):

```
dist/                   built frontend (Vite), embedded into the app
target/debug/           `make dev` and test builds
target/release/<slug>   the app binary (`make run`)
target/release/bundle/  installers: deb/, rpm/, appimage/ (Linux), dmg/ (macOS)
```

## How remote sessions work

1. `remote::probe` runs a POSIX `sh` script on the host over stdin, so
   csh-family login shells work. The script uses the host's environment to
   find each agent's store.
2. `remote::mirror_host` pulls just those paths with a single
   `rsync -aRz --include=*.jsonl,*.json,*.db…` call into
   `<data>/remotes/<host>/root/<absolute remote path>`. rsync keeps mtimes,
   so the importer's size+mtime check works unchanged.
3. `importer::all_sources` appends the mirrored stores. `commit_session` sets
   `session.host` and stores the workspace as `host:/path`.
   `remote::from_mirror` maps a mirror path back to its host and remote path.
4. `resume::remote_plan` wraps the agent command in `ssh -t <host> sh -c '…'`.

SSH always runs with `BatchMode=yes`. An unreachable host adds a `host <name>`
error to the sync, which is not treated as a failed sync.

## Branding

`brand.json` holds the name, slug, wordmark, bundle identifier, env-var prefix,
and tagline. The code reads them through `src/brand.js` (frontend) and
`crate::brand` (Rust, generated by `build.rs`). `index.html` uses
`%BRAND_NAME%`. To rename the app, edit `brand.json`, then run `make brand icons`.

The mark is drawn on a 24-unit grid in `branding/mark.svg`; `icon.svg` and
`favicon.svg` use the same geometry.

Theme and accent colors live in `src/styles/tokens.css`; fonts are Geist and
Geist Mono, bundled via `@fontsource-variable`. Use the tokens (`--surface`,
`--text-2`, `--accent`, …) for UI colors. Theme previews and brand artwork use
fixed colors. Appearance attributes live on the document root so native
dialogs and the startup screen inherit them. Use rem or the text-size tokens
for typography so the text-size preference applies throughout the app.

Agent, workspace, model, branch, and tool filters share `OptionFilter.jsx`.
Their wrappers supply normalized options; `useDropdown.js` handles dismissal.
Native settings dialogs share `useModalDialog.js` to open modally and return
focus to their trigger when closed.

## Tests

`npm test` runs the existing Node unit tests followed by rendered React component
tests in jsdom. `npm run test:unit` and `npm run test:components` run those suites
separately. The component tests mock only the native IPC/dialog boundary and
missing native modal presentation; they exercise actual setup controls, consent
gates, stale previews, interrupted enrollment, and offline help navigation.
They never enroll a real device or launch a model provider.

`npm run test:coverage` reports V8 coverage for the optional-feature dialogs and
consent helpers, with a machine-readable summary in
`coverage/components/coverage-summary.json`. This is targeted frontend coverage,
not a coverage claim for the Rust backend or the entire application.

For dependency checks, run `npm audit` and, when installed, `cargo audit` against
the current RustSec advisory database. Do not treat zero entries in the
vulnerability category as clearing separate soundness or maintenance warnings;
the known upstream boundaries are recorded below under [Dependency security](#dependency-security).
Runtime integration tests use disposable localhost servers and require loopback
binding permission. They should fail clearly when that permission is missing,
not silently substitute a mocked server or skip the test.

The workspace checks include the relay and encrypted record library. Optional
features must remain disabled on a fresh install and when restoring a portable
library backup. Never enable them in test fixtures using a developer's real
settings or OS credential store. Use scratch directories and injected test
credentials for sync integration tests. Browser preview can show settings and
handoff previews but never connects to a relay or launches a real harness.

Read [the sync protocol](sync-protocol.md) before changing replication, and
[the security boundary](sync-security.md) before changing encryption or device
enrollment. Keep protocol-version and harness compatibility failures explicit.

`make test` runs the Rust unit tests and the JS tests (`npm test`;
`src/transcript.test.js` covers how transcripts become conversation turns and
`src/appearance.test.js` covers saved settings, migration, and theme resolution).
`remote.rs` includes an end-to-end test
that puts a stand-in `ssh` on `PATH`, then exercises the real probe, rsync,
and import against a fake remote home. Another test runs the generated
resume command through `sh` with awkward paths and arguments. For visual checks,
use `make preview` to inspect both sessions and analytics with sample data.

## Dependency security

The scans on 2026-11-12 found no entries in npm audit or RustSec's vulnerability
category after updating bundled SQLite, TLS/certificate dependencies, Tokio,
rustix, and Vite. This is a dated scan result, not an independent protocol audit;
rerun against current advisories before release.

RustSec's separate warnings include the upstream
[GTK/glib soundness advisory](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
in Tauri's Linux tree, plus maintenance notices for proc-macro-error and optional
async Secret Service dependencies. Hession uses synchronous Secret Service.
These warnings are not silently ignored; resolving GTK requires a compatible
upstream toolkit change. See [Security](sync-security.md) for user-facing limits.

## Large-project regression checks

`cargo test --workspace --locked` includes real Git and localhost relay fixtures
for these boundaries:

- A 10,003-event history with multi-megabyte Unicode text and embedded NULs,
  encrypted chunking, byte-bounded feed pages, restart from a saved partial
  chunk, and a repeated sync without duplicate sessions or events.
- UTF-8 byte estimates and event counts when an event has multiple text parts.
- 4,000 long filenames that exceed the Git output limit, followed by a smaller
  file list that can be inspected normally.
- Hidden dirty submodules, further edits within an already dirty submodule,
  and exhausted inspection deadlines or repository-count budgets.
- Detached linked worktrees, symlinked checkouts, and renamed filenames with
  Unicode, tabs, and newlines.

For manual native checks, use a scratch `HESSION_DATA_DIR` and synthetic harness
stores. Check a history spanning more than 20 transcript pages: jump to its last
page, search for an event on an earlier page, and open the match in the full log.
Open a continuation against an oversized Git status, reduce its generated file
list, and refresh. Then edit an already dirty submodule after previewing it and
verify that launch requires a new review. These native UI checks are manual;
the automated tests exercise the underlying production code.

## Releases

1. Add a `## [x.y.z]` section to `CHANGELOG.md`; it becomes the release notes.
2. Run `python3 bump_version.py x.y.z --yes` to align desktop and relay manifests,
   lockfiles, and the Compose image tag. It runs metadata/build checks; use
   `--skip-checks` only when running the checks separately.
3. Run `make release-check VERSION=x.y.z` (it fails without a changelog entry).
4. Commit, then push a `vX.Y.Z` tag. The workflow in
   `.github/workflows/release.yml` builds a universal macOS DMG (Apple silicon
   and Intel, on the newest macOS runner) and Linux AppImage/deb/rpm for x64
   and ARM64 into a draft release. It also signs the macOS updater archive and
   Linux packages, verifies all seven installers and signatures, builds the
   eight-target `latest.json` update feed, and publishes only after everything
   passes. Both macOS architectures use the universal archive; Linux targets
   include their package format so updates never switch formats.

### Update signing

The updater public key is committed in `src-tauri/tauri.conf.json`. Its matching
private key belongs in the repository's `TAURI_SIGNING_PRIVATE_KEY` Actions
secret. For password-protected keys, also set
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. A missing private key stops the release
before it creates a draft. Keep a secure backup: replacing the public key
breaks updates for existing installations.

Local private keys can live in the git-ignored `.updater/` directory. Never
commit them. To build signed updater artifacts locally:

```sh
TAURI_SIGNING_PRIVATE_KEY="$PWD/.updater/signing.key" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD='' \
npm run tauri -- build --config src-tauri/tauri.release.conf.json
```

Normal development builds and `make bundle` do not require a signing key.
The release config enables updater artifacts only for signed builds. Tauri CLI
2.12+ binds the release version into each signature; the updater requires it.
`.github/scripts/update-manifest.mjs` checks every expected package and signature
before writing the feed. The updater verifies the cryptographic signature at
installation. `npm test` covers the update controller and feed validation.

Publish the first version containing the updater before testing checks against
GitHub. Older versions still need the existing installer once; in-app updates
work from the new version onward.

## Background indexing storage and bounds

Application schema 3 adds local-only `index_full_source`, `index_source_session`,
and hashed `index_source_generation` state, plus an index on
`index_checkpoint(session_id)`. The ordered migration is transactional and keeps
portable-backup `user_version` unchanged. These derived tables are outside the
history/settings revision lists and the encrypted-sync content model. Backup
preparation and reset clear them through `indexing::clear_local_state`; deleting
the last mapped session removes its plain source URI. Only a hashed generation
remains to reject already prepared imports.

Claude/Codex use their verified append reader. Other adapters use full-source
fingerprints with parser capability version, device/inode, size, nanosecond
mtime/ctime, and SHA-256 of the primary file and relevant companions. SQLite WAL
and rollback journals participate, as do Cline manifests/shared metadata and
Antigravity's optional summaries database.
Routine quiet polling can skip identical metadata. Startup, native dirty/gap
events, and ten-minute reconciliation audits compare content even if metadata
is identical. An unchanged hash avoids normalization/FTS rebuilding; changed
metadata with identical content updates derived state only. When native events
are unavailable, a rewrite hidden by coarse metadata is detected by the next
content audit, not guaranteed by the fifteen-second metadata poll.

Source preparation holds only the library's per-source lock, never an advisory
lock on a native history. Before each commit, the library writer guard checks
both the pass's maintenance epoch and the source generation captured before
preparation. Generation invalidation runs on normalized source-row mutations,
even when a cache was already invalid. A job advances its expected generation
only after its own batch commits. A late source-cache save cannot clear another
writer's invalidation. Legacy JSONL/Cline history and its source cache commit in
one transaction; SQLite imports preserve completed batches, but publish the
source cache only after the complete read snapshot succeeds and its source
identity/generation are revalidated. Parse errors, cancellation and maintenance
leave unsuccessful checkpoints unadvanced.

Full-source hashing reads at most sixteen regular files and 512 MiB in 256 KiB
blocks. Legacy JSONL accepts valid final records without a newline, limits each
line to 8 MiB, the file to 128 MiB and records to 100,000, and applies a
conservative 128 MiB decoded-value budget. Cline JSON reads use the same file and
decoded-value caps. Source SQLite queries cap rows at 100,000, cells at 8 MiB,
and decoded values at 128 MiB, reject oversized blobs before expanding them,
and use a thirty-second cooperative SQL deadline. Limit errors are explicit;
original stores and previously committed library histories are kept. Unsupported
non-UTF-8 native source paths are rejected rather than converted into colliding
lossy keys; ordinary Unicode names remain supported.

Regular-file JSON readers pin a nonblocking file descriptor and verify its
identity against the path. SQLite still opens its existing read-only source by
path after metadata checks; identity checks reject ordinary replacement during
preparation, but a malicious local filesystem actor can race that check and open
(for example, by replacing the path with a FIFO). This is not a filesystem
security boundary. Eliminating that race requires a verified private DB/WAL
snapshot or a dedicated SQLite VFS.

Watcher callback failures discard the failed native backend, retain degraded
health, reconcile by polling, and retry native registration every thirty seconds.
Health becomes active again only after watches are restored and reconciliation
succeeds. `pendingSources` reports remaining discovered checks, rather than the
number of imports already attempted. The local-only `LocalCatalog::manual_pass`
captures a maintenance epoch before discovery and uses manual inclusion policy;
continuous observation retains deletion exclusions. Neither path refreshes SSH
mirrors or starts device sync.
