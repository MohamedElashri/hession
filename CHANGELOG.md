# Changelog

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

[0.4.0]: https://github.com/MohamedElashri/hession/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/MohamedElashri/hession/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/MohamedElashri/hession/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/MohamedElashri/hession/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/MohamedElashri/hession/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/MohamedElashri/hession/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MohamedElashri/hession/releases/tag/v0.1.0
