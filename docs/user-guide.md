# User guide

For installation, see [Install](../README.md#install).

## Import histories

**Refresh histories** imports new and changed local/SSH sessions. It also runs at
startup unless disabled under **Settings → History refresh**. Its menu offers a per-agent refresh
and **Full rescan**. Stopping keeps completed imports; original files are unchanged.

Hession also indexes **local** changes automatically while its desktop window is
open. Pause or resume it in the sidebar or **Settings → History refresh**. This
preference is saved on this computer; an existing startup-refresh opt-out also
disables automatic indexing until you explicitly resume it. Settings shows the
source roots, unavailable stores, last successful import, and recent errors.
Stores for agents you have not installed can be absent. No operating-system
daemon is installed. SSH mirrors and private device transfers keep their separate
explicit refresh/sync actions.

Claude Code and Codex append complete JSONL lines without rebuilding earlier
entries. Incomplete final lines wait for completion; malformed complete lines
preserve the previous import and report an error. Replaced or rewritten files and
other adapters use a safe rebuild. Deleted saved histories stay excluded from
automatic imports; **Refresh histories** or **Full rescan** can import native
sources again. Removing saved history never deletes native agent files.

To add a remote machine, open **Settings → Remote hosts** (or **+** beside **Hosts**), select an SSH suggestion
or enter an alias or `user@host`, then **Test** and refresh. Both machines need
`rsync`; this computer needs `ssh` and a login that works without prompts:

```sh
ssh -o BatchMode=yes <host> true
```

Your SSH configuration is respected. Uncheck a host to pause imports. Copied
history stays available while a host is offline or removes its original files.

## Find and read sessions

- **Developer search** uses AND between words, quoted phrases, uppercase OR,
  and `-exclusions`. Filters include `agent:`, `project:`, `file:`, `tool:`,
  `branch:`, `host:`, `after:`, `before:` and `has:errors`. Quote field values
  with spaces. Dates use `YYYY-MM-DD` in UTC: after includes the date; before
  excludes it. Code such as `C++`, `foo::bar`, and paths keeps its punctuation.
  Query syntax errors identify their byte position. Parenthesized groups and
  regexes are unsupported.
- Developer results rank by relevance. Choose **Sessions** to find words across
  a session, or **Messages** to require text terms in the same entry. Open a
  result to jump to its matching entry in **Full log**. Metadata-only matches
  open the session. Sidebar and toolbar filters, stars, and tags still apply.
  If the library changes between pages, use **Restart search** for fresh results.
- **Legacy search** preserves the earlier prefix and any-word matching over
  transcripts, titles, and workspace paths, with its existing sort choices.
  It is also available in the browser preview; developer search needs the
  desktop SQLite service. **Query syntax** beside developer results provides help.
- **Search this session** searches the complete history, including tool inputs
  and output. Open a result to jump to it in **Full log**; Escape clears results.
- **Conversation** groups prompts, replies, and tools. Expand tool calls,
  thinking, pasted context, and long messages as needed. Your messages have a
  tinted **You** card; replies have an **Agent** marker. **Tool activity** uses
  separate cards with the tool name and recorded outcome. Failed calls remain
  visible when long tool runs are folded.
- Tool cards separate **Command** or **Code**, **All arguments**, and **Output**.
  **Wrap lines** switches between wrapped lines and horizontal scrolling. **Copy**
  copies the complete panel text, including text outside a large-content preview.
  **Show all** reveals the rest inside a scrollable panel; **Show preview** folds
  it again. Full log and expanded Live events use the same controls.
- Tool panels highlight shell commands, code with a recorded language, JSON, and
  unified diffs. Use the language menu beside **Wrap lines** to select a language
  or **Plain text**. Fenced Markdown code uses its language tag. Plain logs stay
  uncolored; large blocks and unusually long lines use plain text to keep reading
  responsive. Highlighting runs locally and preserves the exact text used by Copy.
- **Prompts** lists prompts on the current page. **Full log** includes events
  and role filters. Long histories use pages of 500 entries.
- Star sessions or add tags above the transcript. Multiple selected tags require
  every tag. **Export** saves a session, filtered results, or the whole library
  as Markdown or JSON, optionally with statistics. Filtered transcript exports
  use Legacy search; developer queries support selected-session or whole-library
  exports until shared project export selection is available.

## Live local activity

**Live** passively shows locally recorded sessions and their latest events. Filter
by agent, project, or the last 15 minutes; choose **All recorded activity** for
older histories. **Recently updated**, **Quiet**, **May be stale**, and
**Unavailable** describe recorded evidence. They do not prove an agent process
is running, blocked, awaiting approval, or finished. SSH histories stay in the
normal library and update through manual history refresh.

Select a session to read its live event window. Earlier tool calls update in
place when their recorded results arrive. Source rewrites replace the window
with a bounded tail, close expanded cards, and display a notice. Ordinary late
updates preserve expanded cards. Collapsed previews show conversation text or
event metadata; expand a card to inspect other recorded content. Open **Full history** to read the
complete indexed transcript or jump to an event's actual row position.

Following starts off. Choose **Follow latest events** to scroll with new events;
scrolling away turns following off. A new-event button offers a one-time jump
without enabling following. **Pause live updates** pauses this reader only;
automatic indexing is unchanged. **Pause automatic indexing** separately changes
the existing indexing preference. Indexing errors and recovery polling appear
in the Live header.

Each delta contains at most 100 events. The reader retains at most 300 events
and approximately 4 MiB of serialized content; older events remain available in
full history. Polling and local change notifications are coalesced, so rapid
updates do not queue unlimited requests. Live requires the desktop SQLite
service; browser preview shows recorded sample data and does not monitor native stores.

## Keyboard navigation

Open **Commands** in the sidebar or press **Cmd+K** on macOS / **Ctrl+K** on Linux.
The palette finds commands, indexed sessions, and workspaces. Session search is
debounced and returns up to 12 sessions; workspace matches are limited to 12.
Use ↑/↓, Home/End and Enter to choose a result. Escape closes the palette and
restores your previous focus. Unavailable commands explain what is needed.
Choose **Keyboard shortcuts** in the sidebar, palette, or press **?** for the
current reference, including platform-specific modifier labels.

| Key | Action |
| --- | --- |
| Cmd/Ctrl+K | Open command palette |
| / | Focus global session search |
| Cmd/Ctrl+F | Search the complete selected session |
| Cmd/Ctrl+B | Show/hide navigation |
| Cmd/Ctrl+, | Open Settings |
| Cmd/Ctrl+Shift+L | Focus the session list |
| Cmd/Ctrl+Shift+T | Focus the transcript for scrolling |
| j/k or ↓/↑ | Move focus in the session list; Enter opens the focused session |
| Home/End | Focus the first/last session while in the list |
| ? | Show keyboard shortcuts |
| Escape | Dismiss a menu or dialog when its operation permits |

Tab reaches the sidebar, session list, transcript, and their controls. In the
sidebar, ↑/↓ and Home/End move between navigation buttons. Once the transcript
is focused, use arrows, Page Up/Down, and Home/End to scroll. Palette commands
also change transcript pages, open export, show analytics, or prepare Resume.
Resume and continuation still require their usual review before launching.
Letter shortcuts are inactive in text fields, editors, selection controls, and
dialogs. Cmd/Ctrl+K, Cmd/Ctrl+F, and the focus shortcuts remain available while
typing outside dialogs.
Shortcuts ignore input composition and held-key repeats. Motion preferences
apply to keyboard navigation too.

## Project and file evidence

Choose **Files and Git** in the sidebar, filter the recorded workspaces, and
select one to browse reported paths. Filter filenames or select a path for its
individual history. **Open session…, event…** jumps to the source event in Full
log, including the correct page in long sessions. **Inspect local Git** runs
only when selected; **Refresh recorded history** reloads indexed evidence.

Projects retain the recorded workspace identity. A local folder, an SSH-hosted
folder at the same path, and sibling Git worktrees remain distinct. Recorded
history stays available when its checkout is deleted or inaccessible. Synced-only
history does not authorize inspecting or opening a matching local folder.

File activity links reported paths to their source session, event and tool. A
reported read records a tool's target; a reported write or patch records intent.
Even a successful tool result does not prove that the final file changed.
Repeated references from the same event and tool count once in file summaries;
the activity history preserves every evidence row. Suspicious or unresolved
paths remain visible as evidence and cannot be opened through traversal or a
symlink escape.

Recorded branch and commit metadata describe the session's saved context.
Explicit local Git inspection shows current checkout metadata separately. It
never contacts an SSH host or a Git remote and never attributes commits to an
agent. Verified commit changes come from available commit objects. Observed
worktree changes compare tracked files' raw bytes with their index blobs;
Git attribute conversions and clean filters are not applied. Untracked files,
symlink targets and submodule working trees are not scanned. Large inspections
return a bound or availability error rather than silently claiming a complete
result; an incomplete tracked-path scan is explicitly marked. Partial clones
are unavailable for object inspection to prevent lazy network fetches.

## Portable project ZIP archives

Project ZIP export prepares a preview of the exact project and developer-search
selection, including session counts, an approximate uncompressed size, detected
redaction patterns, options and the intended destination. Review the preview
before saving. A library change requires a new preview. Choose a new filename:
project archives never replace an existing file, including a native agent log.

Markdown and JSON transcripts share portable session and snapshot references
with `activity/files.json` and the versioned `manifest.json`. File references
remain reported evidence; exporting does not verify agent authorship or copy a
repository tree. Optional Git metadata is recorded session context only. Export
never runs Git, refreshes SSH mirrors, downloads prices or contacts a remote.
Optional statistics use the cached values captured during preview.

Tool inputs and outputs, absolute machine paths, Git metadata and statistics
are off by default; secret-pattern redaction is on. Private reasoning,
system/developer messages, raw event payloads, attachments, source URIs and
repository remote credentials are always excluded. Absolute metadata fields
are omitted by default; freeform transcript paths are masked conservatively.
Pattern redaction cannot find every secret or encoded path. Inspect the archive
before sharing; ZIP compression does not encrypt it.

Cancellation before publication removes the temporary archive and leaves the
destination absent. If saving wins the finalization race, the complete archive
is kept and cancellation reports that it is already saved. The archive is a
portable reading format and cannot replace a SQLite backup for restoration.

## Resume or continue a task

| Action | Result |
| --- | --- |
| **Resume** | Reopens the original session in its own CLI and folder, locally or over SSH. Use **Copy command** if you prefer your terminal. |
| **VS Code** | Opens the recorded local folder or an SSH workspace through Remote-SSH. Install the `code` shell command and, for remote folders, the extension. |
| **Continue in…** | Starts a new Claude Code or Codex session from reviewed context. Enable **Settings → Continuation** first; see [Continuation](continuation.md). |

The destination CLI must be installed and signed in. Cline editor-extension
histories resume through **Cline → History** in the editor. Synced library
copies use **Continue in…**; they are not native harness sessions.

## Sync between devices — optional

**Settings → Private sync** connects to your self-hosted relay. Select upload
workspaces, preview, save your recovery key, and explicitly enable sync. Empty
selection means download-only. Transfers run only when you choose **Sync now**.
Each other device joins with an invitation and recovery key, verifies the
fingerprint, and enables separately. Local/SSH refresh and continuation are independent.

See [Private sync](private-sync.md) for setup, pause, recovery, and limitations,
and [Self-hosting](self-hosting.md) for Docker and server backups. Sync does not
copy repository files or install history into agent stores.

## Preferences and analytics

Open the single **Settings** button in the sidebar. All preferences, remote hosts,
saved data, and **About** live in this window. Switch sections without losing
unfinished edits; use **Save** where shown.

- **Settings → Appearance**: theme, accent, text size, spacing, code wrapping,
  and reduced motion. Changes are saved per device; **Reset to defaults** resets them.
- **Settings → Terminal**: automatic, discovered, or custom
  executable. **Discover again** finds newly installed terminals. Resume can
  override the saved choice and warns if it becomes unavailable.
- A custom terminal needs an absolute executable path (inside the `.app` on
  macOS). Enter arguments one per line, with `{script}` exactly once as a separate
  argument. For `-e` terminals: `-e`, `sh`, `{script}`. Arguments are literal;
  do not shell-quote paths with spaces.
- **Analytics** shows tokens, estimated costs, tools, failures, and activity.
  **Sync prices** explicitly downloads models.dev rates. Change the **Pricing
  matrix** or custom overrides if your provider's prices differ. Estimates are
  not billing records; reported usage is used when available.

## Backups and updates

Use **Settings → Saved data → Export SQLite backup** to create a restorable library copy;
filter it if needed. **Import backup** can preserve or replace matching sessions;
review its preview first. Markdown/JSON transcript exports cannot be restored.
Backups exclude sync queues, credentials, device settings, and handoff files.
Keep the sync recovery key separately.

**Delete / reset…** removes indexed data, not original agent files or relay
records. Deleted histories can return on the next refresh or sync. Back up first.

Open **Settings → About** for your version and software updates.
**Check for updates → Download and install** verifies a signed
release and preserves your installation format. Leaving Settings keeps the update
running. Finish refresh/sync and saved-data operations before restarting.
If installation fails, rerun the [installer](../README.md#install); source builds
must be rebuilt. Releases predating the updater need one manual update first.

## Terminal commands and read-only MCP

Running `hession` without arguments opens the desktop. Explicit commands run
without a webview. Linux packages install `hession` on `PATH`; the AppImage
installer forwards command arguments through its launcher.
AppImage terminal commands also require the host graphics libraries listed in
[Install](../README.md#install), even when no display is available.

On macOS, use `/Applications/Hession.app/Contents/MacOS/hession` directly, or
create an optional user-owned link:

```sh
sh /Applications/Hession.app/Contents/Resources/install-cli.sh \
  /Applications/Hession.app/Contents/MacOS/hession
```

The script creates `~/.local/bin/hession`; add that directory to your `PATH`.
It refuses to replace an existing file or link. Pass a second absolute path to
choose another destination. Moving or uninstalling the app leaves the optional
link behind; remove that link yourself when it is no longer needed.

```sh
hession --help
hession status --json
hession projects list --json
hession sessions list --project workspace:1 --limit 20 --json
hession sessions show 42 --format markdown
hession sessions follow 42 --jsonl
hession search 'agent:codex "type error" -has:errors' --project workspace:1 --json
hession project show workspace:1 --json
hession files list --project workspace:1 --json
hession files history src/main.rs --project workspace:1 --json
hession index --once
hession index --watch --agent codex
hession export project workspace:1 --format zip --output ./project.zip --preview --json
hession export project workspace:1 --format zip --output ./project.zip --json
```

Read commands use the existing local catalog. They never create or migrate a
library or fetch remote history. Select a different existing database with
`--database /absolute/path/sessions.sqlite`; otherwise `HESSION_DATA_DIR` and
then `~/.hession` determine its location. `index` is an explicit local indexing
operation; watch stays in the foreground until interrupted. Follow reads durable
recorded updates, including changed tool status; it does not establish that an
agent process is running. Stop follow/watch with Ctrl+C. Project export writes a
new ZIP destination and refuses to overwrite one. Defaults redact common secret
patterns and exclude tool bodies, machine paths, Git metadata and statistics;
review the preview and use explicit flags to include those fields.

Project selectors are exact indexed paths or `workspace:ID` values returned by
`projects list`. They do not infer the current directory, combine worktrees, or
resolve symlink aliases. File-history paths are exact recorded references. A
reported external path does not cause Hession to open the file.

Pages accept `--limit` (1–100) and `--cursor`. Reuse the same request and page size
with a returned cursor. Any catalog mutation can expire it; restart from page
one. Session display includes only explicit user/assistant text, bounded to 2048
characters from 16 text parts per entry. Structured CLI search defaults to the
complete indexed text policy; `--metadata-only` excludes transcript matching.
Search rejects eligible text cells larger than 1 MiB and oversized metadata
before loading it; narrow the scope or repair those indexed fields if a size
limit is reported. `--json` writes one JSON value to stdout and errors to stderr. Follow's `--jsonl`
writes one JSON envelope per update. Exit codes are 0 for success, 2 for invalid
arguments/queries, 3 for unavailable libraries or I/O, 4 for changed or denied
scope, 5 for expired cursors or size limits, and 6 for execution limits/busy
workers. Follow/watch use 130 for Ctrl+C and 143 for SIGTERM.

### MCP server permissions

Configure your MCP client to launch the installed executable with a fixed,
owner-chosen allowlist. This example grants metadata access to one indexed
project; replace the ID after checking `projects list`:

```json
{
  "mcpServers": {
    "hession": {
      "command": "/Applications/Hession.app/Contents/MacOS/hession",
      "args": ["mcp", "serve", "--project", "workspace:1"]
    }
  }
}
```

On Linux use the installed absolute `hession` path as `command`. Repeat
`--project` to approve additional exact project identities. Missing or empty
scope fails closed. There is no automatic current-directory or all-project
fallback. If the database or an approved workspace identity changes, restart
the server with a reviewed allowlist.

The server exposes exactly seven read-only tools: `search_sessions`,
`get_session`, `list_projects`, `get_project_activity`, `get_file_history`,
`get_tool_calls` and `get_recent_sessions`. It serves stdio only, with no HTTP
listener, model API, agent launch, filesystem inspection, indexing or export
tool. Stdout carries JSON-RPC only.

Metadata is the default. Add `--expose-transcripts` to the server's startup
arguments only when the client should receive explicit user/assistant
conversation text from the approved projects. This also permits searching that
text. It never exposes reasoning, system/developer messages, tool commands,
arguments/results, original provider payloads, or unsafe combined legacy text.
Tool-call results include names and recorded status only; file activity excludes
free-form evidence. A per-tool request cannot grant itself more permission.
Historical text is untrusted data and may contain instructions written by others.

## Supported history stores

Paths are defaults; environment overrides must be set when launching Hession
(or on the SSH host). Hession data is separate, under `~/.hession/`; change it
with `HESSION_DATA_DIR`.

| Agent | Default store | Override |
| --- | --- | --- |
| Claude Code | `~/.claude/projects` | `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/sessions` | `CODEX_HOME` |
| OpenCode | `~/.local/share/opencode/opencode.db` | `XDG_DATA_HOME` |
| Cline | `~/.cline/data`, VS Code/Server extension storage | `CLINE_DIR`, `CLINE_DATA_DIR`, `CLINE_STORAGE_DIR` |
| Pi | `~/.pi/agent/sessions` | `PI_CODING_AGENT_DIR` |
| Command Code | `~/.commandcode/projects` | `COMMANDCODE_HOME` |
| Muse | `~/.local/share/muse/sessions` | `XDG_DATA_HOME` |
| Hermes Agent | `~/.hermes/state.db`, `profiles/*/state.db` | `HERMES_HOME` |
| Antigravity | `~/.gemini/antigravity/brain` | `ANTIGRAVITY_HOME` |

The data directory holds `sessions.sqlite`, `remotes.json`, copied files under
`remotes/`, terminal/feature/sync settings, and `handoffs/`. Local history and
backups contain private prompts and code; protect them as you would your projects.

## Troubleshooting

| Problem | Try |
| --- | --- |
| Missing or stale history | Clear filters, check the store path above, then **Full rescan**. |
| SSH host unavailable | Run the SSH command above; check VPN, keys, tickets, and `rsync`. |
| Agent CLI not found | Check its login-shell `PATH` locally or on the host; sign in there. |
| Terminal fails | Choose an installed terminal or correct the custom executable/arguments. |
| VS Code fails | Install its `code` command and Remote-SSH support. |
| Sync or continuation blocked | Read the warning and the relevant [sync](private-sync.md) or [continuation](continuation.md) guide. |
| Costs look wrong | Adjust **Analytics → Pricing matrix** and price overrides. |
