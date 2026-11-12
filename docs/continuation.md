# Cross-harness continuation

Continuation is **optional and off by default**. It starts a new Claude Code or
Codex session from reviewed context. **Resume** reopens the original session;
continuation does not move a running process or rewrite native history.

## Start a handoff

1. Install and sign in to the destination CLI (`claude` or `codex`). Have the
   matching checkout locally; Hession does not clone or copy project files.
2. Refresh source histories, then enable **Settings → Continuation → Enable
   cross-harness continuation**. Choose a terminal under **Settings → Terminal**.
3. Open a session and choose **Continue in…**. Select the destination harness
   and local workspace. SSH and synced sources require an explicit local folder.
4. Add a note if helpful: objective, constraints, decisions, or the next test.
   Leave tool inputs/results excluded unless needed; they may contain secrets.
5. Choose **Refresh preview** after changing options. Review repository warnings,
   omissions, and **Review exact handoff package**.
6. Choose **Continue in Codex** or **Continue in Claude Code**. The new harness
   may ask permission to read the local context file; use its own permission flow.

Preparation stays local and needs no extra model API key. After launch, the
destination harness may send the package to its configured model provider.
Review visible text, notes, and tool context for private code or credentials.

## What carries over

The package includes the latest transferable user request, recent visible
conversation, your note, source/session details, and recorded and current
workspace context. Optional tool context contains up to 12 recent calls with
shortened inputs/results; missing results are marked incomplete.

Reasoning, system/developer messages, permission state, images, attachments,
raw harness data, pending tools, and running processes are omitted. Assistant
records without separable visible text are omitted. Historical tool calls are
reference material and are never replayed by Hession.

| Limit | Maximum |
| --- | --- |
| Whole package | 80,000 characters |
| Recent conversation considered | 120 text events, up to 6,000 characters each |
| Handoff note | 8,000 characters |
| Optional tool input / result | 3,000 / 4,000 characters per call |

The latest user request is retained even after a longer run of assistant updates.
The overall package limit still applies; the preview reports shortened content.

## Workspace warnings

Check the folder, branch, commit, and uncommitted changes before launch. Matching
folder names do not establish that two repositories are the same. Changes to
indexed history, options, or detected Git state invalidate the preview.
Further edits inside dirty submodules also require a new review.

Git inspection shares a five-second deadline across the checkout and up to 31
dirty submodules, with 256,000 bytes per response. Incomplete inspection blocks
launch. Move large generated trees outside the checkout or use the project's
intended ignore rules, then refresh. Non-Git folders show a warning because
repository identity cannot be checked. These checks do not lock your checkout;
avoid simultaneous edits during a handoff.

## Synced sessions and cleanup

Synced histories are library copies, so use **Continue in…** rather than native
Resume. If the source agent is still active, the handoff starts another line of
work; it does not stop or coordinate that agent. Refresh histories afterward to
see the destination's new session. A successful terminal launch is not proof
that the harness read the package or created a session.

Packages are private local plaintext under `~/.hession/handoffs/` (or your
configured data directory). They are not included in portable library backups.
Packages older than seven days are removed on the next continuation launch.
Use **Saved handoff packages → Delete saved packages**, or **Settings →
Continuation → Delete saved handoff packages**, after the destination has read them.

Disable continuation in Settings to hide the action and block new handoffs.
Already launched agents keep running; saved packages remain until cleanup.
Private sync is controlled separately. Native transcript conversion is not
available in 0.3.0. See [Security](sync-security.md) for privacy boundaries.
