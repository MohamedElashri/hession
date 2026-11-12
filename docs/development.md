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
by the app, the encrypted record library, and the optional relay server.

```
Cargo.toml, Cargo.lock  Cargo workspace (member: src-tauri)
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
  db.rs                 SQLite schema and connections
  analytics.rs, pricing.rs, data_management.rs, sync.rs
src-tauri/tauri.conf.json, capabilities/, icons/   Tauri app config and icons
crates/hession-sync/    portable encrypted snapshots and protocol primitives
crates/hession-relay/   standalone self-hosted opaque-record HTTP relay
deploy/                relay container and Compose deployment examples
```

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
