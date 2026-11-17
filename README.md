<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/wordmark.svg">
    <img alt="Hession" src="branding/wordmark-light.svg" width="340">
  </picture>
</p>

Search, read, analyze, and resume your coding-agent sessions on macOS and Linux.
Keep local and SSH histories in one offline library, optionally sync between
devices through your own encrypted relay, or continue a task in another harness.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/sessions-dark.png">
  <img alt="Hession session library" src="docs/screenshots/sessions-light.png">
</picture>

## Install

macOS: Apple silicon or Intel. Linux: x64 or ARM64.

```sh
curl -fsSL https://raw.githubusercontent.com/MohamedElashri/hession/main/install.sh | sh
```

The installer verifies checksums and selects a macOS app, Debian/RPM package,
or AppImage. Download packages directly from
[Releases](https://github.com/MohamedElashri/hession/releases).

Open **Settings** for appearance, terminal, history refresh, remote hosts, private
sync, continuation, and saved data. Update through **Settings → About → Check for
updates → Download and install → Restart Hession**.
In-app downloads are signature-verified. Linux system packages may request
administrator authorization. Your library is kept when updating or uninstalling.
Run the installer with `sh -s -- --help` for package, upgrade, and uninstall options.

AppImages use [host graphics libraries](https://github.com/AppImageCommunity/pkg2appimage/blob/master/excludelist), including for terminal commands.
On minimal Debian/Ubuntu systems, install them first:

```sh
sudo apt-get install -y libegl1 libgles2 libgl1 libgbm1
```

Debian/RPM packages resolve their runtime dependencies through the package manager.

## Start here

1. Open Hession to index local agent histories and their changes. **Refresh
   histories** also imports SSH copies; its menu offers full rescans. Configure
   startup refresh under **Settings → History refresh**.
2. Add an SSH machine with **+** beside **Hosts**, then refresh. SSH must work
   without password prompts, and `rsync` must be installed on both machines.
3. Search or filter sessions, read the conversation or full log, and use
   **Resume** to reopen an original session or **VS Code** to open its folder.

Supported histories: Claude Code, Codex, OpenCode, Cline, Pi, Command Code,
Muse, Hermes Agent, and Antigravity. Stars, tags, Markdown/JSON export, token
and cost analytics, appearance options, and terminal preferences are included.

## Features

- Readable conversations distinguish **You**, **Agent**, and **Tool activity**,
  with separate command/output panels, syntax highlighting, wrapping, and full-text copy.
- Automatic local indexing with pause, recovery and health controls; **Live**
  follows recorded events while preserving your reading position.
- Developer search supports AND, OR, phrases, exclusions and typed filters, with
  matching-message excerpts and precise transcript jumps. Legacy search remains.
- **Files and Git** connects recorded projects and paths to source events, with
  explicit Git inspection and separate reported intent/observed-change labels.
- Preview portable project ZIPs with privacy options, redaction and checksums.
- Open commands, sessions and workspaces with **Cmd/Ctrl+K** and keyboard shortcuts.
- Use [terminal commands and scoped read-only MCP](docs/user-guide.md#terminal-commands-and-read-only-mcp)
  without opening a desktop window.

## Optional features

Both are disabled by default and independent on each device:

- [Private device sync](docs/user-guide.md#private-device-sync): select histories,
  preview, and transfer through your own encrypted relay. Repository files and
  native agent stores stay on their original machines.
- [Cross-harness continuation](docs/user-guide.md#cross-harness-continuation):
  review context and a local checkout, then start a new Claude Code or Codex
  session. Enable it in **Settings → Continuation**.

## Privacy and your data

Hession reads original histories without changing them. Its local library is
`~/.hession/sessions.sqlite`; it remains usable offline. There is no Hession
account or telemetry. SSH imports contact your configured hosts. Price downloads
from models.dev and update checks/downloads from GitHub are user-initiated.

Private sync encrypts selected history before sending it to your relay.
Continuation prepares context locally; the destination harness may send that
context to its normal model provider. Local libraries, handoff files, and ordinary
backups and project ZIPs contain plaintext. Review archive previews before sharing. See [security and limits](docs/user-guide.md#security-and-privacy).

## Guides

- [User guide](docs/user-guide.md): histories, search, terminal/MCP, sync, continuation, privacy, and backups.
- [Relay hosting](docs/self-hosting.md): deployment, access, and recovery.
- [Development](docs/development.md): architecture, checks, and releases.
- [Sync protocol](docs/sync-protocol.md) · [Changelog](CHANGELOG.md).

MIT licensed; see [LICENSE](LICENSE).
