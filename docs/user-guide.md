# User guide

## Syncing

**Sync updates** imports new and changed sessions. It also runs at startup;
you can turn that off in the sync menu. Unchanged files are skipped by size and
modification time. Use **Full rescan** in the same menu to re-read everything.
Stopping a sync keeps everything that was already imported.

## Remote hosts

Click the server icon next to **Hosts** in the sidebar.

- **Add** a host from the suggestions (folders you opened with VS Code
  Remote-SSH, or entries in `~/.ssh/config`), or type an SSH alias or
  `user@host`.
- **Test** connects to the host and lists the agent histories it found.
- Uncheck a host to pause it without removing it.

On each sync, every enabled host is contacted with your own `ssh`, and its
agent histories are copied with `rsync` into
`~/.hession/remotes/<host>/`. Only transcript files are copied (`.jsonl`,
`.json`, `.db`), and nothing on the host is changed. Copies are never pruned,
so sessions remain even after the agent deletes old history on the host.

**Requirements**

- `ssh` and `rsync` on this computer, and `rsync` on the host
- A login that doesn't prompt: SSH keys, an agent, or Kerberos. The app never
  asks for passwords.
- Your `~/.ssh/config` is used as is, so `ProxyJump`, `ControlMaster`, and
  similar options all apply.

A host that can't be reached (off VPN, expired ticket) is reported after the
sync, and its previously copied sessions stay available.

In the library, filter by **This computer** or a host name. Remote sessions
show a host label, and their workspaces appear as `host:/path`.

## Resume and VS Code

**Resume** opens a terminal and runs the agent's own resume command in the
session's original folder. For a remote session it runs `ssh -t <host>`, moves
into the recorded folder, and starts the agent with your login shell, so
`nvm`, `conda`, and `~/.local/bin` are available. When the agent exits, you're
left in a shell on that host. The dialog warns if the agent isn't installed
there. Use **Copy command** to paste the command into your own terminal.

Your choice of terminal app is remembered. On Linux, `$TERMINAL` is tried
first. Remote resume works from Linux and macOS.

**VS Code** opens the session's folder: locally with `code <folder>`, and on a
remote host through Remote-SSH. This needs the `code` command on your `PATH`;
in VS Code, run *Shell Command: Install 'code' command in PATH*.

## Supported agents

| Agent | Default location (macOS / Linux) | Override |
| --- | --- | --- |
| Claude Code | `~/.claude/projects` | `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/sessions` | `CODEX_HOME` |
| OpenCode | `~/.local/share/opencode/opencode.db` | `XDG_DATA_HOME` |
| Cline | `~/.cline/data`, VS Code (and Server) extension storage | `CLINE_DIR`, `CLINE_DATA_DIR`, `CLINE_STORAGE_DIR` |
| Pi | `~/.pi/agent/sessions` | `PI_CODING_AGENT_DIR` |
| Command Code | `~/.commandcode/projects` | `COMMANDCODE_HOME` |
| Muse | `~/.local/share/muse/sessions` | `XDG_DATA_HOME` |
| Hermes Agent | `~/.hermes/state.db`, `profiles/*/state.db` | `HERMES_HOME` |
| Antigravity | `~/.gemini/antigravity/brain` | `ANTIGRAVITY_HOME` |

Overrides are read from the environment that launches the app, and on remote
hosts from the host's own environment. Source files are only ever read.

Cline task folders created by the editor extension can't be resumed from the
CLI; resume them from *Cline → History* in the editor.

## Bookmarks, tags, and export

- **Star** a session with the icon on its card.
- **Tags** are added above the transcript. Selecting several tags matches
  sessions that have all of them.
- **Export** saves the selected session, the current filtered view, or the
  whole library as Markdown or JSON. You can optionally include stats such as
  tokens, cost, and duration.

## Your data

Everything lives in `~/.hession/` (set `HESSION_DATA_DIR` to change this):

| Path | Contents |
| --- | --- |
| `sessions.sqlite` | The library: transcripts, tools, stars, tags, prices |
| `remotes.json` | Your remote host list |
| `remotes/<host>/` | Files copied from each host |

Open the **database icon** (next to the theme toggle) to back up the database,
restore a backup, or delete sessions matching a filter. Deleted sessions come
back on the next sync while their source files still exist. Backups and copies
contain your prompts and code, so treat them as private.

## Troubleshooting

| Problem | Fix |
| --- | --- |
| Host "could not be reached" | Run `ssh -o BatchMode=yes <host> true` in a terminal; it must succeed without prompting |
| "rsync is not installed on the host" | Install `rsync` on the host |
| Remote resume says the agent wasn't found | Make sure the CLI is on `PATH` in your login shell on the host |
| VS Code button fails | Install the `code` command (see above) and the Remote-SSH extension |
| Costs look wrong | Adjust model prices in the **Pricing matrix** in **Analytics** |
