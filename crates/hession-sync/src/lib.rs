//! Versioned session snapshots and authenticated encrypted relay envelopes.
//!
//! This crate never performs network I/O or writes files. Relay storage sees
//! random record IDs and ciphertext; session identities stay inside encryption.
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_PLAINTEXT: usize = 2 * 1024 * 1024;
pub const CHUNK_BYTES: usize = 512 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

/// No Debug/Serialize implementation: a key must never appear in diagnostics.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LibraryKey([u8; 32]);

impl Default for LibraryKey {
    fn default() -> Self {
        Self::generate()
    }
}

impl LibraryKey {
    pub fn generate() -> Self {
        let mut key = [0; 32];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_recovery(value: &str) -> Result<Self, String> {
        let value = value
            .trim()
            .strip_prefix("hession1:")
            .ok_or("Recovery key must start with hession1:")?;
        // A 32-byte key has exactly 44 standard-base64 characters. Bound this
        // before decoding so malformed recovery input cannot allocate freely.
        if value.len() != 44 {
            return Err("Invalid recovery key length".into());
        }
        let mut decoded = STANDARD.decode(value).map_err(|_| "Invalid recovery key")?;
        let result = decoded
            .as_slice()
            .try_into()
            .map(Self)
            .map_err(|_| "Invalid recovery key length".to_owned());
        decoded.zeroize();
        result
    }

    pub fn recovery_key(&self) -> String {
        format!("hession1:{}", STANDARD.encode(self.0))
    }

    pub fn fingerprint(&self) -> String {
        // Domain separation keeps this value separate from snapshot digests.
        digest(&[b"hession-library-fingerprint-v1", &self.0])[..24].to_owned()
    }
}

pub fn digest(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Best-effort defense in depth, never a promise to detect every secret.
pub fn redact(value: &str) -> String {
    static PATTERNS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| [
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
        r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[A-Z0-9]{16})\b",
        r"(?i)\b(?:authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/=-]+",
        r#"(?im)\b(?:api[_-]?key|access[_-]?token|secret[_-]?key|password)\s*[:=]\s*["']?[^\s"',;]{8,}"#,
    ].iter().map(|p| regex::Regex::new(p).expect("fixed redaction regex")).collect());
    patterns.iter().fold(value.to_owned(), |s, p| {
        p.replace_all(&s, "[REDACTED]").into_owned()
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayRecord {
    pub id: String,
    pub device_id: String,
    pub key_epoch: u32,
    pub payload: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

fn aad(vault_id: &str, record: &RelayRecord) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&(
        "hession-sync-v1",
        vault_id,
        &record.id,
        &record.device_id,
        record.key_epoch,
    ))
    .map_err(|_| "Could not encode authenticated metadata".into())
}

fn valid_identifier(value: &str, allow_dot: bool) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_')
                || (allow_dot && byte == b'.')
        })
}

fn validate_record_metadata(vault_id: &str, record: &RelayRecord) -> Result<(), String> {
    if !valid_identifier(vault_id, false)
        || !valid_identifier(&record.id, false)
        || !valid_identifier(&record.device_id, false)
        || record.key_epoch != 1
    {
        return Err("Unsupported or invalid encrypted record metadata".into());
    }
    Ok(())
}

pub fn encrypt(
    key: &LibraryKey,
    vault_id: &str,
    id: &str,
    device_id: &str,
    plaintext: &[u8],
) -> Result<RelayRecord, String> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err("Record exceeds the encrypted transfer size limit".into());
    }
    let mut record = RelayRecord {
        id: id.into(),
        device_id: device_id.into(),
        key_epoch: 1,
        payload: String::new(),
    };
    validate_record_metadata(vault_id, &record)?;
    let mut nonce = [0; 24];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad(vault_id, &record)?,
            },
        )
        .map_err(|_| "Could not encrypt sync record")?;
    let envelope = Envelope {
        version: PROTOCOL_VERSION,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    };
    record.payload =
        STANDARD.encode(serde_json::to_vec(&envelope).map_err(|_| "Could not encode sync record")?);
    Ok(record)
}

