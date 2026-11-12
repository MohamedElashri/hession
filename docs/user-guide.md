# User guide

For installation, see [Install](../README.md#install).

## Import histories

**Refresh histories** imports new and changed local/SSH sessions. It also runs at
startup unless disabled in its menu. The same menu offers a per-agent refresh
and **Full rescan**. Stopping keeps completed imports; original files are unchanged.

To add a remote machine, choose **+** beside **Hosts**, select an SSH suggestion
or enter an alias or `user@host`, then **Test** and refresh. Both machines need
`rsync`; this computer needs `ssh` and a login that works without prompts:

```sh
ssh -o BatchMode=yes <host> true
```

Your SSH configuration is respected. Uncheck a host to pause imports. Copied
history stays available while a host is offline or removes its original files.

## Find and read sessions

- The main search covers transcript text, titles, and workspace paths. Use
  sidebar and toolbar filters to narrow it. Words match prefixes, ignore case
  and accents, and match **any** word in a multiword search.
- **Search this session** searches the complete history, including tool inputs
  and output. Open a result to jump to it in **Full log**; Escape clears results.
- **Conversation** groups prompts, replies, and tools. Expand tool calls,
  thinking, pasted context, and long messages as needed.
- **Prompts** lists prompts on the current page. **Full log** includes events
  and role filters. Long histories use pages of 500 entries.
- Star sessions or add tags above the transcript. Multiple selected tags require
  every tag. **Export** saves a session, filtered results, or the whole library
  as Markdown or JSON, optionally with statistics.

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

- **Appearance** (palette icon): theme, accent, text size, spacing, code wrapping,
  and reduced motion. Changes are saved per device; **Reset to defaults** resets them.
- **Settings → Terminal → Choose terminal**: automatic, discovered, or custom
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

The **database icon → Export SQLite backup** creates a restorable library copy;
filter it if needed. **Import backup** can preserve or replace matching sessions;
review its preview first. Markdown/JSON transcript exports cannot be restored.
Backups exclude sync queues, credentials, device settings, and handoff files.
Keep the sync recovery key separately.

**Delete / reset…** removes indexed data, not original agent files or relay
records. Deleted histories can return on the next refresh or sync. Back up first.

**Software update → Check for updates → Download and install** verifies a signed
release and preserves your installation format. Closing the dialog keeps the
update running. Finish refresh/sync and saved-data operations before restarting.
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
