//! An opaque, single-vault store-and-forward relay. Encryption belongs to clients.

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
pub const MAX_BODY: usize = 8 * 1024 * 1024;
pub const MAX_BATCH: usize = 32;
pub const DEFAULT_QUOTA: u64 = 1024 * 1024 * 1024;
const INVITATION_TTL: i64 = 10 * 60;

#[derive(Clone)]
pub struct Relay(Arc<Inner>);

struct Inner {
    db: Mutex<Connection>,
    admin_hash: [u8; 32],
    quota: u64,
}

#[derive(Debug)]
struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn database_error(_: rusqlite::Error) -> ApiError {
    // Do not send database paths, SQL parameters, tokens, or records to logs/clients.
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Relay storage failed.")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn bearer(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| (32..=256).contains(&value.len()))
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "Authentication required.",
        ))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

impl Relay {
    pub fn open(path: &FilePath, admin_token: &str, quota: u64) -> Result<Self, String> {
        if !(32..=256).contains(&admin_token.len())
            || !admin_token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err("HESSION_RELAY_ADMIN_TOKEN must contain 32–256 printable ASCII characters without whitespace.".into());
        }
        if quota == 0 || quota > i64::MAX as u64 {
            return Err("The relay quota must be a positive number of bytes below 2^63.".into());
        }
        prepare_database_path(path)?;
        let db = Connection::open(path).map_err(|_| "Cannot open relay database.")?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|_| "Cannot configure relay storage.")?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| "Cannot inspect relay schema.")?;
        if version > 1 {
            return Err("This relay database needs a newer Hession relay binary.".into());
        }
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS device(
               id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash BLOB NOT NULL UNIQUE,
               created_at INTEGER NOT NULL, revoked_at INTEGER);
             CREATE TABLE IF NOT EXISTS record(
               sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
               device_id TEXT NOT NULL REFERENCES device(id), key_epoch INTEGER NOT NULL CHECK(key_epoch > 0),
               payload TEXT NOT NULL, byte_length INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS invitation(token_hash BLOB PRIMARY KEY, expires_at INTEGER NOT NULL);
             PRAGMA user_version=1;
             COMMIT;",
        )
        .map_err(|_| "Cannot initialize relay storage.")?;
        Ok(Self(Arc::new(Inner {
            db: Mutex::new(db),
            admin_hash: hash(admin_token),
            quota,
        })))
    }

    async fn with_db<T, F>(&self, operation: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> ApiResult<T> + Send + 'static,
    {
        let relay = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = relay.0.db.lock().map_err(|_| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Relay storage unavailable.",
                )
            })?;
            operation(&mut db)
        })
        .await
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Relay worker failed."))?
    }

    fn admin(&self, headers: &HeaderMap) -> ApiResult<()> {
        if bool::from(hash(bearer(headers)?).ct_eq(&self.0.admin_hash)) {
            Ok(())
        } else {
            Err(ApiError(
                StatusCode::UNAUTHORIZED,
                "Invalid administrator credential.",
            ))
        }
    }

    fn manager(&self, db: &Connection, headers: &HeaderMap) -> ApiResult<()> {
        if self.admin(headers).is_ok() {
            Ok(())
        } else {
            device(db, headers).map(|_| ())
        }
    }
}

pub fn database_path() -> PathBuf {
    std::env::var_os("HESSION_RELAY_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./hession-relay-data"))
        .join("relay.sqlite")
}

fn prepare_parent_directory(path: &FilePath) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let existed = parent.exists();
        std::fs::create_dir_all(parent).map_err(|_| "Cannot create relay data directory.")?;
        #[cfg(unix)]
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "Cannot protect relay data directory.")?;
        }
        #[cfg(not(unix))]
        let _ = existed;
    }
    Ok(())
}

fn prepare_database_path(path: &FilePath) -> Result<(), String> {
    prepare_parent_directory(path)?;
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err("Relay database cannot be a symbolic link.".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "Cannot create relay database.")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "Cannot protect relay database.")?;
    }
    Ok(())
}

