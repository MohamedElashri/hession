# Changelog

## [Unreleased]

### Improved

- Let native background indexing sleep between file events and revision checks.
  Reconcile source metadata periodically instead of walking every source every
  15 seconds or waking the observer every 100 ms.
- Watch agent history locations without recursively observing package installations
  and cache trees that can exhaust native watches and force constant polling.
- Route transcript and SQLite companion-file changes to their dependent sources.
  Appending to one history no longer revalidates every history in its directory.
- Release bulk-import database pages and unused Linux heap pages after imports
  and transcript reads settle, with bounded cleanup during continuous work.
- Send reading views their displayed text and tool fields without duplicate
  search text or native JSON copies. Preserve complete records for exports and
  bookkeeping fallbacks.
- Stop hidden Live-view and indexing-monitor timers, then catch up on visibility.

### Fixed

- Keep failed sources marked stale until they recover and retry them without
  rescanning healthy histories. Wake promptly for pause/resume, shutdown, and
  completion of manual sync or data maintenance.
- Watch the nearest existing parent of missing source roots without traversing
  that parent, and discard unrelated parent events before queue admission.
- Route native events consistently through directory aliases and reattach watches
  when source directories are replaced. Keep polling for symlinked source files.

## [0.5.4] - 2026-11-18

### New

- Read conversations in focus mode with **Cmd/Ctrl+Shift+F**, keeping the current
  transcript page and scroll position. Exit with the focus button or Escape.
- Drag or keyboard-resize navigation and the session list. Save panel visibility
  and reading width in **Layout**, and return to the list on narrow windows.

### Improved

- Coalesce automatic, manual, and private-sync refreshes. Preserve unchanged
  transcript content and defer UI polling while the window is hidden.
- Skip unchanged private-sync histories, reuse query statements, and prepare
  encrypted records outside the library write gate. Keep small encrypted
  snapshots in bounded memory and spill larger ones to owner-only temporary files.
- Import local histories before SSH transfers and limit remote refresh to two
  simultaneous hosts. Preserve cached mirrors when a host is unavailable.
- Read private-sync progress from memory instead of repeatedly scanning workspace
  counts. Reduce duplicate serialization and download assembly allocations.

### Fixed

- Explicitly restore minimized Linux windows before presenting them when another
  launcher activates the existing instance.
- Keep the visible library and reading position after transient refresh failures,
  invalidate cached transcripts after content/tool/tag changes, and handle deleted
  sessions and reused row IDs safely.
- Preserve actual published revision identities when equal content is recaptured.

Synthetic warm Linux measurements show 10,000-event unchanged private capture at
315 → 2.2 ms and unchanged page refresh at 51 → 2 ms. Large source-import timings
remain variable and include integrity verification and additional change tracking.

## [0.5.3] - 2026-11-18

### Fixed

- Hide background Codex, Claude, and OpenCode sessions by default and add
  **Show subagents** to reveal them. Keep histories and annotations intact.
- Preserve a Codex fork's own session identity and metadata when its log also
  contains inherited parent headers.
- Keep hidden helpers outside visible-search size budgets, recover corrupt
  append checkpoints automatically, report storage errors in library statistics,
  and include JSON delimiters in the Live window's byte limit.

### Improved

- Load session pages and filter options using covering indexes and bounded page
  selection. In the synthetic 100,000-session catalog, median browsing improves
  from 192 to 2.8 ms and filter loading from 233 to 1.3 ms.
- Append histories without rescanning old message text or tool results, and
  avoid repeatedly serializing unchanged metadata. Adding ten events after
  10,000 completed tool calls improves from 927 to 22 ms median.
- Reuse bounded previews for repeated large tool arguments and skip doomed
  parsing of incomplete JSON envelopes. Fifty repeated 1 MiB tool cards improve
  from about 40 to 4.9 ms median, with 96–98% less retained preview memory for
  repeated valid path/code payloads.
- Reject malformed, mismatched, and expired search cursors before scanning
  history. Add repeatable catalog, append, browser, and failure-path probes.

Measurements use warm synthetic Linux/Chrome workloads.

## [0.5.2] - 2026-11-18

### Improved

- Open conversations faster and retain less memory by formatting tool arguments
  and detecting panel syntax only when their panels open.