pub fn decrypt(key: &LibraryKey, vault_id: &str, record: &RelayRecord) -> Result<Vec<u8>, String> {
    validate_record_metadata(vault_id, record)?;
    if record.payload.len() > 4 * 1024 * 1024 {
        return Err("Unsupported or oversized encrypted record".into());
    }
    let bytes = STANDARD
        .decode(&record.payload)
        .map_err(|_| "Invalid encrypted record encoding")?;
    let envelope: Envelope =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid encrypted envelope")?;
    if envelope.version != PROTOCOL_VERSION {
        return Err("Unsupported sync envelope version".into());
    }
    if envelope.nonce.len() != 32 {
        return Err("Invalid nonce length".into());
    }
    let nonce = STANDARD
        .decode(envelope.nonce)
        .map_err(|_| "Invalid nonce")?;
    if nonce.len() != 24 {
        return Err("Invalid nonce length".into());
    }
    let ciphertext = STANDARD
        .decode(envelope.ciphertext)
        .map_err(|_| "Invalid ciphertext")?;
    if !(16..=MAX_PLAINTEXT + 16).contains(&ciphertext.len()) {
        return Err("Invalid encrypted record size".into());
    }
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    cipher.decrypt(XNonce::from_slice(&nonce), Payload { msg: &ciphertext, aad: &aad(vault_id, record)? })
        .map_err(|_| "Record authentication failed. Check the recovery-key fingerprint; no data was imported.".into())
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub session_id: String,
    pub revision_id: String,
    pub parent_revision: Option<String>,
    pub content: SessionContent,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionContent {
    pub family: String,
    pub product: String,
    pub title: String,
    pub cwd: String,
    pub started_at: Option<String>,
    pub last_event_at: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub model_key: Option<String>,
    pub starred: bool,
    pub tags: Vec<String>,
    pub events: Vec<Event>,
    pub omissions: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub ordinal: i64,
    pub kind: String,
    pub role: Option<String>,
    pub occurred_at: Option<String>,
    pub text: String,
    pub tools: Vec<Tool>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub command: Option<String>,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub status: Option<String>,
    pub exit_code: Option<i64>,
}

/// Chunking is inside encryption: the relay cannot identify session chunks.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotChunk {
    pub version: u32,
    pub snapshot_id: String,
    pub index: u32,
    pub total: u32,
    pub data: String,
}

pub fn chunks(snapshot: &Snapshot) -> Result<Vec<SnapshotChunk>, String> {
    if snapshot.version != PROTOCOL_VERSION || !valid_identifier(&snapshot.revision_id, true) {
        return Err("Unsupported snapshot version or invalid revision identity".into());
    }
    let bytes = serde_json::to_vec(snapshot).map_err(|_| "Could not encode session snapshot")?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err("Session exceeds the 128 MiB snapshot limit. Exclude it from sync.".into());
    }
    let total = bytes.len().div_ceil(CHUNK_BYTES) as u32;
    Ok(bytes
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(i, b)| SnapshotChunk {
            version: 1,
            snapshot_id: snapshot.revision_id.clone(),
            index: i as u32,
            total,
            data: STANDARD.encode(b),
        })
        .collect())
}

