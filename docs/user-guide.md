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
  thinking, pasted context, and long messages as needed.
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
service; browser preview shows an availability message.

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