- Recover from syntax-worker failures, reuse duplicate and plain-text results,
  and release cancelled highlighting without blocking later blocks.
- Parse truncated prompt wrappers without repeated scans, avoid duplicate tool
  argument decoding, and preserve deeply nested JSON when formatting fails.
- Preserve unchanged Live rows and reading position, avoid repeated serialization,
  and count repeated event IDs once.
- Share indexing startup and pause/resume work, coalesce refreshes, retain failed
  preference and notification warnings, and observe revisions despite health-read failures.
- Export large project archives faster with direct transcript pagination and
  reused SQL statements. Serialize cleanup, release late previews, and coalesce
  cancellation requests without allowing stale responses to affect a newer job.

## [0.5.1] - 2026-11-17

### Improved

- Distinguish your messages, agent replies, and tool activity with labeled cards,
  speaker markers, and clearer spacing across the conversation.
- Read tool commands, multiline code, arguments, and output in separate panels
  shared by Conversation, Full log, and Live. Wrap long lines or scroll them,
  copy complete text, and expand explicitly labeled previews of large content.
- Keep failed calls visible in folded tool runs, retain recorded exit codes,
  and distinguish empty output from output that was never recorded.
- Reduce nested tool borders, show readable action names and explicit outcomes,
  and distinguish command and output panels with labels, icons, and accents.
- Add local syntax highlighting for commands, code, JSON arguments, diffs and
  fenced Markdown. Choose a language or plain text per tool panel. Highlighting
  runs in a bounded background worker; large blocks remain readable as plain text.
- Restore and focus the existing desktop window when Hession is launched again;
  keep explicit CLI, indexing, and MCP commands independent and headless.
- Consolidate setup and privacy help into the user guide, shorten development
  documentation, and describe current README features without release labels.

## [0.5.0] - 2026-11-17

### New

- **Automatic local indexing:** observe local history while Hession is open, with
  incremental Claude/Codex appends, recovery checks for other stores, pause/resume
  controls and indexing health. SSH refresh keeps its startup/manual settings;
  private device sync remains manual.
- **Live activity:** browse recent local sessions and follow bounded event updates.
  Following starts off; new events preserve your reading position. Labels describe
  recorded activity rather than claiming that an agent process is running.
- **Developer search:** combine words with AND, phrases, OR, exclusions and typed
  agent/project/file/tool/branch/date/error/host filters. Browse sessions or matching
  messages, inspect highlighted excerpts and jump to the exact transcript event.
  Legacy search remains available.
- **Projects, files and Git:** follow reported path references to their source
  events. Inspect local Git explicitly to distinguish reported read/write intent
  from observed worktree changes and verified commits, without attributing agent
  authorship or contacting remotes.
- **Portable project ZIPs:** preview a scoped project selection, export Markdown
  and JSON with a versioned manifest and checksums, and cancel without a partial
  destination. Redaction starts on; tool bodies, absolute paths, recorded Git and
  statistics require explicit inclusion. Archives are reading copies, not backups.
- **Keyboard navigation:** find commands, sessions and workspaces from the command
  palette; use platform-specific shortcuts with focus restoration and input guards.
- **Headless CLI and MCP:** query indexed sessions, search, projects and files,
  follow recorded changes, index locally and export projects without a webview.
  The seven-tool MCP server is read-only stdio, requires an explicit project
  allowlist and returns metadata by default. Safe conversation text requires a
  separate owner opt-in; raw tool bodies and reasoning are excluded.

### Improved

- Share history services across desktop and headless interfaces. Coordinate
  writers, maintenance, source imports and watcher leadership across processes.
- Migrate v0.4 libraries transactionally while retaining session IDs, annotations,
  native resume, existing exports, backups, continuation and encrypted sync.
- Bound new read/search/export work, handle oversized events explicitly and apply
  backpressure to MCP/output queues. Local derived indexes stay outside sync data.

### Before upgrading

Create a SQLite backup in **Settings → Saved data** before upgrading and keep it
separate from the live library. The backup format remains v1. For rollback, install
v0.4 and restore the pre-upgrade backup into a fresh data directory; keep the v0.5
library copy until verified. Project ZIPs cannot restore a database or credentials.
New project identities preserve recorded host/workspace distinctions; aliases and
sibling worktrees are not silently merged. Pattern redaction cannot find every
secret. Metadata can include titles derived from prompts and recorded paths.

