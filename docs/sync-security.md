# Security and privacy

Private sync and continuation are **separate opt-ins, disabled by default**.
There is no Hession account or telemetry. Restoring a portable library backup
does not enable either feature or import sync credentials.

## Where your data goes

| Location or action | Privacy boundary |
| --- | --- |
| Local library, original histories, handoff files, ordinary backups | Plaintext on your device. Protect your device and backups. |
| Private relay | Selected histories are encrypted locally with XChaCha20-Poly1305 and random keys/nonces. HTTPS protects transport and device credentials. |
| Relay administrator | Can observe device membership, addresses, record sizes, and timing, and can withhold or lose records. Cannot decrypt history without the library key. |
| Continuation | Preparation stays local. The destination harness can send reviewed context to its normal model provider after launch. |

Sync omits reasoning, system/developer instructions, tool inputs/results, raw
harness data, and attachments. Known credential patterns are redacted; arbitrary
secrets in visible conversation may remain. Select upload workspaces deliberately.
Continuation has its own preview and optional tool context; inspect that package too.

## Keys and access

- Sync keys and device tokens stay in macOS Keychain or Linux Secret Service;
  there is no plaintext credential-file fallback. The relay never receives the
  recovery key; Hession does not retain the administrator setup token.
- Keep the recovery key separately from relay backups. Anyone holding the key
  and encrypted records can decrypt them. Losing every key copy makes recovery
  impossible. Compare fingerprints when enrolling another device.
- **Disable**, **disconnect**, local deletion, and server access revocation do
  not erase downloaded history or rotate a device's retained encryption key.
  A compromised key requires a new library/key and trusted-device enrollment.
- A compromised device can read its local plaintext and keys in use. Hession
  cannot protect against that endpoint being compromised.

## Limits of this release

The initial protocol has **not had an independent security audit**. It detects
modified ciphertext and authenticated metadata, but does not guarantee detection
of server rollback, selective withholding, or forked views. Keep independent
backups. Remote deletion, key rotation, and global rollback detection are absent.

Downloaded sessions are data, not executable instructions. They cannot use
native Resume; continuation needs separate enablement, review, and launch.
Portable backups exclude sync queues and partial-download buffers; manually
copying the live database includes private local state.

Dependency scans are not a security audit. Known upstream dependency warnings
and scan details are in [Development](development.md#dependency-security);
protocol details are in [Sync protocol](sync-protocol.md).
