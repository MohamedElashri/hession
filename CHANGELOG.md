# Changelog

All notable changes to Hession are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.1] - 2026-10-21

### Added

- **Check for updates**, from the new info button at the bottom of the
  sidebar. Hession never checks or updates on its own: a check is one request
  to GitHub for the latest release, and nothing about you or your library is
  sent. When a newer version exists, you see its release notes and can update
  in a terminal (using that release's checksum-verified installer), copy the
  command, or open the release page.

### Changed

- The installer says "Reinstalling" when `--force` reinstalls the same
  version, and finishes with a clearer final line.

### Fixed

- The installer counted a `.deb` package that was removed but left its
  configuration behind as still installed.

## [0.1.0] - 2026-10-21

First release, for macOS (universal) and Linux (x64 and ARM64).

### Added

- One searchable library for Claude Code, Codex, OpenCode, Cline, Pi, Command
  Code, Muse, Hermes Agent, and Antigravity sessions.
- **Remote hosts**: sessions on SSH machines (including VS Code Remote-SSH
  workspaces) are copied in on each sync with `ssh` and `rsync`, work like
  local ones, and can be filtered by host. Works with macOS's openrsync and
  with stores behind symlinked paths.
- **Conversation view**: transcripts read as turns, with your prompts set
  apart, the agent's replies as prose, one line per tool call, and injected
  context, thinking, and bookkeeping collapsed. **Prompts** and **Full log**
  views for skimming and debugging.
- **Resume** a session in its agent CLI, locally or over `ssh -t`, or open
  its folder in VS Code (Remote-SSH for remote sessions).
- Analytics for tokens, estimated cost, tool failures, and activity.
- Light and dark themes, and bundled Geist fonts.
- `curl | sh` installer that installs or updates the latest release and
  verifies its SHA-256 checksum.

[0.1.1]: https://github.com/MohamedElashri/hession/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MohamedElashri/hession/releases/tag/v0.1.0