## [0.4.0] - 2026-11-13

### Improved

- **One Settings window:** organize appearance, terminal, history refresh,
  remote hosts, private sync, continuation, saved data, and About in one place.
- **About and updates:** check for signed updates, download and install them,
  and restart when ready from Settings → About. The Settings button shows
  download progress and when an update is ready to apply.
- Keep unfinished edits when switching sections, remember the last section,
  and support keyboard navigation. Keep Settings open after saving a terminal.
- Prevent closing or switching sections while saves and backup operations finish.
- Adapt Settings to smaller windows and all appearance themes. Correct accessible
  names for the terminal path and the About update indicator.
- Update the user guide and preserve sync/continuation as separate, optional
  features that remain disabled until enabled.

## [0.3.0] - 2026-11-12

### New

- **Private device sync:** exchange selected histories, stars, and tags through
  your own end-to-end encrypted relay. Includes Docker Compose setup, device
  invitations, recovery keys, download-only scope, and retryable transfers.
- **Cross-harness continuation:** review session context and a matching local
  checkout, then start a new Claude Code or Codex session in your chosen terminal.
- Both features are **optional and disabled by default**, with separate controls
  and offline setup guides in Settings. Local/SSH imports are now called
  **Refresh histories**.

### Improved

- Preserve the selected session and transcript page after successful or failed sync.
- Retain the latest user request in long handoffs and detect further changes in
  dirty submodules before launch. Correct Unicode sync estimates and multipart counts.
- Strengthen interrupted setup, cancellation, backup/restore, subprocess limits,
  and dependencies. Add large-history, native desktop, and container checks.
- Shorten user guides while keeping setup, recovery, privacy, and current limits explicit.

### Before upgrading

Existing local/SSH histories continue to work; neither new feature enables itself.
Back up your library through the database menu before upgrading. Sync credentials
and recovery keys are separate from portable library backups.

Sync is manual and does not transfer project files, delete remote history, or
rotate encryption keys. The initial protocol has not had an independent security
audit. Continuation starts a new session with reviewed context; it does not migrate
a running agent or convert native transcript files. The chosen harness may send
that context to its model provider.

## [0.2.1] - 2026-11-09

- Install updates inside Hession with release notes, download progress, and
  restart when ready.
- Verify signed updates for the macOS app and Linux AppImage, deb, and rpm
  installations, preserving the installed package format.
- Keep update progress when the dialog closes and allow retries after errors.

## [0.2.0] - 2026-11-09

- Search complete sessions with highlighted excerpts and jumps to matches.
- Choose automatic, discovered, or custom terminals in Settings.
- Find session titles and workspace paths in global search.

## [0.1.2] - 2026-10-24

### Added

- Appearance controls for themes, accent colors, text size, spacing, code
  wrapping, and reduced motion.
- Binary-only and debug builds; installer packaging via `make bundle`.

### Changed

- Faster startup, transcripts, search, analytics, and maintenance previews.
- Stream JSON and Markdown exports to disk with lower memory use.
- Consolidate shared code and remove stale comments and debug UI.

### Fixed

- Keyboard focus restoration when closing dialogs.
- Date-only parsing in analytics.

## [0.1.1] - 2026-10-21

### Added

- Manual update checks with release notes and terminal-based installation.

### Changed

- Clearer installer messages for reinstalls and completed installations.

### Fixed

- Treat removed `.deb` packages with leftover configuration as uninstalled.

## [0.1.0] - 2026-10-21

Initial release for macOS (universal) and Linux.

- Search local and SSH-hosted sessions from nine coding agents.
- Conversation, prompt-index, and full-log transcript views.
- Resume sessions in their native CLI or open workspaces in VS Code.
- Analytics for tokens, estimated costs, tool failures, and activity.
- Light/dark themes and bundled Geist fonts.
- Download installer with SHA-256 verification.

[0.5.3]: https://github.com/MohamedElashri/hession/compare/v0.5.2...v0.5.3
[0.4.0]: https://github.com/MohamedElashri/hession/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/MohamedElashri/hession/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/MohamedElashri/hession/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/MohamedElashri/hession/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/MohamedElashri/hession/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/MohamedElashri/hession/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MohamedElashri/hession/releases/tag/v0.1.0
