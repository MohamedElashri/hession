# Development

Hession is a [Tauri 2](https://tauri.app) app: a React (JSX) frontend in `src/`
and a Rust backend in `src-tauri/src/`. The two communicate over Tauri IPC.
It supports Linux and macOS.

## Setup

You need Node.js 20.19+ or 22.12+, Rust stable ≥ 1.85, and Tauri's system
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
A Cargo workspace at the root keeps one `Cargo.lock` and one `target/`.

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
  commands.rs           IPC commands for search, sessions, facets, and export
  db.rs                 SQLite schema and connections
  analytics.rs, pricing.rs, data_management.rs, sync.rs
src-tauri/tauri.conf.json, capabilities/, icons/   Tauri app config and icons
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

`make test` runs the Rust unit tests and the JS tests (`npm test`;
`src/transcript.test.js` covers how transcripts become conversation turns and
`src/appearance.test.js` covers saved settings, migration, and theme resolution).
`remote.rs` includes an end-to-end test
that puts a stand-in `ssh` on `PATH`, then exercises the real probe, rsync,
and import against a fake remote home. Another test runs the generated
resume command through `sh` with awkward paths and arguments. For visual checks,
use `make preview` to inspect both sessions and analytics with sample data.

## Releases

1. Add a `## [x.y.z]` section to `CHANGELOG.md`; it becomes the release notes.
2. Run `python3 bump_version.py` to set the version in every manifest.
3. Run `make release-check VERSION=x.y.z` (it fails without a changelog entry).
4. Commit, then push a `vX.Y.Z` tag. The workflow in
   `.github/workflows/release.yml` builds a universal macOS DMG (Apple silicon
   and Intel, on the newest macOS runner) and Linux AppImage/deb/rpm for x64
   and ARM64 into a draft release, checks all seven installers, and
   publishes it.
