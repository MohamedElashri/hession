# Self-hosting Hession's private relay

The relay stores encrypted histories until your devices connect. It needs no
library decryption key, model-provider account, or project files. Running it
never enables sync in Hession; follow [Private sync](private-sync.md) afterward.

## Start with Docker Compose

Use a tagged Hession checkout on a Linux x64/ARM64 server with Docker Engine,
Compose, persistent disk space, and an HTTPS address. Images build from this
checkout; no published Hession image is required.

**First setup only:** create an administrator token. Preserve the existing token
when upgrading.

```sh
mkdir -p deploy/relay/secrets
chmod 700 deploy/relay/secrets
openssl rand -hex 32 > deploy/relay/secrets/admin-token
chmod 600 deploy/relay/secrets/admin-token
sudo chown 10001:10001 deploy/relay/secrets/admin-token
```

The relay runs as UID/GID 10001. File-backed secrets preserve host permissions;
use an appropriate ACL/ownership mapping if your container engine remaps users.
Keep the secret private and out of version control; supplied ignore files exclude it.

### Included HTTPS proxy

Point a domain at the server and allow inbound TCP 80/443. Create
`deploy/relay/.env` with your actual domain:

```dotenv
HESSION_RELAY_DOMAIN=sync.example.com
```

Start and check:

```sh
docker compose -f deploy/relay/compose.yaml --profile https up -d --build
curl --fail https://sync.example.com/v1/status
```

Caddy obtains/renews certificates. Before device setup, status returns
`protocolVersion: 1` and `initialized: false`. The example follows the
`caddy:2-alpine` family; pin a tested image digest if you need immutable dependencies.

### Existing HTTPS proxy

```sh
docker compose -f deploy/relay/compose.yaml up -d --build relay
```

Forward HTTPS to `127.0.0.1:8744`, or `relay:8744` from the same container network.
Preserve `Authorization`, allow 8 MiB bodies, and disable request-body logging.
Keep the HTTP port on loopback; use HTTPS or your private network for device access.

## Connect devices and manage access

Follow [Private sync](private-sync.md): create the library with the administrator
token, save its separate recovery key, preview scope, and explicitly enable.
Other devices join using a ten-minute, single-use invitation and the recovery key.
Compare fingerprints. Never put the recovery key in server or proxy configuration.

Bootstrap is one-time. Device tokens are separate; only hashes are stored. The
administrator token cannot read/write history. For recovery or revocation, the
[relay API](sync-protocol.md#relay-api) accepts administrator or active-device
bearer credentials:

| Action | Endpoint |
| --- | --- |
| Create a replacement-device invitation | `POST /v1/invitations` |
| List registrations | `GET /v1/devices` |
| Revoke a device | `DELETE /v1/devices/{id}` |

Keep credentials out of process arguments, shell history, and request logs.
Revocation invalidates outstanding invitations and blocks future relay access;
it cannot erase downloaded history or rotate the shared key. If a key is
compromised, create a new library/key and enroll only trusted devices.
Losing all devices requires both administrator access and the saved recovery key.

## Back up and restore

While the relay is running, use a new backup filename each time:

```sh
docker compose -f deploy/relay/compose.yaml exec relay \
  hession-relay backup /data/relay-backup.sqlite
mkdir -p deploy/relay/backups
docker compose -f deploy/relay/compose.yaml cp \
  relay:/data/relay-backup.sqlite deploy/relay/backups/relay-backup.sqlite
```

The command refuses overwrites. Store verified backups off-server; remove
unneeded temporary copies from the volume. Do not copy a running `relay.sqlite`
alone because recent writes may be in its WAL. Alternatively, stop the server
and back up the whole volume. Save the administrator token separately, and the
library recovery key separately from both.

To restore on Linux Docker Engine without UID remapping, preserve the original
volume and use a clean one:

```sh
docker compose -f deploy/relay/compose.yaml stop relay
docker volume create hession-relay-restored
relay_restore_dir=$(docker volume inspect --format '{{.Mountpoint}}' hession-relay-restored)
sudo install -d -o 10001 -g 10001 -m 700 "$relay_restore_dir"
sudo install -o 10001 -g 10001 -m 600 \
  deploy/relay/backups/relay-backup.sqlite "$relay_restore_dir/relay.sqlite"
```

Use an unused volume name. For a remote, VM-based, or user-remapped engine,
copy into the volume on that engine and set container UID 10001 ownership
instead of using its mountpoint as a local host path.
Create `deploy/relay/restore.yaml`:

```yaml
volumes:
  relay-data:
    name: hession-relay-restored
```

Restart with the existing administrator token and the override:

```sh
docker compose -f deploy/relay/compose.yaml -f deploy/relay/restore.yaml up -d relay
```

Keep that override on subsequent commands. Check `/v1/status` and a trusted
device before discarding the old volume. In Hession, disable sync, choose
**Repair after restoring a relay backup → Replay after relay restore**, then
preview, enable, and sync. This reconciles surviving local history with the
backup; records missing everywhere cannot be recovered.

## Upgrade or move servers

Back up first. Check out the new release tag and rebuild without removing volumes:

```sh
docker compose -f deploy/relay/compose.yaml up -d --build relay
```

For the included proxy, retain `--profile https`; retain any restore override.
**Do not use `down -v` on a library you want to keep.** Incompatible newer schemas
are rejected. Roll back schema changes using a compatible backup. To move servers,
stop writes, restore the final backup and administrator token on the new server,
and update the HTTPS address on clients. Keep the same library recovery key.

## Limits

| Limit | Default |
| --- | --- |
| Ciphertext quota | 1 GiB, excluding SQLite overhead |
| Payload / request / feed page | 4 MiB / 8 MiB / 8 MiB |
| Upload batch / download page | 32 / up to 100 records |
| Concurrent requests | 16 |
| Device identities / active invitations | 256, including revoked devices / 32 |
| Client session snapshot | 128 MiB after serialization; chunked for transfer |

Set `HESSION_RELAY_QUOTA_BYTES` in `deploy/relay/.env` and recreate the relay to
change its quota. Leave room for database overhead and backups. There is no
automatic pruning, remote deletion, or encryption-key rotation. See [Security](sync-security.md).

## Standalone server and testing

```sh
cargo build --locked --release -p hession-relay
./target/release/hession-relay --help
```

Set `HESSION_RELAY_ADMIN_TOKEN_FILE` and `HESSION_RELAY_DATA_DIR`; bind defaults
to `127.0.0.1:8744`. Run as an unprivileged service behind HTTPS.

With Docker Engine and Rust 1.90+, `make test-relay-container` builds and tests
the image using disposable volumes, loopback ports, and synthetic credentials.
It covers authentication, encryption, hardening, limits, restart, and backup/restore,
but not public TLS issuance. To retest a built image:

```sh
cargo test --locked -p hession-relay --test container -- --ignored --nocapture
```

`HESSION_RELAY_TEST_IMAGE` selects another tag; default is `hession-relay:test`.

## Troubleshooting

| Problem | Check |
| --- | --- |
| Cannot read token | Container UID 10001 can read the mounted secret. |
| Already initialized | Join with an invitation; keep the existing database. |
| Authentication fails | Device revocation and credentials; re-enroll with a fresh identity if needed. |
| Quota reached | Disk space and quota setting; older records are not pruned. |
| TLS/connection error | DNS, certificate, proxy, firewall, VPN. Keep certificate verification enabled. |
| Protocol/schema mismatch | Compatible client/relay releases and a usable backup. |

The local Hession library remains usable during a relay outage. A relay is not
an independent backup of your recovery key.
