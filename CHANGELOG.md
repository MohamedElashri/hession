# Changelog

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

[0.2.1]: https://github.com/MohamedElashri/hession/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/MohamedElashri/hession/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/MohamedElashri/hession/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/MohamedElashri/hession/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MohamedElashri/hession/releases/tag/v0.1.0