impl SnapshotChunk {
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        if self.version != 1
            || self.total == 0
            || self.total > (MAX_SNAPSHOT_BYTES / CHUNK_BYTES) as u32
            || self.index >= self.total
            || !valid_identifier(&self.snapshot_id, true)
            || self.data.len() > CHUNK_BYTES * 2
        {
            return Err("Invalid session chunk".into());
        }
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| "Invalid session chunk encoding")?;
        if bytes.is_empty()
            || bytes.len() > CHUNK_BYTES
            || (self.index < self.total - 1 && bytes.len() != CHUNK_BYTES)
        {
            return Err("Invalid session chunk size".into());
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_binds_vault_record_writer_and_epoch() {
        let key = LibraryKey::generate();
        let record = encrypt(&key, "vault", "id", "writer", b"private transcript").unwrap();
        assert_eq!(
            decrypt(&key, "vault", &record).unwrap(),
            b"private transcript"
        );
        assert!(!record.payload.contains("private transcript"));
        assert!(decrypt(&key, "other-vault", &record).is_err());
        assert!(decrypt(&LibraryKey::generate(), "vault", &record).is_err());
        let mut changed = record.clone();
        changed.id.push('x');
        assert!(decrypt(&key, "vault", &changed).is_err());
        changed = record.clone();
        changed.device_id.push('x');
        assert!(decrypt(&key, "vault", &changed).is_err());
        changed = record;
        changed.key_epoch = 2;
        assert!(decrypt(&key, "vault", &changed).is_err());
    }

    #[test]
    fn recovery_round_trip_and_nonces_are_random() {
        let key = LibraryKey::generate();
        let recovered = LibraryKey::from_recovery(&key.recovery_key()).unwrap();
        assert_eq!(key.fingerprint(), recovered.fingerprint());
        assert!(LibraryKey::from_recovery("password").is_err());
        assert_ne!(
            encrypt(&key, "v", "id", "d", b"same").unwrap().payload,
            encrypt(&key, "v", "id", "d", b"same").unwrap().payload
        );
    }

    fn snapshot(text: String) -> Snapshot {
        Snapshot {
            version: 1,
            session_id: "portable-session".into(),
            revision_id: "revision-1".into(),
            parent_revision: None,
            content: SessionContent {
                family: "codex".into(),
                product: "Codex".into(),
                title: "Portable task".into(),
                cwd: "/work/project".into(),
                started_at: None,
                last_event_at: None,
                branch: None,
                commit_sha: None,
                model_provider: None,
                model_key: None,
                starred: false,
                tags: Vec::new(),
                events: vec![Event {
                    ordinal: 0,
                    kind: "message".into(),
                    role: Some("user".into()),
                    occurred_at: None,
                    text,
                    tools: Vec::new(),
                }],
                omissions: Vec::new(),
            },
        }
    }

    fn with_envelope(record: &RelayRecord, value: serde_json::Value) -> RelayRecord {
        let mut result = record.clone();
        result.payload = STANDARD.encode(serde_json::to_vec(&value).unwrap());
        result
    }

    #[test]
    fn malformed_and_future_envelopes_fail_without_accepting_partial_plaintext() {
        let key = LibraryKey::generate();
        let record = encrypt(&key, "vault", "record", "writer", b"confidential context").unwrap();
        let valid: serde_json::Value =
            serde_json::from_slice(&STANDARD.decode(&record.payload).unwrap()).unwrap();
        let mut cases = Vec::new();
        for version in [0, 2, u32::MAX] {
            let mut value = valid.clone();
            value["version"] = version.into();
            cases.push(value);
        }
        for nonce in [
            "!".into(),
            STANDARD.encode([0; 23]),
            STANDARD.encode([0; 25]),
            "A".repeat(512),
        ] {
            let mut value = valid.clone();
            value["nonce"] = nonce.into();
            cases.push(value);
        }
        for ciphertext in [
            "!".into(),
            String::new(),
            STANDARD.encode([0; 15]),
            STANDARD.encode(vec![0; MAX_PLAINTEXT + 17]),
        ] {
            let mut value = valid.clone();
            value["ciphertext"] = ciphertext.into();
            cases.push(value);
        }
        let mut unknown = valid.clone();
        unknown["secretFromFutureVersion"] = true.into();
        cases.push(unknown);
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("nonce");
        cases.push(missing);
        for value in cases {
            assert!(decrypt(&key, "vault", &with_envelope(&record, value)).is_err());
        }
        for bytes in [
            b"not json".as_slice(),
            br#"{"version":1,"version":1,"nonce":"","ciphertext":""}"#,
        ] {
            let mut malformed = record.clone();
            malformed.payload = STANDARD.encode(bytes);
            assert!(decrypt(&key, "vault", &malformed).is_err());
        }
        let mut invalid_base64 = record.clone();
        invalid_base64.payload = "not-base64!".into();
        assert!(decrypt(&key, "vault", &invalid_base64).is_err());
        invalid_base64.payload = "A".repeat(4 * 1024 * 1024 + 1);
        assert!(decrypt(&key, "vault", &invalid_base64).is_err());
    }

    #[test]
    fn every_ciphertext_and_tag_byte_is_authenticated() {
        let key = LibraryKey::generate();
        let record = encrypt(&key, "vault", "record", "writer", b"secret context").unwrap();
        let envelope: Envelope =
            serde_json::from_slice(&STANDARD.decode(&record.payload).unwrap()).unwrap();
        let original = STANDARD.decode(&envelope.ciphertext).unwrap();
        for index in 0..original.len() {
            let mut changed = original.clone();
            changed[index] ^= 1;
            let value = serde_json::json!({"version":1,"nonce":envelope.nonce,"ciphertext":STANDARD.encode(changed)});
            assert!(
                decrypt(&key, "vault", &with_envelope(&record, value)).is_err(),
                "unauthenticated ciphertext byte {index}"
            );
        }
        let mut nonce = STANDARD.decode(&envelope.nonce).unwrap();
        nonce[0] ^= 1;
        let value = serde_json::json!({"version":1,"nonce":STANDARD.encode(nonce),"ciphertext":envelope.ciphertext});
        assert!(decrypt(&key, "vault", &with_envelope(&record, value)).is_err());
    }

    #[test]
    fn encryption_boundaries_and_recovery_validation_are_bounded() {
        let key = LibraryKey::generate();
        let plaintext = vec![0x5a; MAX_PLAINTEXT];
        let record = encrypt(&key, "vault", "record", "writer", &plaintext).unwrap();
        assert!(record.payload.len() <= 4 * 1024 * 1024);
        assert_eq!(decrypt(&key, "vault", &record).unwrap(), plaintext);
        assert!(encrypt(
            &key,
            "vault",
            "record",
            "writer",
            &vec![0; MAX_PLAINTEXT + 1]
        )
        .is_err());
        for metadata in ["", "../escape", "contains space", &"a".repeat(129)] {
            assert!(encrypt(&key, metadata, "record", "writer", b"x").is_err());
            assert!(encrypt(&key, "vault", metadata, "writer", b"x").is_err());
            assert!(encrypt(&key, "vault", "record", metadata, b"x").is_err());
        }
        for decoded in [vec![], vec![0; 31], vec![0; 33], vec![0; 1024]] {
            assert!(
                LibraryKey::from_recovery(&format!("hession1:{}", STANDARD.encode(decoded)))
                    .is_err()
            );
        }
        assert!(LibraryKey::from_recovery(&format!("hession1:{}", "A".repeat(100_000))).is_err());
        assert!(LibraryKey::from_recovery(&format!("hession1:{}", "!".repeat(44))).is_err());
        assert_eq!(
            LibraryKey::from_recovery(&format!(" \n{}\n ", key.recovery_key()))
                .unwrap()
                .fingerprint(),
            key.fingerprint()
        );
    }

    #[test]
    fn unicode_snapshot_roundtrips_across_encrypted_chunk_boundaries() {
        let expected = snapshot("🦀 café context\n".repeat(60_000));
        let key = LibraryKey::generate();
        let pieces = chunks(&expected).unwrap();
        assert!(pieces.len() > 1);
        let mut assembled = Vec::new();
        for (index, piece) in pieces.into_iter().enumerate() {
            let plaintext = serde_json::to_vec(&piece).unwrap();
            let encrypted = encrypt(
                &key,
                "vault",
                &format!("record-{index}"),
                "writer",
                &plaintext,
            )
            .unwrap();
            let decrypted = decrypt(&key, "vault", &encrypted).unwrap();
            let received: SnapshotChunk = serde_json::from_slice(&decrypted).unwrap();
            assert_eq!(received.index, index as u32);
            assembled.extend(received.bytes().unwrap());
        }
        assert!(serde_json::from_slice::<Snapshot>(&assembled).unwrap() == expected);
    }

    #[test]
    fn malformed_chunks_cannot_create_empty_or_truncated_partial_snapshots() {
        let valid = || SnapshotChunk {
            version: 1,
            snapshot_id: "revision-1".into(),
            index: 0,
            total: 1,
            data: STANDARD.encode(b"payload"),
        };
        assert_eq!(valid().bytes().unwrap(), b"payload");
        for id in ["", "../path", "white space", "🦀", &"x".repeat(129)] {
            let mut piece = valid();
            piece.snapshot_id = id.into();
            assert!(piece.bytes().is_err());
        }
        for (index, total) in [(0, 0), (1, 1), (0, 257), (u32::MAX, u32::MAX)] {
            let mut piece = valid();
            piece.index = index;
            piece.total = total;
            assert!(piece.bytes().is_err());
        }
        for version in [0, 2, u32::MAX] {
            let mut piece = valid();
            piece.version = version;
            assert!(piece.bytes().is_err());
        }
        for bytes in [Vec::new(), vec![0; CHUNK_BYTES + 1]] {
            let mut piece = valid();
            piece.data = STANDARD.encode(bytes);
            assert!(piece.bytes().is_err());
        }
        let mut piece = valid();
        piece.data = "!".into();
        assert!(piece.bytes().is_err());
        piece = valid();
        piece.data = "A".repeat(CHUNK_BYTES * 2 + 1);
        assert!(piece.bytes().is_err());
        piece = valid();
        piece.total = 2;
        assert!(
            piece.bytes().is_err(),
            "non-final chunk must fill its declared span"
        );
        piece.data = STANDARD.encode(vec![1; CHUNK_BYTES]);
        assert_eq!(piece.bytes().unwrap().len(), CHUNK_BYTES);
        piece.index = 1;
        piece.data = STANDARD.encode(b"final");
        assert_eq!(piece.bytes().unwrap(), b"final");
        let mut invalid = snapshot("x".into());
        invalid.version = 2;
        assert!(chunks(&invalid).is_err());
        invalid.version = 1;
        invalid.revision_id.clear();
        assert!(chunks(&invalid).is_err());
    }
}