/// Online backup: a single consistent database, even when the server uses WAL.
pub fn backup(source: &FilePath, destination: &FilePath) -> Result<(), String> {
    if !source.is_file() {
        return Err("Relay database does not exist.".into());
    }
    // symlink_metadata also detects dangling links without following them.
    if destination.symlink_metadata().is_ok() {
        return Err("Backup destination already exists; choose a new path.".into());
    }
    prepare_parent_directory(destination)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| FilePath::new("."));
    // Never create, truncate, or delete the final path until a consistent
    // database is ready. Atomic no-clobber persistence protects concurrent
    // backup commands and existing files, including symbolic links.
    let temporary = tempfile::Builder::new()
        .prefix(".hession-relay-backup-")
        .tempfile_in(parent)
        .map_err(|_| "Cannot prepare relay backup.")?;
    {
        let source =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "Cannot open relay database for backup.")?;
        source
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| "Cannot configure backup.")?;
        let mut destination =
            Connection::open(temporary.path()).map_err(|_| "Cannot open backup.")?;
        rusqlite::backup::Backup::new(&source, &mut destination)
            .and_then(|backup| backup.run_to_completion(128, Duration::from_millis(5), None))
            .map_err(|_| "Relay backup failed.")?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| "Cannot synchronize relay backup.")?;
    temporary.persist_noclobber(destination).map_err(|_| {
        "Backup destination exists or could not be saved; existing files were not changed."
    })?;
    Ok(())
}

fn device(db: &Connection, headers: &HeaderMap) -> ApiResult<String> {
    let digest = hash(bearer(headers)?);
    db.query_row(
        "SELECT id FROM device WHERE token_hash=? AND revoked_at IS NULL",
        [digest.as_slice()],
        |row| row.get(0),
    )
    .optional()
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::UNAUTHORIZED,
        "Device credential is invalid or revoked.",
    ))
}

fn vault(db: &Connection) -> ApiResult<String> {
    db.query_row("SELECT value FROM meta WHERE key='vault_id'", [], |row| {
        row.get(0)
    })
    .optional()
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::CONFLICT,
        "Initialize this relay first.",
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Enrollment {
    device_id: String,
    device_name: String,
}

fn enroll(db: &Connection, request: &Enrollment, vault_id: String) -> ApiResult<Value> {
    if !identifier(&request.device_id)
        || request.device_name.trim().is_empty()
        || request.device_name.len() > 128
        || request.device_name.chars().any(char::is_control)
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Provide a valid device ID and name (up to 128 bytes).",
        ));
    }
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM device WHERE id=?)",
            [&request.device_id],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if exists {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Device ID already enrolled; generate a new ID.",
        ));
    }
    let count: u32 = db
        .query_row("SELECT count(*) FROM device", [], |r| r.get(0))
        .map_err(database_error)?;
    if count >= 256 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Relay device limit reached.",
        ));
    }
    let credential = token();
    db.execute(
        "INSERT INTO device(id,name,token_hash,created_at) VALUES(?,?,?,?)",
        params![
            request.device_id,
            request.device_name.trim(),
            hash(&credential).as_slice(),
            now()
        ],
    )
    .map_err(database_error)?;
    Ok(json!({"vaultId":vault_id,"deviceId":request.device_id,"deviceToken":credential}))
}

pub fn router(relay: Relay) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/bootstrap", post(bootstrap))
        .route("/v1/records", get(pull).post(push))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/join", post(join))
        .route("/v1/devices/{id}", delete(revoke))
        .route("/v1/invitations", post(invite))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(tower::limit::ConcurrencyLimitLayer::new(16))
        .layer(middleware::from_fn(private_response))
        .with_state(relay)
}

async fn private_response(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn status(State(relay): State<Relay>) -> ApiResult<Json<Value>> {
    relay
        .with_db(|db| {
            let initialized: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM meta WHERE key='vault_id')",
                    [],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            Ok(Json(
                json!({"protocolVersion":PROTOCOL_VERSION,"initialized":initialized}),
            ))
        })
        .await
}

async fn bootstrap(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(request): Json<Enrollment>,
) -> ApiResult<Json<Value>> {
    relay.admin(&headers)?;
    relay
        .with_db(move |db| {
            let transaction = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let initialized: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM meta WHERE key='vault_id')",
                    [],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            if initialized {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "This relay is already initialized; pair a device instead.",
                ));
            }
            let vault_id = Uuid::new_v4().to_string();
            transaction
                .execute(
                    "INSERT INTO meta(key,value) VALUES('vault_id',?)",
                    [&vault_id],
                )
                .map_err(database_error)?;
            let result = enroll(&transaction, &request, vault_id)?;
            transaction.commit().map_err(database_error)?;
            Ok(Json(result))
        })
        .await
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub id: String,
    pub device_id: String,
    pub key_epoch: u32,
    pub payload: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Push {
    records: Vec<Record>,
}

