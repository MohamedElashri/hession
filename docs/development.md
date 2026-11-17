# Development

Hession is a [Tauri 2](https://tauri.app) app: a React (JSX) frontend in `src/`
and a Rust backend in `src-tauri/src/`. The two communicate over Tauri IPC.
It supports Linux and macOS.

## Setup

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

### Shared developer search

`hession_core::search` provides the typed parser and read-only service for desktop,
CLI, MCP and export adapters. `Library::search` opens an existing library without
migration and enforces a three-second SQLite execution budget. Adapters that own
an existing read transaction can use `search_snapshot` to share its snapshot with
other bounded reads; those callers own the SQL interrupt/cancellation budget. The query and independent `SearchFilters` apply together.
The desktop selects this service through `developer_search` with Developer
search; Legacy search retains the older commands and sorting behavior. The IPC
adapter admits at most two blocking searches and returns a retryable busy error
instead of building an unbounded queue. Result `position` is the row count before
the selected entry, so transcript jumps remain correct when ordinals have gaps.
Stars and bounded tags accompany scoped result metadata.

The grammar uses implicit AND and uppercase `OR` with AND binding more tightly.
A leading `-` excludes a term. Quotes enclose whole phrases or field values, with
`\"` and `\\` escapes; other backslashes remain literal. Supported fields are
`agent:`, `project:`, `file:`, `tool:`, `branch:`, `after:`, `before:`,
`has:errors` and `host:`. Dates are valid `YYYY-MM-DD` UTC calendar dates;
`after:` includes that date and `before:` excludes that date. `host:@local`
selects local sessions. `has:errors` means a recorded failed tool status or
nonzero exit code, not an inferred unsuccessful session. Fields use literal
ASCII-case-insensitive substrings, except exact host matching and date comparison.
Parenthesized Boolean groups and regexes are unsupported. Syntax errors include
UTF-8 byte offsets. A bare `foo::bar`, `C++`, URI or path retains its punctuation;
unknown field names report an error. Quote literal leading parentheses.

Session search permits AND terms in different entries of a session. Message
search requires its text terms in the same entry, with session metadata filters
applied alongside them. Metadata-only session matches can have no entry ID.
Word/phrase search uses safely quoted FTS5 expressions; punctuation uses literal
matching without SQL wildcard interpretation. Ranking sorts the sum of best
BM25 ranks first, with small deterministic phrase/title/exact-path boosts,
then activity time, session ID and entry ordinal/ID. These scores are ordering
values, not probabilities. Scoped desktop BM25 uses library-wide index
statistics; out-of-scope entries can affect numeric rank but never expand the
allowed IDs or excerpts. Restricted MetadataOnly/Conversation policies do not
use those global FTS statistics. Snippets are bounded plain text; highlight spans
use Unicode scalar offsets. A UI must render them as text, never HTML.

The immutable `SearchScope::workspaces` allowlist is always enforced in SQL before
content is fetched, outside every query OR. An empty allowlist denies all;
unrestricted access requires explicitly constructing `SearchScope::all()`.
Never deserialize or construct an MCP scope from a per-call request. The
service caps queries at 4096 bytes/64 terms, pages at 100 results, scope at 512
workspace IDs and encoded responses at 1 MiB. Cursor tokens are bound to query,
filters, mode, visibility, page size, scope and library identity/revision. Any
library mutation expires a cursor; restart the search instead of silently
skipping/duplicating results. `countLowerBound` reports only observed rows plus
one when a later row exists; it is not an expensive full count.

`MetadataOnly` is the default visibility: only indexed metadata is searched and
no transcript excerpts are returned. `Conversation` matches and excerpts only
explicit `content_part.kind=text` attached to user/assistant entries whose kind
is in the existing continuation allowlist (`VISIBLE_CONVERSATION_KINDS`). A
reasoning event with an assistant role and a text part is still excluded. Thinking,
system, developer and tool parts, and legacy combined `entry.search_text`
without safe explicit text parts, are excluded from matching as well as results.
Unknown Antigravity MODEL event kinds remain excluded; known `USER_INPUT`
records are supported. There is no legacy transcript fallback. This restriction is reported in
`visibilityNotice`. `AllIndexedText` is the explicit desktop policy and uses the
complete FTS index. Conversation search scans the allowed text parts using a
streaming Unicode-normalized matcher; its performance differs from FTS search.
MCP adapters must fix visibility from server startup permissions and reject any
request to elevate it. A query must never grant its own content permission.

The opt-in synthetic benchmark creates 10,000 sessions and 50,000 entries in a
temporary database and measures all three visibility policies. It reports 100
warm top-20 searches and the first connection separately. The latter is not an
OS-cold-cache claim; no global cache dropping is attempted:

```sh
CARGO_BUILD_JOBS=4 cargo test --locked --release -p hession-core --test search benchmark_10k -- --ignored --nocapture
```

Reference run (2026-11-17): optimized Rust build on Linux x86_64 kernel
7.0.0-42, AMD Ryzen 7 7735H, 61,008 MiB RAM, NVMe/ext4 temporary storage.
The database was 19,734,528 bytes. The one-minute host load average was 18.48
at the start and 15.40 at the end on sixteen logical CPUs. Each policy used the same four queries
(`compiler regression`, `"regression needle"`, `src/module-42.rs`,
`agent:codex compiler`), twenty warmup searches and one hundred measured
searches, with a new read-only connection per request. Results are local
measurements, not a guarantee for every library or machine.

| Visibility | Warm median | Warm p95 | First connection |
| --- | ---: | ---: | ---: |
| All indexed text | 240.89 ms | 379.93 ms | 347.31 ms |
| Conversation | 197.45 ms | 398.14 ms | 351.45 ms |
| Metadata only | 36.69 ms | 64.72 ms | 50.15 ms |

The 250 ms warm p95 target was not met in this run. These measurements
include the recorded host load and do not establish quiet-host performance.

## Portable project export service

`hession_core::project_export::prepare` reads one existing library snapshot using
an immutable `SearchScope`, the shared developer-query grammar and an exact
workspace ID. It returns `PreparedProjectExport`; only `.preview()` is public.
The prepared selection, options, destination, history revision, sanitized-content
snapshot identities and cached statistics cannot be supplied or altered through
JSON. Adapters should keep the plan in trusted process memory and pass it to
`project_export::write` with the current scope and a per-job `ExportCancellation`.
A changed history revision rejects the write and requires another preview.

The writer creates a private temporary file beside a new destination, streams
Deflate members through the extracted `transcript_export::ExportWriter`, checks
SHA-256 hashes and byte budgets, syncs the complete file, and publishes atomically
without clobbering an existing path. Cancellation and publication share a final
commit gate. Progress callbacks run outside that gate. A directory-sync failure
after publication is reported as a saved-archive durability warning, not a failed
or cancelled write. External read-only MCP tools must not expose this writer.

Schema-v1 manifests contain the producer version, UTC export time, project key,
redacted filters/options, included and omitted categories, snapshot/member
references and uncompressed-member checksums. The manifest excludes itself from
the checksum list. Logical session identities reuse replication/native identity
where available; otherwise a recorded-source digest is explicitly identified as
a weaker cross-device reference. Snapshot identities hash sanitized canonical
JSON with the volatile export timestamp excluded. Identical sanitized snapshots
share transcript members. Local database row IDs never become archive identities.

Defaults bound a job to 2,000 selected sessions, one million scanned entries,
100,000 evidence/context rows, 512 MiB archive/uncompressed output, 64 MiB per
member, 256 KiB per recorded field, 2 MiB per entry and 120 seconds per phase.
Hard caps validate caller-supplied limits. Incremental SQLite BLOB reads reject
oversized TEXT before allocating it, and fixed allowlists exclude raw payloads
and hidden content. No filesystem tree is traversed or original agent log opened.

Dependency review used the published package metadata and documentation already
present in the local registry: exact `zip 9.5.0` (Rust 1.88, MIT), with defaults off
and only `deflate-flate2-zlib-rs`; exact `regex 1.13.1` (Rust 1.65, MIT/Apache-2.0)
for fixed linear-time redaction patterns; and existing `tempfile 3.27.0` (Rust 1.63,
MIT/Apache-2.0) promoted to a runtime dependency. These fit the project's Rust 1.90
minimum and reuse existing locked compression/runtime packages.
