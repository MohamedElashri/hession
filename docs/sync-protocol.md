# Private sync protocol v1

The desktop client uses the standalone `hession-sync` crate for portable
snapshots, chunk encoding, record authentication, and encryption. The relay
stores opaque records and never needs the library key. All JSON HTTP fields use
camelCase. Production clients require HTTPS; HTTP is accepted only for loopback.

## Relay API

| Method and path | Authentication | Purpose |
| --- | --- | --- |
| `GET /v1/status` | None | `{protocolVersion:1, initialized}` |
| `POST /v1/bootstrap` | Administrator bearer | `{deviceId,deviceName}` → `{vaultId,deviceId,deviceToken}`; once only |
| `POST /v1/invitations` | Device or administrator bearer | Single-use `{invitation,expiresAt}` |
| `POST /v1/devices/join` | Invitation in body | `{invitation,deviceId,deviceName}` → enrollment credentials |
| `GET /v1/devices` | Device or administrator bearer | Device registrations |
| `DELETE /v1/devices/{id}` | Device or administrator bearer | Access revocation only; no encryption rotation |
| `POST /v1/records` | Device bearer | `{records:[{id,deviceId,keyEpoch,payload}]}` → `{accepted}` |
| `GET /v1/records?after=N&limit=L` | Device bearer | `{records:[{sequence,id,deviceId,keyEpoch,payload}],cursor,hasMore}` |

An administrator bearer cannot read/write history records. An upload's writer
must match the authenticated device. Record IDs are immutable: an exact retry
succeeds and conflicting data under an existing ID is rejected. The relay
assigns contiguous sequence numbers within its persistent database. Each feed
record must have the previous sequence plus one; gaps and inconsistent cursors
are rejected before the local checkpoint commits.

The current relay bounds encoded payloads to 4 MiB, request bodies and feed
pages to 8 MiB, upload batches to 32 records, and feed limits to 100 records.
The desktop sends one record per upload and requests 32 records per feed page.
`expiresAt` and registration `createdAt` values are Unix seconds.

## Encrypted envelopes

`payload` is standard base64 of a JSON envelope:

```json
{"version":1,"nonce":"base64-24-bytes","ciphertext":"base64-ciphertext-and-tag"}
```

The plaintext is a JSON `SnapshotChunk` with `version`, `snapshotId`, `index`,
`total`, and base64 `data`. Each chunk carries at most 512 KiB of snapshot bytes.
At most 256 chunks form a 128 MiB snapshot. Chunks are independently encrypted,
authenticated, and checkpointed; reassembly resumes after interruption.

AEAD additional data is UTF-8 JSON encoding of the tuple:

```json
["hession-sync-v1","vault-id","record-id","device-id",1]
```

Only key epoch 1 is currently supported. Do not increment epochs without a key
distribution and migration protocol; v1 has no such key-rotation mechanism.

## Snapshot and identities

A `Snapshot` contains version 1, portable `sessionId`, `revisionId`, optional
`parentRevision`, and `content`. Content includes allowlisted session metadata,
stars/tags, recorded branch/commit/model hints, normalized user/assistant text,
tool names/status, and explicit omissions. Unknown fields and unsupported
versions are rejected. The optional context fields default to absent for older
v1 snapshots; permissions and raw harness context never transfer.

Native identities derive from a domain-separated SHA-256 of harness family,
native session ID, and recorded start time. Sources without native IDs receive
a persisted random ID. Downloaded synthetic source URIs preserve the portable
identity across backup/restore. Aliases with different source metadata can still
produce duplicates; v1 does not silently reconcile uncertain identity matches.

Content digests and revision lineage remain inside the encrypted payload or
local database. Identical local snapshots are not uploaded repeatedly. Divergent
snapshots materialize as branches; only a remote copy whose exact head matches
the incoming parent advances in place. Native-source rows are never overwritten.

## Reliability and local state

Local `replication_identity`, `replication_revision`, `replication_outbox`,
`replication_publication`, `replication_checkpoint`, and `replication_chunk` tables
are additive and separate from the source-import checkpoints. Outbox records are
persisted before sending. Upload acknowledgments remove only acknowledged IDs.
Feed checkpoints advance in the same transaction as imported chunks/snapshots.
Downloaded snapshots are marked published locally to avoid echo uploads.

Outbox rows retain their original workspace scope separately from mutable
session metadata. Before uploading, the client verifies that both the original
scope and current live session identity remain selected. Excluded pending
records and their corresponding publication markers are canceled; re-opting in
can queue the current snapshot again. An empty upload scope never flushes an old
outbox. Before advancing a downloaded head, locally changed content is retained
as a local-only revision even when uploads for that project are disabled.

Remembered revision IDs only suppress a replay when a live matching portable
session's current revision ancestry includes them. Deleted or reused SQLite row
IDs do not silently discard restored history. Portable database exports strip
all replication tables, and destructive maintenance resets affected replication
state so the relay can be replayed safely. Device sync holds the same maintenance
gate as source imports, saved-data operations, and update restart.

The client first authenticates/decrypts one existing relay record without
applying it. It then captures local edits durably before applying downloads, so
incoming history cannot overwrite unrecorded offline work. Only afterward does
it push and pull again. A wrong recovery key fails before uploads or outbox
creation in a non-empty vault. Empty-vault key verification relies on comparing
fingerprints. HTTP requests time out and do not follow redirects. Cancellation
is checked between transfer steps; an already in-flight request may finish.

Explicit local replay resets checkpoints/chunks/publication markers after an
older relay backup is restored; queued ciphertext is preserved. When re-pairing
changes the device ID, pending chunks are authenticated with the original
library key and re-encrypted with fresh record IDs/nonces bound to the new
writer. Snapshot IDs and content remain unchanged. Chunk assembly accepts this
writer transition, while rejecting conflicting data at the same chunk index.

This version has no remote deletion/tombstone protocol, convergent per-field
annotation edits, global rollback detection, or cryptographic writer signatures
beyond authenticated shared-library encryption and relay device authentication.