async fn push(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(request): Json<Push>,
) -> ApiResult<Json<Value>> {
    let quota = relay.0.quota;
    relay.with_db(move |db| {
        let writer = device(db, &headers)?;
        if request.records.is_empty() || request.records.len() > MAX_BATCH {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Upload between 1 and 32 records at a time."));
        }
        let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(database_error)?;
        let mut used = transaction.query_row("SELECT COALESCE(SUM(byte_length),0) FROM record", [], |r| r.get::<_,i64>(0)).map_err(database_error)? as u64;
        for record in &request.records {
            if !identifier(&record.id) || record.device_id != writer || record.key_epoch == 0 {
                return Err(ApiError(StatusCode::BAD_REQUEST, "Invalid record ID, key epoch, or writer."));
            }
            if record.payload.is_empty() || record.payload.len() > MAX_PAYLOAD {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "Encrypted record exceeds the 4 MiB limit."));
            }
            if STANDARD.decode(&record.payload).is_err() {
                return Err(ApiError(StatusCode::BAD_REQUEST, "Encrypted payload must be standard base64."));
            }
            let previous: Option<(String, u32, String)> = transaction.query_row("SELECT device_id,key_epoch,payload FROM record WHERE id=?", [&record.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(database_error)?;
            if let Some(previous) = previous {
                if previous != (record.device_id.clone(), record.key_epoch, record.payload.clone()) {
                    return Err(ApiError(StatusCode::CONFLICT, "Record ID already contains different data."));
                }
                continue;
            }
            used = used.saturating_add(record.payload.len() as u64);
            if used > quota {
                return Err(ApiError(StatusCode::INSUFFICIENT_STORAGE, "Relay storage quota reached; ask the owner to increase it."));
            }
            transaction.execute("INSERT INTO record(id,device_id,key_epoch,payload,byte_length) VALUES(?,?,?,?,?)", params![record.id,record.device_id,record.key_epoch,record.payload,record.payload.len() as i64]).map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)?;
        Ok(Json(json!({"accepted":request.records.len()})))
    }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedQuery {
    #[serde(default)]
    after: u64,
    limit: Option<u32>,
}

async fn pull(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<Value>> {
    relay.with_db(move |db| {
        device(db, &headers)?;
        let limit = query.limit.unwrap_or(100);
        if !(1..=100).contains(&limit) || query.after > i64::MAX as u64 {
            return Err(ApiError(StatusCode::BAD_REQUEST, "Use a valid cursor and a limit between 1 and 100."));
        }
        let newest: i64 = db.query_row("SELECT COALESCE(MAX(sequence),0) FROM record", [], |r| r.get(0)).map_err(database_error)?;
        if query.after as i64 > newest {
            return Err(ApiError(StatusCode::CONFLICT, "Download cursor is ahead of this relay; reconnect to replay restored history."));
        }
        let mut statement = db.prepare("SELECT sequence,id,device_id,key_epoch,payload FROM record WHERE sequence>? ORDER BY sequence LIMIT ?").map_err(database_error)?;
        let mut rows = statement.query(params![query.after as i64, limit + 1]).map_err(database_error)?;
        let mut records = Vec::new();
        let mut cursor = query.after as i64;
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(database_error)? {
            let payload: String = row.get(4).map_err(database_error)?;
            if records.len() >= limit as usize || (!records.is_empty() && bytes + payload.len() + 512 > MAX_BODY - 4096) {
                has_more = true;
                break;
            }
            let sequence: i64 = row.get(0).map_err(database_error)?;
            bytes += payload.len() + 512;
            records.push(json!({"sequence":sequence,"id":row.get::<_,String>(1).map_err(database_error)?,"deviceId":row.get::<_,String>(2).map_err(database_error)?,"keyEpoch":row.get::<_,u32>(3).map_err(database_error)?,"payload":payload}));
            cursor = sequence;
        }
        Ok(Json(json!({"records":records,"cursor":cursor,"hasMore":has_more})))
    }).await
}

async fn devices(State(relay): State<Relay>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let access = relay.clone();
    relay.with_db(move |db| {
        access.manager(db, &headers)?;
        let mut statement = db.prepare("SELECT id,name,created_at,revoked_at IS NOT NULL FROM device ORDER BY created_at,id").map_err(database_error)?;
        let result = statement.query_map([], |r| Ok(json!({"deviceId":r.get::<_,String>(0)?,"deviceName":r.get::<_,String>(1)?,"createdAt":r.get::<_,i64>(2)?,"revoked":r.get::<_,bool>(3)?}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
        Ok(Json(json!({"devices":result})))
    }).await
}

async fn invite(State(relay): State<Relay>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let access = relay.clone();
    relay
        .with_db(move |db| {
            access.manager(db, &headers)?;
            vault(db)?;
            let transaction = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            transaction
                .execute("DELETE FROM invitation WHERE expires_at<=?", [now()])
                .map_err(database_error)?;
            let count: u32 = transaction
                .query_row("SELECT count(*) FROM invitation", [], |r| r.get(0))
                .map_err(database_error)?;
            if count >= 32 {
                return Err(ApiError(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many outstanding invitations; wait ten minutes.",
                ));
            }
            let invitation = token();
            let expires_at = now() + INVITATION_TTL;
            transaction
                .execute(
                    "INSERT INTO invitation(token_hash,expires_at) VALUES(?,?)",
                    params![hash(&invitation).as_slice(), expires_at],
                )
                .map_err(database_error)?;
            transaction.commit().map_err(database_error)?;
            Ok(Json(
                json!({"invitation":invitation,"expiresAt":expires_at}),
            ))
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Join {
    invitation: String,
    device_id: String,
    device_name: String,
}

async fn join(State(relay): State<Relay>, Json(request): Json<Join>) -> ApiResult<Json<Value>> {
    if request.invitation.len() != 64 {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invitation is invalid or expired.",
        ));
    }
    relay
        .with_db(move |db| {
            let transaction = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let valid: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM invitation WHERE token_hash=? AND expires_at>?)",
                    params![hash(&request.invitation).as_slice(), now()],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            if !valid {
                return Err(ApiError(
                    StatusCode::UNAUTHORIZED,
                    "Invitation is invalid or expired.",
                ));
            }
            let result = enroll(
                &transaction,
                &Enrollment {
                    device_id: request.device_id,
                    device_name: request.device_name,
                },
                vault(&transaction)?,
            )?;
            transaction
                .execute(
                    "DELETE FROM invitation WHERE token_hash=?",
                    [hash(&request.invitation).as_slice()],
                )
                .map_err(database_error)?;
            transaction.commit().map_err(database_error)?;
            Ok(Json(result))
        })
        .await
}

async fn revoke(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let access = relay.clone();
    relay
        .with_db(move |db| {
            access.manager(db, &headers)?;
            let transaction = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(database_error)?;
            let changed = transaction
                .execute(
                    "UPDATE device SET revoked_at=? WHERE id=? AND revoked_at IS NULL",
                    params![now(), id],
                )
                .map_err(database_error)?;
            if changed == 0 {
                return Err(ApiError(StatusCode::NOT_FOUND, "Active device not found."));
            }
            // A revoked device may have already handed out invitations. In a
            // personal vault, invalidate all outstanding invites conservatively.
            transaction
                .execute("DELETE FROM invitation", [])
                .map_err(database_error)?;
            transaction.commit().map_err(database_error)?;
            Ok(Json(json!({"revoked":true})))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    const ADMIN: &str = "test-administrator-secret-at-least-32-characters";

    fn setup(quota: u64) -> (tempfile::TempDir, Relay, Router) {
        let directory = tempfile::tempdir().unwrap();
        let relay = Relay::open(&directory.path().join("relay.sqlite"), ADMIN, quota).unwrap();
        let app = router(relay.clone());
        (directory, relay, app)
    }

    async fn request(
        app: &Router,
        method: &str,
        uri: &str,
        credential: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(credential) = credential {
            request = request.header("authorization", format!("Bearer {credential}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), MAX_BODY + 4096)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn initialize(app: &Router) -> String {
        let (status, response) = request(
            app,
            "POST",
            "/v1/bootstrap",
            Some(ADMIN),
            json!({"deviceId":"laptop","deviceName":"Laptop"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        response["deviceToken"].as_str().unwrap().to_owned()
    }

    fn record(id: &str, writer: &str, contents: &str) -> Value {
        json!({"id":id,"deviceId":writer,"keyEpoch":1,"payload":STANDARD.encode(contents)})
    }

    #[tokio::test]
    async fn bootstrap_is_authenticated_one_time_and_admin_cannot_read_history() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let (status, response) = request(&app, "GET", "/v1/status", None, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response, json!({"protocolVersion":1,"initialized":false}));
        let enrollment = json!({"deviceId":"laptop","deviceName":"Laptop"});
        assert_eq!(
            request(&app, "POST", "/v1/bootstrap", None, enrollment.clone())
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let credential = initialize(&app).await;
        assert_eq!(
            request(&app, "POST", "/v1/bootstrap", Some(ADMIN), enrollment)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(&app, "GET", "/v1/records", Some(ADMIN), Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, "GET", "/v1/records", Some(&credential), Value::Null)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "GET", "/v1/records", None, Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let (_, devices) = request(&app, "GET", "/v1/devices", Some(ADMIN), Value::Null).await;
        assert_eq!(devices["devices"][0]["deviceName"], "Laptop");
        assert!(!devices.to_string().contains(&credential));
    }

    #[tokio::test]
    async fn record_retries_are_idempotent_but_conflicting_ids_and_other_writers_fail() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        let batch = json!({"records":[record("one","laptop","ciphertext")]});
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                batch.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "POST", "/v1/records", Some(&credential), batch)
                .await
                .1["accepted"],
            1
        );
        let (_, feed) = request(&app, "GET", "/v1/records", Some(&credential), Value::Null).await;
        assert_eq!(feed["records"].as_array().unwrap().len(), 1);
        assert_eq!(feed["cursor"], 1);
        assert_eq!(feed["hasMore"], false);
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                json!({"records":[record("one","laptop","changed")]})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                json!({"records":[record("two","desktop","anything")]})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(request(&app,"POST","/v1/records",Some(&credential),json!({"records":[{"id":"bad","deviceId":"laptop","keyEpoch":1,"payload":"not-base64!"}]})).await.0,StatusCode::BAD_REQUEST);
        assert_eq!(
            request(
                &app,
                "GET",
                "/v1/records?limit=101",
                Some(&credential),
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/v1/records?after=18446744073709551615",
                Some(&credential),
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn invitations_are_one_use_and_revocation_denies_both_read_and_write() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let laptop = initialize(&app).await;
        let (_, invitation) =
            request(&app, "POST", "/v1/invitations", Some(&laptop), Value::Null).await;
        let enrollment = json!({"deviceId":"desktop","deviceName":"Desktop","invitation":invitation["invitation"]});
        let (status, desktop) =
            request(&app, "POST", "/v1/devices/join", None, enrollment.clone()).await;
        assert_eq!(status, StatusCode::OK);
        let desktop = desktop["deviceToken"].as_str().unwrap();
        assert_eq!(
            request(&app, "POST", "/v1/devices/join", None, enrollment)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let records = json!({"records":[record("a","laptop","one"),record("b","laptop","two")]});
        assert_eq!(
            request(&app, "POST", "/v1/records", Some(&laptop), records)
                .await
                .0,
            StatusCode::OK
        );
        let (_, first) = request(
            &app,
            "GET",
            "/v1/records?limit=1",
            Some(desktop),
            Value::Null,
        )
        .await;
        assert_eq!(first["hasMore"], true);
        assert_eq!(first["records"][0]["id"], "a");
        let (_, second) = request(
            &app,
            "GET",
            "/v1/records?limit=1&after=1",
            Some(desktop),
            Value::Null,
        )
        .await;
        assert_eq!(second["hasMore"], false);
        assert_eq!(second["records"][0]["id"], "b");
        assert_eq!(
            request(
                &app,
                "DELETE",
                "/v1/devices/desktop",
                Some(&laptop),
                Value::Null
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "GET", "/v1/records", Some(desktop), Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(desktop),
                json!({"records":[record("c","desktop","three")]})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, "POST", "/v1/invitations", Some(desktop), Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, "POST", "/v1/invitations", Some(ADMIN), Value::Null)
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn quota_failure_rolls_back_entire_batch_and_expired_invitations_cannot_join() {
        let (_directory, relay, app) = setup(12);
        let credential = initialize(&app).await;
        let batch =
            json!({"records":[record("a","laptop","123456"),record("b","laptop","123456")]});
        assert_eq!(
            request(&app, "POST", "/v1/records", Some(&credential), batch)
                .await
                .0,
            StatusCode::INSUFFICIENT_STORAGE
        );
        let (_, feed) = request(&app, "GET", "/v1/records", Some(&credential), Value::Null).await;
        assert!(feed["records"].as_array().unwrap().is_empty());
        let (_, invitation) = request(
            &app,
            "POST",
            "/v1/invitations",
            Some(&credential),
            Value::Null,
        )
        .await;
        relay
            .0
            .db
            .lock()
            .unwrap()
            .execute("UPDATE invitation SET expires_at=0", [])
            .unwrap();
        assert_eq!(request(&app,"POST","/v1/devices/join",None,json!({"invitation":invitation["invitation"],"deviceId":"desktop","deviceName":"Desktop"})).await.0,StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn durable_backup_restores_records_and_credentials_without_exposing_plain_tokens() {
        let (directory, relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        request(
            &app,
            "POST",
            "/v1/records",
            Some(&credential),
            json!({"records":[record("persist","laptop","encrypted contents")]}),
        )
        .await;
        let hash: Vec<u8> = relay
            .0
            .db
            .lock()
            .unwrap()
            .query_row("SELECT token_hash FROM device", [], |r| r.get(0))
            .unwrap();
        assert_ne!(hash, credential.as_bytes());
        let source = directory.path().join("relay.sqlite");
        let destination = directory.path().join("backup.sqlite");
        backup(&source, &destination).unwrap();
        assert!(backup(&source, &destination).is_err());
        let restored = router(Relay::open(&destination, ADMIN, DEFAULT_QUOTA).unwrap());
        let (status, feed) = request(
            &restored,
            "GET",
            "/v1/records",
            Some(&credential),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(feed["records"][0]["id"], "persist");
        assert_eq!(
            feed["records"][0]["payload"],
            STANDARD.encode("encrypted contents")
        );
    }

    #[tokio::test]
    async fn actual_client_envelopes_roundtrip_and_tampered_metadata_cannot_decrypt() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        let key = hession_sync::LibraryKey::generate();
        let vault_id = _relay
            .0
            .db
            .lock()
            .unwrap()
            .query_row("SELECT value FROM meta WHERE key='vault_id'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap();
        let encrypted = hession_sync::encrypt(
            &key,
            &vault_id,
            "opaque-id",
            "laptop",
            b"private session context",
        )
        .unwrap();
        let encoded = serde_json::to_value(&encrypted).unwrap();
        assert!(!encoded.to_string().contains("private session context"));
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                json!({"records":[encoded]})
            )
            .await
            .0,
            StatusCode::OK
        );
        let (_, feed) = request(&app, "GET", "/v1/records", Some(&credential), Value::Null).await;
        let mut value = feed["records"][0].clone();
        value.as_object_mut().unwrap().remove("sequence");
        let mut received: hession_sync::RelayRecord = serde_json::from_value(value).unwrap();
        assert_eq!(
            hession_sync::decrypt(&key, &vault_id, &received).unwrap(),
            b"private session context"
        );
        received.id = "changed-by-relay".into();
        assert!(hession_sync::decrypt(&key, &vault_id, &received).is_err());
        assert!(hession_sync::decrypt(&key, "another-vault", &encrypted).is_err());
    }

    #[tokio::test]
    async fn ahead_cursors_and_oversized_requests_are_rejected_and_secrets_are_not_cached() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        assert_eq!(
            request(
                &app,
                "GET",
                "/v1/records?after=1",
                Some(&credential),
                Value::Null
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/records")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {credential}"))
                    .body(Body::from("x".repeat(MAX_BODY + 1)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
    }

    #[tokio::test]
    async fn revoking_a_device_also_closes_existing_enrollment_invitations() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        let (_, invitation) = request(
            &app,
            "POST",
            "/v1/invitations",
            Some(&credential),
            Value::Null,
        )
        .await;
        assert_eq!(
            request(
                &app,
                "DELETE",
                "/v1/devices/laptop",
                Some(ADMIN),
                Value::Null
            )
            .await
            .0,
            StatusCode::OK
        );
        let enrollment = json!({"invitation":invitation["invitation"],"deviceId":"unexpected","deviceName":"Unexpected"});
        assert_eq!(
            request(&app, "POST", "/v1/devices/join", None, enrollment)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        // Administrator access can still recover enrollment without decrypting.
        assert_eq!(
            request(&app, "POST", "/v1/invitations", Some(ADMIN), Value::Null)
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_failed_batch_cannot_publish_its_valid_prefix_or_consume_sequence_numbers() {
        let (_directory, relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        let initial = json!({"records":[record("existing","laptop","original")]});
        assert_eq!(
            request(&app, "POST", "/v1/records", Some(&credential), initial)
                .await
                .0,
            StatusCode::OK
        );
        let mut invalid_base64 = record("invalid", "laptop", "unused");
        invalid_base64["payload"] = "malformed!".into();
        for (invalid, status) in [
            (
                record("existing", "laptop", "conflicting content"),
                StatusCode::CONFLICT,
            ),
            (
                record("spoofed", "another-device", "unauthorized writer"),
                StatusCode::BAD_REQUEST,
            ),
            (invalid_base64, StatusCode::BAD_REQUEST),
        ] {
            let batch =
                json!({"records":[record("valid-prefix","laptop","must roll back"),invalid]});
            assert_eq!(
                request(&app, "POST", "/v1/records", Some(&credential), batch)
                    .await
                    .0,
                status
            );
            let db = relay.0.db.lock().unwrap();
            assert_eq!(
                db.query_row("SELECT count(*) FROM record", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT seq FROM sqlite_sequence WHERE name='record'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(ADMIN),
                json!({"records":[record("admin-write","laptop","must fail")]})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                None,
                json!({"records":[record("anonymous","laptop","must fail")]})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let next =
            json!({"records":[record("next","laptop","valid"),record("next","laptop","valid")]});
        let (status, reply) = request(&app, "POST", "/v1/records", Some(&credential), next).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reply["accepted"], 2,
            "identical records in one retry batch must be acknowledged"
        );
        let (_, feed) = request(&app, "GET", "/v1/records", Some(&credential), Value::Null).await;
        assert_eq!(feed["cursor"], 2);
        assert_eq!(feed["records"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn exact_quota_retries_do_not_charge_existing_ciphertext_twice() {
        let (_directory, _relay, app) = setup(12);
        let credential = initialize(&app).await;
        let batch = json!({"records":[record("fills-quota","laptop","123456789")]});
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                batch.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "POST", "/v1/records", Some(&credential), batch)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/records",
                Some(&credential),
                json!({"records":[record("too-much","laptop","x")]})
            )
            .await
            .0,
            StatusCode::INSUFFICIENT_STORAGE
        );
        let (_, feed) = request(&app, "GET", "/v1/records", Some(&credential), Value::Null).await;
        assert_eq!(feed["cursor"], 1);
    }

    #[tokio::test]
    async fn invalid_bootstrap_rolls_back_and_failed_enrollment_does_not_spend_invitation() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/bootstrap",
                Some(ADMIN),
                json!({"deviceId":"invalid/path","deviceName":"Bad"})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (_, status) = request(&app, "GET", "/v1/status", None, Value::Null).await;
        assert_eq!(status["initialized"], false);
        let credential = initialize(&app).await;
        let (_, invitation) = request(
            &app,
            "POST",
            "/v1/invitations",
            Some(&credential),
            Value::Null,
        )
        .await;
        let join = |id: &str, name: &str| json!({"invitation":invitation["invitation"],"deviceId":id,"deviceName":name});
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/devices/join",
                None,
                join("laptop", "Duplicate")
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/devices/join",
                None,
                join("desktop", "control\nname")
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (first, second) = tokio::join!(
            request(
                &app,
                "POST",
                "/v1/devices/join",
                None,
                join("desktop-one", "One")
            ),
            request(
                &app,
                "POST",
                "/v1/devices/join",
                None,
                join("desktop-two", "Two")
            )
        );
        let statuses = [first.0, second.0];
        assert_eq!(statuses.iter().filter(|s| **s == StatusCode::OK).count(), 1);
        assert_eq!(
            statuses
                .iter()
                .filter(|s| **s == StatusCode::UNAUTHORIZED)
                .count(),
            1,
            "a one-use invitation must not admit concurrent joiners"
        );
    }

    #[tokio::test]
    async fn malformed_feed_queries_do_not_advance_or_mutate_the_store() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        request(
            &app,
            "POST",
            "/v1/records",
            Some(&credential),
            json!({"records":[record("one","laptop","data")]}),
        )
        .await;
        for query in [
            "after=-1",
            "after=text",
            "after=1&after=2",
            "limit=0",
            "limit=-1",
            "limit=101",
            "limit=999999999999999999999999",
            "after=18446744073709551616",
            "unknown=1",
        ] {
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("/v1/records?{query}"),
                    Some(&credential),
                    Value::Null
                )
                .await
                .0,
                StatusCode::BAD_REQUEST,
                "{query}"
            );
        }
        let (_, feed) = request(
            &app,
            "GET",
            "/v1/records?after=1&limit=1",
            Some(&credential),
            Value::Null,
        )
        .await;
        assert_eq!(feed["cursor"], 1);
        assert_eq!(feed["records"], json!([]));
        assert_eq!(feed["hasMore"], false);
    }

    #[tokio::test]
    async fn size_bounded_pages_deliver_all_large_records_without_skipping() {
        let (_directory, _relay, app) = setup(DEFAULT_QUOTA);
        let credential = initialize(&app).await;
        let payload = STANDARD.encode(vec![0xab; MAX_PAYLOAD / 4 * 3]);
        assert_eq!(payload.len(), MAX_PAYLOAD);
        for id in ["large-a", "large-b"] {
            let batch =
                json!({"records":[{"id":id,"deviceId":"laptop","keyEpoch":1,"payload":payload}]});
            assert_eq!(
                request(&app, "POST", "/v1/records", Some(&credential), batch)
                    .await
                    .0,
                StatusCode::OK
            );
        }
        request(
            &app,
            "POST",
            "/v1/records",
            Some(&credential),
            json!({"records":[record("small-c","laptop","last")]}),
        )
        .await;
        let mut cursor = 0;
        let mut ids = Vec::new();
        loop {
            let request = Request::builder()
                .uri(format!("/v1/records?after={cursor}&limit=100"))
                .header("authorization", format!("Bearer {credential}"))
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), MAX_BODY + 1).await.unwrap();
            assert!(
                bytes.len() <= MAX_BODY,
                "feed exceeded its documented response bound"
            );
            let page: Value = serde_json::from_slice(&bytes).unwrap();
            let records = page["records"].as_array().unwrap();
            assert!(!records.is_empty());
            for record in records {
                assert_eq!(record["sequence"].as_i64().unwrap(), cursor + 1);
                cursor += 1;
                ids.push(record["id"].as_str().unwrap().to_owned());
            }
            assert_eq!(page["cursor"], cursor);
            if !page["hasMore"].as_bool().unwrap() {
                break;
            }
        }
        assert_eq!(ids, ["large-a", "large-b", "small-c"]);
    }

    #[test]
    fn failed_backups_and_existing_destinations_do_not_clobber_files_or_leave_partial_output() {
        let directory = tempfile::tempdir().unwrap();
        let corrupt = directory.path().join("corrupt.sqlite");
        std::fs::write(&corrupt, b"not a database").unwrap();
        let destination = directory.path().join("backup.sqlite");
        assert!(backup(&corrupt, &destination).is_err());
        assert!(!destination.exists());
        std::fs::write(&destination, b"existing owner data").unwrap();
        assert!(backup(&corrupt, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing owner data");
        assert!(!directory.path().read_dir().unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hession-relay-backup-")));
    }

    #[cfg(unix)]
    #[test]
    fn backup_refuses_symlink_destinations_and_protects_source_and_private_file_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (directory, _relay, _app) = setup(DEFAULT_QUOTA);
        let source = directory.path().join("relay.sqlite");
        let source_bytes = std::fs::read(&source).unwrap();
        assert!(backup(&source, &source).is_err());
        let hard_link = directory.path().join("hard-link.sqlite");
        std::fs::hard_link(&source, &hard_link).unwrap();
        assert!(backup(&source, &hard_link).is_err());
        let target = directory.path().join("owner-file");
        std::fs::write(&target, b"do not replace").unwrap();
        for (name, target) in [
            ("existing-link", &target),
            ("dangling-link", &directory.path().join("missing-target")),
        ] {
            let destination = directory.path().join(name);
            symlink(target, &destination).unwrap();
            assert!(backup(&source, &destination).is_err());
            assert_eq!(std::fs::read_link(&destination).unwrap(), *target);
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"do not replace");
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        let destination = directory.path().join("private-backup.sqlite");
        backup(&source, &destination).unwrap();
        assert_eq!(
            destination.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn simultaneous_backups_to_the_same_path_produce_one_intact_file() {
        let (directory, relay, _app) = setup(DEFAULT_QUOTA);
        relay
            .0
            .db
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TABLE padding(data BLOB); INSERT INTO padding VALUES(zeroblob(4194304));",
            )
            .unwrap();
        let source = directory.path().join("relay.sqlite");
        let destination = directory.path().join("raced-backup.sqlite");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let workers = (0..2)
            .map(|_| {
                let (source, destination, barrier) =
                    (source.clone(), destination.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    backup(&source, &destination)
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let restored = Connection::open(&destination).unwrap();
        assert_eq!(
            restored
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            restored
                .query_row("SELECT length(data) FROM padding", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4_194_304
        );
    }
}
