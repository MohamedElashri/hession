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

The 2026-11-17 advisory scan queried 1,187 locked public Rust/npm
name-version pairs against OSV/RustSec. No vulnerability-category advisory
matched. This is a dated dependency scan, not an independent security audit.
Rerun it before publishing.

Informational notices remain for
[chrono](https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2026-0543.json),
[chrono-tz](https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2026-0555.json),
proc-macro-error and the older zvariant/zvariant_derive/zbus_names tree.
The previously documented
[glib VariantStrIter soundness issue](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
remains in Tauri's GTK dependency tree. Those toolkit/maintenance notices are
retained explicitly; no compatible dependency-only replacement resolves them
all. Hession uses synchronous Secret Service. See
[Security](sync-security.md) for product limits.

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
workspace IDs and encoded responses at 1 MiB. Before string functions run, metadata cells are capped at 64 KiB and eligible
transcript cells at 1 MiB. Total transcript bytes are not a query limit: the
matcher consumes one cell at a time. The materialized scoped metadata has a
16 MiB budget, including 128 bytes of estimated overhead per session. Its
approved-ID fence is capped before construction and rejects an oversized scope
rather than silently truncating it. Materialized hit CTEs contain only IDs and
rank, and a per-query counter permits at most 174,762 records across the term
CTEs and their union copies (a conservative 96-byte-per-record estimate).
Messages rank fixed-width IDs/score/date before projecting display metadata;
the retained sort window, including cursor offset and the extra paging row,
has the same record cap. Exceeding an input budget returns `InputTooLarge`;
narrow the query/scope or restart with a smaller paging window. These are record
and input budgets, not a claim about an exact allocator or process RSS.
Explicit nested `CASE` gates check scope, visible entry kind/role and part kind
before a matcher can access its TEXT argument; a neighboring SQL `WHERE`
predicate alone does not establish evaluation order. Approved-first `CROSS JOIN`
loops and existing numeric per-session indexes also prevent automatic covering indexes
from copying denied text before those gates. Automatic indexes remain enabled
for bounded numeric hit CTEs. Preflight session/tag scans avoid TEXT-bearing
indexes entirely: SQLite's index-rowid lookup reads a complete index entry,
including its date or label, before a later cell guard could reject it.
ScopedReader search delegates to this shared
checker; project/file reads use a separate metadata-only fence over the fields
those services actually consume.

Cursor tokens are bound to query,
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

Reference run (2026-11-17): optimized Rust 1.92 build on Linux x86_64 kernel
7.0.0-42, AMD Ryzen 7 7735H, 61,008 MiB RAM, NVMe/ext4 temporary storage.
The schema-3 database was 19,996,672 bytes. Team builds/stress were paused;
external workloads remained: one-minute host load was 5.17 at the start and
5.61 at the end on sixteen logical CPUs. Each policy used the same four queries
(`compiler regression`, `"regression needle"`, `src/module-42.rs`,
`agent:codex compiler`), twenty warmup searches and one hundred measured searches,
with a new read-only connection per request. These are local measurements.

| Visibility | Warm median | Warm p95 | First connection |
| --- | ---: | ---: | ---: |
| All indexed text | 151.27 ms | 169.93 ms | 169.41 ms |
| Conversation | 238.81 ms | 335.89 ms | 333.59 ms |
| Metadata only | 33.66 ms | 52.00 ms | 41.11 ms |

The indexed-search 250 ms warm p95 target was met. Restricted Conversation
scanning remains slower. The complete fixture/build-and-query process took
82.22 seconds wall and 77.17 seconds CPU. Earlier indexed p95 was 379.93 ms under
one-minute load 18.48–15.40; load/schema/code differences prevent attributing the
change to one optimization. First connection still makes no OS-cache-cold claim.


The separate Linux-only advisory file-cache experiment recreates the exact
10,000-session/50,000-entry corpus without changing the warm benchmark. It
checkpoints/truncates its own WAL and closes all SQLite handles, fsyncs only its
private fixture DB/WAL, and calls `posix_fadvise(DONTNEED)` on those files before
each fresh read-only query. It never drops global OS caches, touches a user
library, or advises unrelated files:

```sh
CARGO_BUILD_JOBS=4 cargo test --locked --release -p hession-core --test search_cold advisory_file_cache_10k -- --ignored --nocapture --test-threads=1
```

Run it only in a coordinated quiet window. Each query/policy sample reports
advice return codes, query and CPU time, process `read_bytes`, input blocks and
major faults. Successful advisory calls are not proof every page was absent
from the OS cache, and process counters cannot attribute every read exclusively
to the fixture. Query code/allocator and storage-device caches may remain warm.
If eviction is unsupported or storage reads cannot be demonstrated, report that
limit and the ordinary first-connection case rather than claiming a cold-cache
measurement. Other platforms print the unsupported limitation explicitly.


The first advisory attempt (2026-11-17, same hardware) accepted DONTNEED for the
19,996,672-byte fixture DB and observed 1,191,936 process storage-read bytes
(2,328 input blocks). The indexed `compiler regression` query returned the
existing execution-budget timeout after 3,281.17 ms elapsed and 19.93 ms CPU.
This is an observed non-CPU-wait limit with actual I/O, not a cold-cache pass or
proof that every page was cold. The initial harness stopped on that first
timeout; the final test records all twelve query/policy outcomes, tolerates only
`SearchError::TimedOut` as a measured limit, and still fails unexpected errors.
Its summary reports success/timeout counts explicitly. Test completion does not
mean every query passed or met a latency goal. Production deadlines are unchanged.


The final twelve-sample advisory run (2026-11-17) completed with twelve successful
queries and zero timeouts. All twenty-three DB/WAL advisory calls returned zero
(the WAL was initially absent and subsequently empty); every sample observed
positive process storage reads. Four distinct queries per policy give ranges,
not latency quantiles:

| Visibility | Query elapsed range | Query CPU range | Process storage-read bytes |
| --- | ---: | ---: | ---: |
| All indexed text | 658.37–1,040.91 ms | 66.86–197.69 ms | 12,550,144–13,656,064 |
| Conversation | 646.39–821.36 ms | 177.16–334.51 ms | 18,272,256 |
| Metadata only | 43.44–203.65 ms | 25.08–46.13 ms | 1,200,128–1,204,224 |

Fixture construction plus sampling took 34.89 seconds wall and 28.82 seconds CPU;
one-minute load was 4.26–3.31 with team jobs paused. The earlier timeout remains
an observed outcome. These file-advice/I/O samples are not proof of all-pages
OS-cache coldness, and they do not replace the warm p95 gate.

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

### Indexing measurements

On the same Linux Ryzen 7 7735H host (16 logical CPUs, about 59.6 GiB RAM),
30 serialized, fsynced appends across 36 synthetic source files containing
7,224 entries and 7.42 MB measured optimized write-to-commit median/p95 of
422.64/427.64 ms and matching-notification latency of 418.90/424.71 ms. Debug
commit latency was 826.80/858.05 ms. The real filesystem observer, parsers and
SQLite commits were used; one-minute load was 3.88–3.60 in the optimized run.
Quiet legacy metadata checks measured 0.744/1.309 ms and content audits
2.306/2.981 ms (30 samples each).

A 30-second idle watcher over 100 files and 1,100 entries consumed 0.04 process
CPU seconds (0.133% of one CPU), with 15,452 KiB RSS and 15,508 KiB high-water
memory. Two 15-second metadata polls left library revision/event count unchanged.
This is isolated headless watcher memory, not full desktop memory.

Five paired samples appending ten events to 1,000/10,000/100,000 existing events
measured median append costs of 5/18/203 ms, versus 198/5,472/33,151 ms for full
reimports. The first 100,000-event append took 8,306 ms; the remaining four took
193–204 ms. That outlier is retained; checkpoint/I/O attribution is unproven.
Full-prefix hashing reads O(total source bytes) to detect rewrites while
normalization stages new records. This correctness tradeoff does not establish
strictly new-byte-only I/O or a universal two-second bound.

Reproduce with the ignored `indexing_benchmark` and `indexing_scaling_benchmark`
tests in the app crate; they use disposable stores and source directories.

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


### Headless CLI and MCP boundary

`crates/hession-cli` is independently testable without linking the desktop GUI.
The installed app calls `hession_cli::dispatch_env()` before Tauri/webview
initialization. `Gui` preserves no-argument startup; `Handled(code)` exits;
`Index(IndexRequest)` delegates to the application's existing local importer and
observer. The adapter receives the database path, once/watch mode, optional
agent and JSON preference. It may await `wait_for_shutdown()` to handle
SIGINT/SIGTERM. `write_output_async` admits one bounded stdout worker; select it
against a persistent shutdown future so an unread pipe cannot block the signal
driver. A cancelled write retains that worker permit until the OS write returns.
Stop/join catalog workers before exiting; the dispatch runtime uses a bounded
shutdown timeout so a blocked stdout thread cannot prevent process exit.
`write_output` is the synchronous variant for callers that can tolerate blocking.
`report_error` writes stderr and returns its documented exit code. No second
importer is implemented in the CLI crate.

For isolated development, `cargo run -p hession-cli --bin hession-headless --
--database /path/to/existing.sqlite status --json` runs the same reader/MCP/export
dispatch. That development binary intentionally lacks GUI and index adapters.
`cargo test --locked -p hession-cli` includes synthetic database fixtures and
actual subprocess stdio tests. No test uses real histories, credentials or model
calls. Tokio signal notifications require a working local Unix socket pair; a
sandbox that suppresses those writes cannot validate signal shutdown. Run that
process test in a normal native environment rather than treating its sandbox
timeout as a production pass. Run `node --test .github/scripts/install-cli.test.mjs` to test the optional
non-overwriting bundle link installer.

The command parser pins [clap 4.6.7](https://docs.rs/crate/clap/4.6.7)
(MIT OR Apache-2.0, declared Rust 1.85), and the stdio server pins the official
[rmcp 3.10.0 SDK](https://docs.rs/crate/rmcp/3.10.0)
(Apache-2.0, declared Rust 1.88). The crate MSRV remains 1.90. Both disable default
features; rmcp enables only `server` and `transport-io`, with no HTTP/auth/client
transport feature. Tokio is pinned to the workspace's cached 1.54.1. Verify the
resolved feature graph with `cargo tree --locked -p hession-cli -e features`.

`ScopedReader::projects` captures immutable workspace-ID/exact-recorded-path
bindings plus library identity. It requires 1–512 explicit selectors. Every
operation opens `db::open_read_only`, validates those bindings in its read
snapshot and applies the allowlist in SQL before reading session content.
`unrestricted` and `search_indexed` are explicit local CLI APIs; MCP never calls
them. It fixes MetadataOnly/Conversation visibility from startup permission,
never from request data. Cursor fingerprints bind operation, arguments, page
size, exact scope and exposure policy to library revision. Replacement or ID
reuse fails closed. Project paths are indexed identities, not filesystem access;
file paths are literal reported metadata, including external references.

MCP uses an explicit projection whitelist over core project evidence. It drops
free-form evidence, actions, tool bodies, raw provider/context payloads, original
native source fields and Git inspection data. Consent still permits only
`VISIBLE_CONVERSATION_KINDS`, user/assistant roles and explicit text parts.
Session text is capped at 2048 characters from 16 parts per entry. Legacy combined
text has no safe fallback. Direct metadata fields use conditional byte-size gates before clipping. Session
text uses read-only incremental BLOB prefixes (at most 8192 bytes), so SQLite
does not materialize a large TEXT cell before a substring is applied. Search
rejects eligible text cells over 1 MiB before entering the shared matcher. Its
metadata preflight caps a row's selected fields at 64 KiB and their aggregate
at 16 MiB within the approved scope; project/file descriptor reads use the same
metadata guard. These checks use SQLite `octet_length(column)`, which reads the
stored byte size without materializing TEXT. Oversized input returns exit code 5
with a size-limit error; narrowing scope or repairing oversized indexed fields
is required. Metadata-only search never inspects transcript cells, and oversized
excluded/denied conversation cells cannot affect consent-mode results. The
common snapshot response cap is 1 MiB.

MCP input lines are capped at 128 KiB (oversized lines are discarded by the SDK),
arguments at 16 KiB, tools at 512 KiB, pages at 100 rows, queries at 4096 bytes/64
terms and cursors at 4096 bytes. At most 16 requests are in flight; pipe
backpressure retains permits until writes complete. Two database workers use
immediate busy rejection instead of a work queue, with a three-second SQLite
progress deadline and five-second outer deadline. Cancellation interrupts the
read; the permit stays with a detached blocking worker until it actually exits.
Duplicate pending request IDs close the malformed stream. Responses mark
historical data untrusted; no text is interpreted or executed. Diagnostics go to
stderr and stdout is exclusively SDK JSON-RPC.

CLI export calls the shared immutable prepare/write API with one exact project
scope, validates the prepared project key and library identity, and writes only
on an explicit export command. Preview writes nothing. No MCP tool reaches the
writer. Portable archive construction and atomic no-clobber publication remain
owned by the shared export service.


### Project export measurements

An optimized shared-core benchmark on the same reference host used a
230,387,712-byte synthetic database with 1,000 approved sessions, 50,000 events,
50,000 content parts (72 MB text), 5,000 tool calls and 5,000 file-activity rows.
Ten fresh processes per profile ran in alternating order after a separate
checksum warmed filesystem pages; SQLite/application caches were fresh.
One-minute load ranged from 2.68 to 5.94. These are export-reader measurements,
not resident desktop memory or cold-disk results. With ten samples, nearest-rank
p95 is the maximum.

| Profile | Prepare median/p95 | Write median/p95 | Total median/p95 | Peak reader RSS |
| --- | ---: | ---: | ---: | ---: |
| Defaults | 7.47/7.95 s | 9.31/11.63 s | 16.99/19.19 s | 15,500 KiB |
| Tool bodies included | 14.59/15.22 s | 17.02/20.34 s | 31.47/35.42 s | 15,632 KiB |

All twenty completed within unchanged 120-second per-phase limits. Each had
1,003 members; all ZIP CRC checks, 20,040 member checksum/size checks, redaction
canaries and database/native-source invariance checks passed. Median compressed
sizes were 10.145/16.535 MB for defaults/tools. Validation was outside timing.
Earlier pilots under concurrent builds include a 120-second tool-export write
timeout with clean temporary-file removal, and a successful retry with about
88 seconds of non-CPU preparation wait. Those pilots remain recorded; final
warmed samples do not erase them.

## Candidate package validation

The `Candidate packages` workflow builds macOS universal and Linux x86_64/ARM64
installers from the release PR with disposable updater keys. It verifies real
package signatures and their authenticated release version independently with
Node's Ed25519 and streaming BLAKE2b APIs, exercises the real application's
headless dispatch and an official MCP SDK client, and checks package installation,
reinstallation, CLI paths and removal. No tag, GitHub release or update feed is
created. The retained artifacts are test-signed candidates; they do not use the
production updater key and must not replace production release artifacts.

The workflow's short native GUI launches check startup and backend initialization;
these are distinct from manual visual review. Synthetic browser checks cover
keyboard/focus, Live scrolling and export dialogs, including light/dark and the
800-pixel minimum width. They do not establish macOS/Linux native-webview visual
parity. The regular release workflow remains the publishing path after review.

Library write paths require UTF-8 filenames. An unresolved SQLite filename must
fail instead of falling back to a process-local lock; in-memory fixtures alone
use that fallback. An invalid explicit data-directory override is preserved and
rejected, never silently redirected to the default personal library.

The local Linux x86_64 test-signed deb candidate was also launched on an isolated
Xvfb display with synthetic history and private config/data/source directories.
Its native webview displayed the initial session, automatically indexed and
displayed a timestamped append in Live, and opened the command palette with
Ctrl+K. Screenshots were inspected. Extracted deb/rpm/AppImage executables passed
help/version, missing-library, synthetic import and status checks. All three real
updater signatures and their signed 0.5.0 version verified. This limited native
Linux check does not establish macOS/ARM64 package or visual parity; those
remain candidate-CI gates. The test-signed files are not production updates.
