# Private device sync

Sync is **optional, manual, and off by default on each device**. It copies
selected histories through your own encrypted relay while Hession is open.
Local/SSH refresh and continuation work independently.

## What you need

- A relay and HTTPS address: follow [Self-hosting](self-hosting.md). Loopback
  HTTP is allowed only for testing on the same computer.
- An unlocked credential store: macOS Keychain or Linux Secret Service
  (such as GNOME Keyring or KWallet's Secret Service integration). Setup stops
  if secure storage is unavailable; there is no plaintext credential fallback.

## First device

1. Open **Settings → Private sync → Create library**. Enter the relay address,
   device name, and relay setup token. **Test connection** sends no history.
2. Select workspaces that may upload, then **Preview selected histories**.
   Empty selection means download-only. Scope includes existing indexed history;
   SSH workspaces are separate choices. The estimate excludes protocol overhead.
3. Choose **Create encrypted library**. Save the recovery key separately from
   the relay and confirm **I have saved my recovery key**. Hession does not
   retain the relay setup token.
4. Choose **Enable device sync**, then **Sync now**. Setup and enablement alone
   do not transfer history.

## Another device

1. On an enabled trusted device, choose **Pair another device**. Invitations
   are single-use and expire after ten minutes.
2. On the other device, choose **Join library**. Enter the same relay address,
   a device name, the invitation, and the recovery key through a trusted channel.
3. Select upload workspaces, **Preview selected histories**, then **Connect
   this device**. Leave the selection empty for download-only use.
4. Compare the library fingerprint with your trusted device and confirm it
   matches. Choose **Enable device sync**, then **Sync now**.

The invitation grants relay access; the recovery key decrypts history and is
never sent to the relay. Verify fingerprints even if the library is empty.

## What transfers

| Included, encrypted | Omitted |
| --- | --- |
| Selected sessions' user/assistant text, titles, paths, timestamps, recorded Git/model hints, stars, tags, and tool names/status/exit codes | Project files, uncommitted changes, tool inputs/results, reasoning, system/developer messages, attachments, credentials, terminal settings, and feature preferences |

Known secret patterns are redacted, but arbitrary secrets in conversation text
may remain. Choose your scope carefully. Local copies and ordinary backups are
plaintext; see [Security](sync-security.md).

Downloaded sessions appear in Hession, not native agent stores. Enable
**Continue in…** separately to work from them. Offline or concurrent versions,
including conflicting stars/tags, are kept as separate branches.

## Pause, change scope, or disconnect

- **Stop** preserves completed work; an in-flight request may finish. Retry
  **Sync now** after an interruption or outage.
- **Disable device sync** pauses transfers and lets you change upload scope.
  Preview before enabling again. Unsent data for excluded workspaces is canceled;
  history already on the relay remains.
- **Disconnect relay** requires sync to be disabled and stopped. It removes
  this device's local relay settings and credentials, keeping indexed history.
  It does not revoke server registration or erase relay copies.

Finish sync before refreshing histories, backing up/restoring, deleting data,
or restarting for an update. Those operations cannot run together.

## Recovery and current limits

| Situation | Action |
| --- | --- |
| Need another key copy | **Show recovery key** while the OS credential store is unlocked. Save it separately from server backups. |
| Setup interrupted | Setup stays disabled with its key saved securely. Show and save that key before disconnecting. If the relay already initialized, use an administrator-created invitation and join with the saved key. |
| All devices lost | Restore relay access with its administrator token, create an invitation, and join using your saved recovery key. Without any key copy, encrypted history cannot be recovered. |
| Older relay backup restored | Disable sync, open **Repair after restoring a relay backup → Replay after relay restore**, preview and re-enable, then sync. Surviving devices republish selected history; data absent from both devices and backup cannot be recovered. |
| Device or key compromised | Revoke relay access through the server API. Previously downloaded data remains readable; a compromised key requires a new library/key and trusted-device enrollment. See [server management](self-hosting.md). |

Version 0.4.0 has no automatic/background sync, remote deletion, encryption-key
rotation, or automatic field-by-field annotation merging. Local deletion does
not erase relay history. The initial sync protocol has not had an independent
security audit. See [Security](sync-security.md) and [server limits](self-hosting.md).
