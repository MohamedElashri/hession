//! Optional private relay replication. Nothing starts automatically.
//!
//! Settings are device-local files, outside portable library backups. Secrets
//! live only in the platform credential store. Downloaded history is indexed in
//! Hession; the agent's own files are never written.
use crate::{db, AppState};
use hession_sync::{LibraryKey, RelayRecord, SessionContent, Snapshot, SnapshotChunk};
use reqwest::blocking::{Client, Response};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};
use tauri::State;
use zeroize::Zeroizing;

const SETTINGS_FILE: &str = "device-sync.json";
const SERVICE: &str = "sh.hession.private-sync";
const MAX_RESPONSE: u64 = 12 * 1024 * 1024;
const MAX_SETTINGS: usize = 64 * 1024;
type RecordedContext = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    enabled: bool,
    relay_url: String,
    vault_id: String,
    device_id: String,
    device_name: String,
    credential_ref: String,
    key_fingerprint: String,
    scope: Vec<String>,
    last_sync: Option<String>,
}

#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    running: bool,
    phase: String,
    uploaded: u64,
    downloaded: u64,
    error: Option<String>,
}

fn progress() -> &'static Mutex<Progress> {
    static VALUE: OnceLock<Mutex<Progress>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(Progress::default()))
}

fn config_lock() -> &'static Mutex<()> {
    static VALUE: OnceLock<Mutex<()>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(()))
}

fn sql<T>(v: rusqlite::Result<T>) -> Result<T, String> {
    v.map_err(|e| e.to_string())
}

fn config_path(path: &Path) -> Result<PathBuf, String> {
    Ok(path
        .parent()
        .ok_or("Invalid library directory")?
        .join(SETTINGS_FILE))
}

fn load(path: &Path) -> Result<Config, String> {
    let path = config_path(path)?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let file = fs::File::open(path).map_err(|_| "Could not read device sync settings")?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read device sync settings")?;
    if bytes.len() > MAX_SETTINGS {
        return Err("Device sync settings exceed size limit".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "Device sync settings are invalid; sync remains stopped".into())
}

fn save(path: &Path, config: &Config) -> Result<(), String> {
    let encoded = serde_json::to_vec(config).map_err(|_| "Could not encode settings")?;
    if encoded.len() > MAX_SETTINGS {
        return Err("Device sync settings exceed size limit. Choose fewer workspace paths.".into());
    }
    let destination = config_path(path)?;
    let parent = destination.parent().ok_or("Invalid settings directory")?;
    fs::create_dir_all(parent).map_err(|_| "Could not create settings directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not protect settings directory")?;
    }
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Could not prepare settings")?;
    file.write_all(&encoded)
        .map_err(|_| "Could not write device sync settings")?;
    file.flush()
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "Could not save settings")?;
    file.persist(&destination)
        .map_err(|_| "Could not replace device sync settings")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "Could not make device sync settings durable")?;
    Ok(())
}

fn configured(config: &Config) -> bool {
    !config.vault_id.is_empty() && !config.device_id.is_empty() && !config.credential_ref.is_empty()
}

fn require_enabled(config: &Config) -> Result<(), String> {
    if !config.enabled {
        return Err("Private device sync is disabled. Enable it in Settings first.".into());
    }
    if !configured(config) {
        return Err("Set up a private relay before enabling sync.".into());
    }
    Ok(())
}

fn validate_scope(scope: Vec<String>) -> Result<Vec<String>, String> {
    if scope.len() > 500
        || scope.iter().map(String::len).sum::<usize>() > 48 * 1024
        || scope
            .iter()
            .any(|s| s.is_empty() || s.len() > 4096 || s.contains('\0'))
    {
        return Err("Choose valid workspace paths for device sync".into());
    }
    let mut scope = scope;
    scope.sort();
    scope.dedup();
    Ok(scope)
}

fn relay_url(value: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(value.trim()).map_err(|_| "Enter a valid HTTPS relay URL")?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Relay URLs cannot contain credentials, query parameters, or fragments".into());
    }
    let local = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return Err(
            "Use HTTPS for your relay. HTTP is allowed only on loopback for local testing.".into(),
        );
    }
    if parsed.host_str().is_none() {
        return Err("Relay URL needs a host".into());
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize relay client".into())
}

fn response<T: DeserializeOwned>(result: reqwest::Result<Response>) -> Result<T, String> {
    let response = result.map_err(|e| {
        if e.is_timeout() {
            "Relay request timed out. Retry when reachable."
        } else {
            "Could not reach the relay. Check its URL, certificate, and network."
        }
    })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => {
                "Relay access was rejected. Check credentials or device enrollment.".into()
            }
            409 => {
                "The relay rejected a conflicting operation. Setup may already be complete; after restoring a relay backup, disable sync and use Replay after relay restore.".into()
            }
            413 => "The relay rejected an oversized transfer.".into(),
            code => format!("Relay returned HTTP {code}; no response body was logged."),
        });
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read relay response")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Relay response exceeds size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Relay returned an incompatible response".into())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secrets {
    device_token: String,
    recovery_key: String,
}

fn credential(reference: &str) -> Result<keyring::Entry, String> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Err("This platform does not have a configured secure credential store".into());
    }
    keyring::Entry::new(SERVICE, reference)
        .map_err(|_| "Could not access the OS credential store")
        .map_err(Into::into)
}

/// Explicit dependency injection keeps setup/recovery tests out of the user's
/// credential store. Production always uses the native OS implementation.
trait SecretStore {
    fn read(&self, reference: &str) -> Result<String, String>;
    fn write(&self, reference: &str, value: &str) -> Result<(), String>;
    fn remove(&self, reference: &str) -> Result<(), String>;
}

struct OsSecretStore;
impl SecretStore for OsSecretStore {
    fn read(&self, reference: &str) -> Result<String, String> {
        credential(reference)?.get_password().map_err(|_|"Sync credentials are unavailable. Unlock the OS credential store or re-pair this device.".into())
    }
    fn write(&self, reference: &str, value: &str) -> Result<(), String> {
        credential(reference)?.set_password(value).map_err(|_|"The OS credential store is locked or unavailable. Unlock it and retry; no plaintext fallback is used.".into())
    }
    fn remove(&self, reference: &str) -> Result<(), String> {
        match credential(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(
                "Could not remove credentials. Unlock the OS credential store and retry.".into(),
            ),
        }
    }
}

fn put_secrets_in(
    store: &dyn SecretStore,
    reference: &str,
    secrets: &Secrets,
) -> Result<(), String> {
    let text =
        Zeroizing::new(serde_json::to_string(secrets).map_err(|_| "Could not encode credentials")?);
    store.write(reference, &text)?;
    let saved = Zeroizing::new(store.read(reference)?);
    if *saved != *text {
        return Err("Could not verify secure credential storage".into());
    }
    Ok(())
}

fn get_secrets(config: &Config) -> Result<Secrets, String> {
    get_secrets_in(&OsSecretStore, config)
}

fn get_secrets_in(store: &dyn SecretStore, config: &Config) -> Result<Secrets, String> {
    let text = Zeroizing::new(store.read(&config.credential_ref)?);
    serde_json::from_str(&text).map_err(|_| "Stored sync credentials are invalid".into())
}

impl Drop for Secrets {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.device_token.zeroize();
        self.recovery_key.zeroize();
    }
}

fn path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|p| p.clone())
        .map_err(|_| "Could not access the library path".into())
}

fn settings(path: &Path) -> Result<Value, String> {
    let config = load(path)?;
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    let mut statement = sql(conn.prepare("SELECT COALESCE(w.path,s.cwd,''),COUNT(*) FROM session s LEFT JOIN workspace w ON w.id=s.workspace_id WHERE s.source_uri NOT LIKE 'hession-sync://%' GROUP BY 1 ORDER BY 1"))?;
    let workspaces: Vec<Value> = sql(sql(statement.query_map([], |r| {
        Ok(json!({"path":r.get::<_,String>(0)?,"count":r.get::<_,i64>(1)?}))
    }))?
    .collect())?;
    let status = progress().lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(
        json!({"enabled":config.enabled,"configured":configured(&config),"setupPending":!configured(&config) && !config.credential_ref.is_empty(),"relayUrl":config.relay_url,"deviceId":config.device_id,"deviceName":config.device_name,"workspaces":workspaces,"scope":config.scope,"keyFingerprint":config.key_fingerprint,"lastSync":config.last_sync,"status":status}),
    )
}

#[tauri::command]
pub async fn get_device_sync_settings(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || settings(&path))
        .await
        .map_err(|_| "Could not load device sync settings")?
}

#[tauri::command]
pub async fn test_device_sync_relay(relay_url: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let url = relay_url_checked(&relay_url)?;
        let value: Value = response(client()?.get(format!("{url}/v1/status")).send())?;
        if value["protocolVersion"].as_u64() != Some(1) {
            return Err("The relay protocol is incompatible with this Hession version".into());
        }
        Ok(value)
    })
    .await
    .map_err(|_| "Relay test stopped unexpectedly")?
}

fn relay_url_checked(value: &str) -> Result<String, String> {
    relay_url(value)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrollment {
    vault_id: String,
    device_id: String,
    device_token: String,
}

struct EnrollRequest {
    url: String,
    auth: String,
    device_name: String,
    workspaces: Vec<String>,
    recovery_key: Option<String>,
}

fn enroll(path: &Path, request: EnrollRequest) -> Result<Value, String> {
    enroll_with_store(path, request, &OsSecretStore)
}

fn enroll_with_store(
    path: &Path,
    request: EnrollRequest,
    store: &dyn SecretStore,
) -> Result<Value, String> {
    let _guard = config_lock().lock().map_err(|_| "Settings are busy")?;
    if RUNNING.load(Ordering::Acquire) {
        return Err("Stop device sync before changing setup".into());
    }
    if !load(path)?.credential_ref.is_empty() {
        return Err("This device already has sync setup. Export its recovery key, disable sync, and disconnect before pairing another library or retrying interrupted setup.".into());
    }
    let url = relay_url(&request.url)?;
    let scope = validate_scope(request.workspaces)?;
    if request.device_name.trim().is_empty() || request.device_name.len() > 128 {
        return Err("Enter a device name of 1–128 bytes".into());
    }
    let joining = request.recovery_key.is_some();
    let key = match &request.recovery_key {
        Some(s) => LibraryKey::from_recovery(s)?,
        None => LibraryKey::generate(),
    };
    let reference = uuid::Uuid::new_v4().to_string();
    let device_id = uuid::Uuid::new_v4().to_string();
    // Verify secure storage before changing anything on the relay.
    if let Err(error) = put_secrets_in(
        store,
        &reference,
        &Secrets {
            device_token: String::new(),
            recovery_key: key.recovery_key(),
        },
    ) {
        let _ = store.remove(&reference);
        return Err(error);
    }
    // Persist recoverable, disabled setup before the relay can initialize. A
    // timeout may happen after its transaction commits, so preserve the key
    // even on enrollment errors rather than orphaning an encrypted library.
    let pending = Config {
        relay_url: url.clone(),
        device_id: device_id.clone(),
        device_name: request.device_name.trim().into(),
        credential_ref: reference.clone(),
        key_fingerprint: key.fingerprint(),
        scope: scope.clone(),
        ..Default::default()
    };
    if let Err(error) = save(path, &pending) {
        let _ = store.remove(&reference);
        return Err(error);
    }
    let http = client()?;
    let mut body = json!({"deviceId":device_id,"deviceName":request.device_name.trim()});
    if joining {
        body["invitation"] = Value::String(request.auth.clone());
    }
    let call = if joining {
        http.post(format!("{url}/v1/devices/join")).json(&body)
    } else {
        http.post(format!("{url}/v1/bootstrap"))
            .bearer_auth(&request.auth)
            .json(&body)
    };
    let enrollment: Enrollment = response(call.send()).map_err(|error| format!("{error} Setup remains disabled. Its recovery key is saved securely; export it before disconnecting or retrying enrollment."))?;
    if enrollment.device_id != device_id
        || enrollment.vault_id.is_empty()
        || enrollment.device_token.len() < 32
    {
        return Err("Relay returned invalid enrollment credentials".into());
    }
    put_secrets_in(
        store,
        &reference,
        &Secrets {
            device_token: enrollment.device_token,
            recovery_key: key.recovery_key(),
        },
    )?;
    let config = Config {
        enabled: false,
        relay_url: url,
        vault_id: enrollment.vault_id,
        device_id,
        device_name: request.device_name.trim().into(),
        credential_ref: reference,
        key_fingerprint: key.fingerprint(),
        scope,
        last_sync: None,
    };
    save(path, &config)?;
    let result = settings(path)?;
    if joining {
        Ok(result)
    } else {
        Ok(json!({"settings":result,"recoveryKey":key.recovery_key()}))
    }
}

#[tauri::command]
pub async fn setup_device_sync(
    state: State<'_, AppState>,
    relay_url: String,
    bootstrap_token: String,
    device_name: String,
    workspaces: Vec<String>,
) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        enroll(
            &path,
            EnrollRequest {
                url: relay_url,
                auth: bootstrap_token,
                device_name,
                workspaces,
                recovery_key: None,
            },
        )
    })
    .await
    .map_err(|_| "Relay setup stopped unexpectedly")?
}

#[tauri::command]
pub async fn join_device_sync(
    state: State<'_, AppState>,
    relay_url: String,
    invitation: String,
    recovery_key: String,
    device_name: String,
    workspaces: Vec<String>,
) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        enroll(
            &path,
            EnrollRequest {
                url: relay_url,
                auth: invitation,
                device_name,
                workspaces,
                recovery_key: Some(recovery_key),
            },
        )
    })
    .await
    .map_err(|_| "Device pairing stopped unexpectedly")?
}

#[tauri::command]
pub async fn set_device_sync_enabled(
    state: State<'_, AppState>,
    enabled: bool,
    workspaces: Option<Vec<String>>,
) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || set_enabled(&path, enabled, workspaces))
        .await
        .map_err(|_| "Could not save device sync settings")?
}

fn set_enabled(
    path: &Path,
    enabled: bool,
    workspaces: Option<Vec<String>>,
) -> Result<Value, String> {
    let _guard = config_lock().lock().map_err(|_| "Settings are busy")?;
    let mut config = load(path)?;
    if enabled && !configured(&config) {
        return Err("Complete private relay setup first".into());
    }
    if RUNNING.load(Ordering::Acquire) && enabled {
        return Err("Stop device sync before changing its scope".into());
    }
    if let Some(scope) = workspaces {
        config.scope = validate_scope(scope)?;
    }
    config.enabled = enabled;
    save(path, &config)?;
    if !enabled {
        STOP.store(true, Ordering::Release);
    }
    settings(path)
}

#[tauri::command]
pub fn stop_device_sync() -> Value {
    let _lifecycle = config_lock().lock().unwrap_or_else(|e| e.into_inner());
    STOP.store(true, Ordering::Release);
    json!({"stopping":true})
}

#[tauri::command]
pub async fn create_device_sync_invitation(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let config = load(&path)?;
        require_enabled(&config)?;
        let secrets = get_secrets(&config)?;
        let mut value: Value = response(
            client()?
                .post(format!("{}/v1/invitations", config.relay_url))
                .bearer_auth(&secrets.device_token)
                .json(&json!({}))
                .send(),
        )?;
        value["keyFingerprint"] = Value::String(config.key_fingerprint);
        Ok(value)
    })
    .await
    .map_err(|_| "Could not create device invitation")?
}

/// Additive private-sync state. Call only for an explicit sync operation.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    sql(conn.execute_batch("CREATE TABLE IF NOT EXISTS replication_identity(local_session_id INTEGER PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,portable_id TEXT NOT NULL,head_revision TEXT,digest TEXT);
        CREATE INDEX IF NOT EXISTS replication_portable_idx ON replication_identity(portable_id);
        CREATE TABLE IF NOT EXISTS replication_revision(revision_id TEXT PRIMARY KEY,portable_id TEXT NOT NULL,parent_revision TEXT,content_digest TEXT NOT NULL,local_session_id INTEGER);
        CREATE TABLE IF NOT EXISTS replication_outbox(record_id TEXT PRIMARY KEY,vault_id TEXT NOT NULL,revision_id TEXT NOT NULL,record_json TEXT NOT NULL,workspace TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS replication_checkpoint(vault_id TEXT PRIMARY KEY,cursor INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS replication_publication(vault_id TEXT NOT NULL,local_session_id INTEGER NOT NULL,digest TEXT NOT NULL,revision_id TEXT NOT NULL,PRIMARY KEY(vault_id,local_session_id));
        CREATE TABLE IF NOT EXISTS replication_chunk(vault_id TEXT NOT NULL,snapshot_id TEXT NOT NULL,part INTEGER NOT NULL,total INTEGER NOT NULL,data BLOB NOT NULL,writer_id TEXT NOT NULL,PRIMARY KEY(vault_id,snapshot_id,part));"))?;
    let has_workspace:bool=sql(conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('replication_outbox') WHERE name='workspace')",[],|r|r.get(0)))?;
    if !has_workspace {
        sql(conn.execute_batch(
            "ALTER TABLE replication_outbox ADD COLUMN workspace TEXT NOT NULL DEFAULT '';",
        ))?;
    }
    Ok(())
}

fn check_cancel(path: &Path) -> Result<(), String> {
    if STOP.load(Ordering::Acquire) {
        return Err(
            "Device sync stopped. Completed transfers are saved; retry to continue.".into(),
        );
    }
    require_enabled(&load(path)?)
}

fn set_phase(phase: &str) {
    progress().lock().unwrap_or_else(|e| e.into_inner()).phase = phase.into();
}

#[tauri::command]
pub async fn export_device_sync_recovery_key(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let config = load(&path)?;
        if config.credential_ref.is_empty() {
            return Err("Set up private sync before exporting a recovery key".into());
        }
        let secrets = get_secrets(&config)?;
        Ok(json!({"recoveryKey":secrets.recovery_key,"keyFingerprint":config.key_fingerprint}))
    })
    .await
    .map_err(|_| "Could not export the recovery key")?
}

#[tauri::command]
pub async fn disconnect_device_sync(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = config_lock().lock().map_err(|_| "Settings are busy")?;
        let config = load(&path)?;
        if config.enabled || RUNNING.load(Ordering::Acquire) {
            return Err("Disable and stop device sync before disconnecting".into());
        }
        if !config.credential_ref.is_empty() {
            match credential(&config.credential_ref)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => (),
                Err(_) => {
                    return Err(
                        "Could not remove credentials. Unlock the OS credential store and retry."
                            .into(),
                    )
                }
            }
        }
        save(&path, &Config::default())?;
        settings(&path)
    })
    .await
    .map_err(|_| "Could not disconnect private sync")?
}

#[tauri::command]
pub async fn reset_device_sync_checkpoint(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || reset_checkpoint(&path))
        .await
        .map_err(|_| "Could not prepare relay replay")?
}

fn reset_checkpoint(path: &Path) -> Result<Value, String> {
    let _guard = config_lock().lock().map_err(|_| "Settings are busy")?;
    let config = load(path)?;
    if !configured(&config) || config.enabled || RUNNING.load(Ordering::Acquire) {
        return Err("Disable and stop private sync before replaying a restored relay".into());
    }
    let _maintenance = crate::sync::acquire_data_management_for(path)?;
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    let _write = db::user_write();
    init_schema(&conn)?;
    let tx = sql(conn.unchecked_transaction())?;
    for table in [
        "replication_checkpoint",
        "replication_chunk",
        "replication_publication",
    ] {
        sql(tx.execute(
            &format!("DELETE FROM {table} WHERE vault_id=?"),
            [&config.vault_id],
        ))?;
    }
    sql(tx.commit())?;
    settings(path)
}

fn workspace_sessions(conn: &Connection, scope: &[String]) -> Result<Vec<i64>, String> {
    let mut statement = sql(conn.prepare("SELECT s.id,COALESCE(w.path,s.cwd,'') FROM session s LEFT JOIN workspace w ON w.id=s.workspace_id ORDER BY s.id"))?;
    let rows = sql(statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))))?;
    let mut ids = Vec::new();
    for row in rows {
        let (id, workspace) = sql(row)?;
        if scope.contains(&workspace) {
            ids.push(id);
        }
    }
    Ok(ids)
}

#[tauri::command]
pub async fn preview_device_sync(
    state: State<'_, AppState>,
    workspaces: Vec<String>,
) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || preview_scope(&path, workspaces))
        .await
        .map_err(|_| "Could not preview device sync scope")?
}

fn preview_scope(path: &Path, workspaces: Vec<String>) -> Result<Value, String> {
    let scope = validate_scope(workspaces)?;
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    let ids = workspace_sessions(&conn, &scope)?;
    let mut entries = 0i64;
    let mut bytes = 0i64;
    for id in &ids {
        let (n, size): (i64,i64) = sql(conn.query_row("SELECT COUNT(DISTINCT e.id),COALESCE(SUM(length(CAST(cp.text AS BLOB))),0) FROM content_part cp JOIN entry e ON e.id=cp.entry_id WHERE e.session_id=? AND cp.kind='text' AND e.role IN ('user','assistant') AND lower(e.kind) NOT LIKE '%reason%' AND lower(e.kind) NOT LIKE '%thinking%'", [id], |r| Ok((r.get(0)?,r.get(1)?))))?;
        entries += n;
        bytes += size;
    }
    Ok(json!({"sessions":ids.len(),"entries":entries,"estimatedBytes":bytes}))
}

fn content(conn: &Connection, id: i64) -> Result<SessionContent, String> {
    let text_bytes:i64=sql(conn.query_row("SELECT COALESCE(SUM(length(CAST(cp.text AS BLOB))),0) FROM content_part cp JOIN entry e ON e.id=cp.entry_id WHERE e.session_id=? AND cp.kind='text' AND e.role IN ('user','assistant')",[id],|r|r.get(0)))?;
    if text_bytes as u64 > hession_sync::MAX_SNAPSHOT_BYTES as u64 {
        return Err(
            "Session text exceeds the 128 MiB snapshot limit. Exclude it from sync.".into(),
        );
    }
    let mut value = sql(conn.query_row("SELECT a.family,a.product,COALESCE(s.title,''),COALESCE(s.cwd,''),s.started_at,s.last_event_at,COALESCE(sa.starred,0) FROM session s JOIN agent a ON a.id=s.agent_id LEFT JOIN session_annotation sa ON sa.session_id=s.id WHERE s.id=?", [id], |r| Ok(SessionContent {
        family:r.get(0)?,product:r.get(1)?,title:r.get(2)?,cwd:r.get(3)?,started_at:r.get(4)?,last_event_at:r.get(5)?,branch:None,commit_sha:None,model_provider:None,model_key:None,starred:r.get::<_,i64>(6)? != 0,tags:Vec::new(),events:Vec::new(),omissions:vec!["Images and attachments are not transferred.".into(),"Private reasoning, system instructions, and raw harness metadata are omitted.".into(),"Tool inputs and outputs are omitted; only tool names, status, and exit codes are transferred.".into(),"Known credential patterns are redacted; arbitrary secrets cannot be detected reliably.".into()]
    })))?;
    let context:Option<RecordedContext>=sql(conn.query_row("SELECT c.branch,c.commit_sha,m.provider,m.model_key FROM session_context c LEFT JOIN model m ON m.id=c.model_id WHERE c.session_id=? ORDER BY c.id DESC LIMIT 1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
    if let Some((branch, commit, provider, model)) = context {
        value.branch = branch.map(|s| hession_sync::redact(&s));
        value.commit_sha = commit.map(|s| hession_sync::redact(&s));
        value.model_provider = provider.map(|s| hession_sync::redact(&s));
        value.model_key = model.map(|s| hession_sync::redact(&s));
    }
    value.title = hession_sync::redact(&value.title);
    value.cwd = hession_sync::redact(&value.cwd);
    let mut statement = sql(conn.prepare(
        "SELECT label FROM session_tag WHERE session_id=? ORDER BY label COLLATE NOCASE",
    ))?;
    value.tags = sql(sql(statement.query_map([id], |r| r.get::<_, String>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>())?
    .into_iter()
    .map(|s| hession_sync::redact(&s))
    .collect();
    let mut statement = sql(conn.prepare(
        "SELECT id,ordinal,kind,role,occurred_at FROM entry WHERE session_id=? ORDER BY ordinal",
    ))?;
    let mut rows = sql(statement.query([id]))?;
    while let Some(row) = sql(rows.next())? {
        let entry_id: i64 = sql(row.get(0))?;
        let role: Option<String> = sql(row.get(3))?;
        if matches!(role.as_deref(), Some("system" | "developer")) {
            continue;
        }
        let kind: String = sql(row.get(2))?;
        if kind.to_ascii_lowercase().contains("reason")
            || kind.to_ascii_lowercase().contains("thinking")
        {
            continue;
        }
        let mut texts = sql(conn.prepare("SELECT text FROM content_part WHERE entry_id=? AND kind='text' AND text IS NOT NULL ORDER BY ordinal"))?;
        let text = if matches!(role.as_deref(), Some("user" | "assistant")) {
            sql(sql(texts.query_map([entry_id], |r| r.get::<_, String>(0)))?
                .collect::<rusqlite::Result<Vec<_>>>())?
            .join("\n")
        } else {
            String::new()
        };
        let mut tools = sql(conn.prepare("SELECT tool_name,status,exit_code FROM tool_invocation WHERE session_id=? AND entry_id=? ORDER BY id"))?;
        let tools = sql(sql(tools.query_map(params![id, entry_id], |r| {
            Ok(hession_sync::Tool {
                name: r.get(0)?,
                status: r.get(1)?,
                exit_code: r.get(2)?,
                command: None,
                arguments: None,
                result: None,
            })
        }))?
        .collect::<rusqlite::Result<Vec<_>>>())?;
        if text.is_empty() && tools.is_empty() {
            continue;
        }
        value.events.push(hession_sync::Event {
            ordinal: sql(row.get(1))?,
            kind,
            role,
            occurred_at: sql(row.get(4))?,
            text: hession_sync::redact(&text),
            tools,
        });
        if value.events.len() > 200_000 {
            return Err("Session exceeds the 200,000 event limit. Exclude it from sync.".into());
        }
    }
    Ok(value)
}

fn content_digest(value: &SessionContent) -> Result<String, String> {
    Ok(hession_sync::digest(&[
        &serde_json::to_vec(value).map_err(|_| "Could not serialize portable session")?
    ]))
}

fn identity(
    conn: &Connection,
    id: i64,
) -> Result<(String, Option<String>, Option<String>), String> {
    let existing: Option<(String,Option<String>,Option<String>)> = sql(conn.query_row("SELECT portable_id,head_revision,digest FROM replication_identity WHERE local_session_id=?", [id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
    if let Some(value) = existing {
        return Ok(value);
    }
    let (family,external,source,started): (String,Option<String>,String,Option<String>) = sql(conn.query_row("SELECT a.family,s.external_id,s.source_uri,s.started_at FROM session s JOIN agent a ON a.id=s.agent_id WHERE s.id=?", [id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))))?;
    let portable = if let Some(rest) = source.strip_prefix("hession-sync://") {
        rest.split('/')
            .next()
            .ok_or("Invalid synced source identity")?
            .to_owned()
    } else if let Some(native) = external.filter(|s| !s.is_empty()) {
        hession_sync::digest(&[
            b"hession-session-v1",
            family.as_bytes(),
            native.as_bytes(),
            started.as_deref().unwrap_or("").as_bytes(),
        ])
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    sql(conn.execute(
        "INSERT INTO replication_identity(local_session_id,portable_id) VALUES(?,?)",
        params![id, portable],
    ))?;
    Ok((portable, None, None))
}

fn capture(path: &Path, config: &Config, key: &LibraryKey) -> Result<(), String> {
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    let ids = workspace_sessions(&conn, &config.scope)?;
    for id in ids {
        check_cancel(path)?;
        let _guard = db::background_write();
        let tx = sql(conn.unchecked_transaction())?;
        let value = content(&tx, id)?;
        let digest = content_digest(&value)?;
        let sent: Option<String> = sql(tx.query_row("SELECT digest FROM replication_publication WHERE vault_id=? AND local_session_id=?",params![config.vault_id,id],|r|r.get(0)).optional())?;
        if sent.as_deref() == Some(&digest) {
            continue;
        }
        let (portable, head, old_digest) = identity(&tx, id)?;
        let same = old_digest.as_deref() == Some(&digest);
        let revision_id = if same {
            head.clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let parent_revision = if same {
            sql(tx
                .query_row(
                    "SELECT parent_revision FROM replication_revision WHERE revision_id=?",
                    [&revision_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional())?
            .flatten()
        } else {
            head
        };
        let snapshot = Snapshot {
            version: 1,
            session_id: portable.clone(),
            revision_id: revision_id.clone(),
            parent_revision: parent_revision.clone(),
            content: value,
        };
        let workspace:String=sql(tx.query_row("SELECT COALESCE(w.path,s.cwd,'') FROM session s LEFT JOIN workspace w ON w.id=s.workspace_id WHERE s.id=?",[id],|r|r.get(0)))?;
        for chunk in hession_sync::chunks(&snapshot)? {
            let record = hession_sync::encrypt(
                key,
                &config.vault_id,
                &uuid::Uuid::new_v4().to_string(),
                &config.device_id,
                &serde_json::to_vec(&chunk).map_err(|_| "Could not encode snapshot chunk")?,
            )?;
            sql(tx.execute("INSERT INTO replication_outbox(record_id,vault_id,revision_id,record_json,workspace) VALUES(?,?,?,?,?)",params![record.id,config.vault_id,revision_id,serde_json::to_string(&record).map_err(|_| "Could not serialize encrypted record")?,workspace]))?;
        }
        sql(tx.execute("INSERT OR IGNORE INTO replication_revision(revision_id,portable_id,parent_revision,content_digest,local_session_id) VALUES(?,?,?,?,?)",params![revision_id,portable,parent_revision,digest,id]))?;
        sql(tx.execute(
            "UPDATE replication_identity SET head_revision=?,digest=? WHERE local_session_id=?",
            params![revision_id, digest, id],
        ))?;
        sql(tx.execute("INSERT INTO replication_publication(vault_id,local_session_id,digest,revision_id) VALUES(?,?,?,?) ON CONFLICT(vault_id,local_session_id) DO UPDATE SET digest=excluded.digest,revision_id=excluded.revision_id",params![config.vault_id,id,digest,revision_id]))?;
        sql(tx.commit())?;
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn validate_snapshot(value: &Snapshot) -> Result<(), String> {
    if value.version != 1
        || !valid_id(&value.session_id)
        || !valid_id(&value.revision_id)
        || value
            .parent_revision
            .as_deref()
            .is_some_and(|p| !valid_id(p))
        || value.content.events.len() > 200_000
        || value.parent_revision.as_deref() == Some(value.revision_id.as_str())
        || value.content.family.len() > 64
        || value.content.product.len() > 128
        || value.content.tags.len() > 1000
        || value.content.tags.iter().any(|s| s.len() > 256)
    {
        return Err("Invalid portable session snapshot".into());
    }
    if value.content.events.iter().any(|e| {
        e.ordinal < 0
            || matches!(e.role.as_deref(), Some("system" | "developer"))
            || e.tools.len() > 1000
            || (!e.text.is_empty() && !matches!(e.role.as_deref(), Some("user" | "assistant")))
            || e.tools.iter().any(|t| {
                t.name.len() > 512
                    || t.command.is_some()
                    || t.arguments.is_some()
                    || t.result.is_some()
            })
    }) {
        return Err("Unsupported event in portable session snapshot".into());
    }
    Ok(())
}

fn apply_snapshot(conn: &Connection, value: &Snapshot, vault_id: &str) -> Result<(), String> {
    validate_snapshot(value)?;
    if let Some(parent) = &value.parent_revision {
        let parent_session: Option<String> = sql(conn
            .query_row(
                "SELECT portable_id FROM replication_revision WHERE revision_id=?",
                [parent],
                |r| r.get(0),
            )
            .optional())?;
        if parent_session.is_some_and(|id| id != value.session_id) {
            return Err("A session revision cannot use another session's parent".into());
        }
        let cycle:bool=sql(conn.query_row("WITH RECURSIVE parents(id) AS (SELECT ?1 UNION SELECT r.parent_revision FROM replication_revision r JOIN parents p ON r.revision_id=p.id WHERE r.parent_revision IS NOT NULL) SELECT EXISTS(SELECT 1 FROM parents WHERE id=?2)",params![parent,value.revision_id],|r|r.get(0)))?;
        if cycle {
            return Err("Cyclic session revision ancestry is invalid".into());
        }
    }
    let digest = content_digest(&value.content)?;
    let remembered:Option<(String,Option<String>,String)>=sql(conn.query_row("SELECT portable_id,parent_revision,content_digest FROM replication_revision WHERE revision_id=?",[&value.revision_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
    if remembered.is_some_and(|(portable, parent, old_digest)| {
        portable != value.session_id || parent != value.parent_revision || old_digest != digest
    }) {
        return Err("Conflicting data for an existing session revision".into());
    }
    // Retained ancestry is only considered materialized while its associated
    // session still has the same portable identity. A deleted/reused row ID
    // must not make a replay silently discard the original history.
    let known: bool = sql(conn.query_row(
        "WITH RECURSIVE ancestry(revision_id,parent_revision) AS (
           SELECT head.revision_id,head.parent_revision FROM replication_revision original
           JOIN session s ON s.id=original.local_session_id
           JOIN replication_identity i ON i.local_session_id=s.id AND i.portable_id=original.portable_id
           JOIN replication_revision head ON head.revision_id=i.head_revision WHERE original.revision_id=?1
           UNION SELECT parent.revision_id,parent.parent_revision FROM replication_revision parent JOIN ancestry child ON parent.revision_id=child.parent_revision
         ) SELECT EXISTS(SELECT 1 FROM ancestry WHERE revision_id=?1)",
        [&value.revision_id],
        |r| r.get(0),
    ))?;
    if known {
        return Ok(());
    }
    let equal: Option<i64> = sql(conn.query_row("SELECT i.local_session_id FROM replication_identity i JOIN session s ON s.id=i.local_session_id WHERE i.portable_id=? AND i.digest=? LIMIT 1",params![value.session_id,digest],|r|r.get(0)).optional())?;
    let mut local_id = equal;
    if local_id.is_none() {
        // Only advance a remote copy whose exact head is this revision's parent.
        // Native-source rows and competing heads are never overwritten.
        let candidate: Option<i64> = if let Some(parent) = &value.parent_revision {
            sql(conn.query_row("SELECT i.local_session_id FROM replication_identity i JOIN session s ON s.id=i.local_session_id WHERE i.portable_id=? AND i.head_revision=? AND s.source_uri LIKE 'hession-sync://%' LIMIT 1",params![value.session_id,parent],|r|r.get(0)).optional())?
        } else {
            None
        };
        let ancestor = if let Some(id) = candidate {
            let actual = content_digest(&content(conn, id)?)?;
            let recorded: String = sql(conn.query_row(
                "SELECT digest FROM replication_identity WHERE local_session_id=?",
                [id],
                |r| r.get(0),
            ))?;
            if actual != recorded {
                // Download-only devices can have local annotation edits. They
                // must become a preserved local branch before applying remote
                // history, without being queued for upload outside consent.
                let revision = uuid::Uuid::new_v4().to_string();
                sql(conn.execute("INSERT INTO replication_revision(revision_id,portable_id,parent_revision,content_digest,local_session_id) VALUES(?,?,?,?,?)",params![revision,value.session_id,value.parent_revision,actual,id]))?;
                sql(conn.execute("UPDATE replication_identity SET head_revision=?,digest=? WHERE local_session_id=?",params![revision,actual,id]))?;
                None
            } else {
                Some(id)
            }
        } else {
            None
        };
        let agent = db::get_or_create_agent(
            conn,
            &value.content.family,
            &value.content.product,
            Some("hession-sync"),
        )
        .map_err(|e| e.to_string())?;
        let workspace =
            db::get_or_create_workspace(conn, &value.content.cwd).map_err(|e| e.to_string())?;
        let id = if let Some(id) = ancestor {
            sql(conn.execute("DELETE FROM tool_invocation WHERE session_id=?", [id]))?;
            sql(conn.execute("DELETE FROM file_activity WHERE session_id=?", [id]))?;
            sql(conn.execute("DELETE FROM session_context WHERE session_id=?", [id]))?;
            sql(conn.execute("DELETE FROM entry WHERE session_id=?", [id]))?;
            sql(conn.execute("DELETE FROM session_tag WHERE session_id=?", [id]))?;
            sql(conn.execute("UPDATE session SET agent_id=?,workspace_id=?,title=?,cwd=?,started_at=?,last_event_at=?,imported_at=CURRENT_TIMESTAMP WHERE id=?",params![agent,workspace,value.content.title,value.content.cwd,value.content.started_at,value.content.last_event_at,id]))?;
            id
        } else {
            sql(conn.execute("INSERT INTO session(agent_id,workspace_id,source_uri,source_format,title,cwd,started_at,last_event_at) VALUES(?,?,?,'hession-sync-v1',?,?,?,?)",params![agent,workspace,format!("hession-sync://{}/{}",value.session_id,value.revision_id),value.content.title,value.content.cwd,value.content.started_at,value.content.last_event_at]))?;
            conn.last_insert_rowid()
        };
        sql(conn.execute("INSERT INTO session_annotation(session_id,starred) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET starred=excluded.starred",params![id,value.content.starred]))?;
        for tag in &value.content.tags {
            sql(conn.execute(
                "INSERT OR IGNORE INTO session_tag(session_id,label) VALUES(?,?)",
                params![id, tag],
            ))?;
        }
        for event in &value.content.events {
            let payload = json!({"role":event.role,"content":[{"type":"text","text":event.text}]});
            sql(conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,occurred_at,search_text,payload_json) VALUES(?,?,?,?,?,?,?)",params![id,event.ordinal,event.kind,event.role,event.occurred_at,event.text,payload.to_string()]))?;
            let entry_id = conn.last_insert_rowid();
            if !event.text.is_empty() {
                sql(conn.execute(
                    "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
                    params![entry_id, event.text],
                ))?;
            }
            for tool in &event.tools {
                sql(conn.execute("INSERT INTO tool_invocation(session_id,entry_id,tool_name,status,exit_code) VALUES(?,?,?,?,?)",params![id,entry_id,tool.name,tool.status,tool.exit_code]))?;
            }
        }
        let model = match &value.content.model_key {
            Some(key) => db::get_or_create_model(
                conn,
                value.content.model_provider.as_deref().unwrap_or(""),
                key,
            )
            .map_err(|e| e.to_string())?,
            None => None,
        };
        if value.content.branch.is_some() || value.content.commit_sha.is_some() || model.is_some() {
            sql(conn.execute("INSERT INTO session_context(session_id,observed_at,cwd,branch,commit_sha,model_id) VALUES(?,?,?,?,?,?)",params![id,value.content.last_event_at,value.content.cwd,value.content.branch,value.content.commit_sha,model]))?;
        }
        local_id = Some(id);
    }
    let id = local_id.ok_or("Could not identify imported session")?;
    sql(conn.execute("INSERT INTO replication_identity(local_session_id,portable_id,head_revision,digest) VALUES(?,?,?,?) ON CONFLICT(local_session_id) DO UPDATE SET head_revision=excluded.head_revision,digest=excluded.digest",params![id,value.session_id,value.revision_id,digest]))?;
    sql(conn.execute("INSERT INTO replication_revision(revision_id,portable_id,parent_revision,content_digest,local_session_id) VALUES(?,?,?,?,?) ON CONFLICT(revision_id) DO UPDATE SET local_session_id=excluded.local_session_id",params![value.revision_id,value.session_id,value.parent_revision,digest,id]))?;
    sql(conn.execute("INSERT INTO replication_publication(vault_id,local_session_id,digest,revision_id) VALUES(?,?,?,?) ON CONFLICT(vault_id,local_session_id) DO UPDATE SET digest=excluded.digest,revision_id=excluded.revision_id",params![vault_id,id,digest,value.revision_id]))?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    records: Vec<FeedRecord>,
    cursor: i64,
    has_more: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedRecord {
    sequence: i64,
    id: String,
    device_id: String,
    key_epoch: u32,
    payload: String,
}

fn apply_chunk(
    conn: &Connection,
    config: &Config,
    key: &LibraryKey,
    record: &FeedRecord,
) -> Result<(), String> {
    let portable = RelayRecord {
        id: record.id.clone(),
        device_id: record.device_id.clone(),
        key_epoch: record.key_epoch,
        payload: record.payload.clone(),
    };
    let plaintext = Zeroizing::new(hession_sync::decrypt(key, &config.vault_id, &portable)?);
    let chunk: SnapshotChunk =
        serde_json::from_slice(&plaintext).map_err(|_| "Invalid encrypted session chunk")?;
    let bytes = chunk.bytes()?;
    let existing: Option<(u32,String)> = sql(conn.query_row("SELECT total,writer_id FROM replication_chunk WHERE vault_id=? AND snapshot_id=? LIMIT 1",params![config.vault_id,chunk.snapshot_id],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
    if existing.is_some_and(|(total, _)| total != chunk.total) {
        return Err("Inconsistent session chunk metadata".into());
    }
    let duplicate: Option<Vec<u8>> = sql(conn
        .query_row(
            "SELECT data FROM replication_chunk WHERE vault_id=? AND snapshot_id=? AND part=?",
            params![config.vault_id, chunk.snapshot_id, chunk.index],
            |r| r.get(0),
        )
        .optional())?;
    if duplicate.is_some_and(|old| old != bytes) {
        return Err("Conflicting data for an existing session chunk".into());
    }
    sql(conn.execute("INSERT OR IGNORE INTO replication_chunk(vault_id,snapshot_id,part,total,data,writer_id) VALUES(?,?,?,?,?,?)",params![config.vault_id,chunk.snapshot_id,chunk.index,chunk.total,bytes,record.device_id]))?;
    let count: u32 = sql(conn.query_row(
        "SELECT COUNT(*) FROM replication_chunk WHERE vault_id=? AND snapshot_id=?",
        params![config.vault_id, chunk.snapshot_id],
        |r| r.get(0),
    ))?;
    if count == chunk.total {
        let mut statement = sql(conn.prepare(
            "SELECT data FROM replication_chunk WHERE vault_id=? AND snapshot_id=? ORDER BY part",
        ))?;
        let parts = sql(sql(statement
            .query_map(params![config.vault_id, chunk.snapshot_id], |r| {
                r.get::<_, Vec<u8>>(0)
            }))?
        .collect::<rusqlite::Result<Vec<_>>>())?;
        let mut assembled = Zeroizing::new(Vec::new());
        for part in parts {
            assembled.extend_from_slice(&part);
            if assembled.len() > hession_sync::MAX_SNAPSHOT_BYTES {
                return Err("Session exceeds snapshot size limit".into());
            }
        }
        let snapshot: Snapshot =
            serde_json::from_slice(&assembled).map_err(|_| "Invalid session snapshot")?;
        if snapshot.revision_id != chunk.snapshot_id {
            return Err("Session revision does not match its chunk identity".into());
        }
        apply_snapshot(conn, &snapshot, &config.vault_id)?;
        sql(conn.execute(
            "DELETE FROM replication_chunk WHERE vault_id=? AND snapshot_id=?",
            params![config.vault_id, chunk.snapshot_id],
        ))?;
    }
    Ok(())
}

fn pull(
    path: &Path,
    config: &Config,
    key: &LibraryKey,
    secrets: &Secrets,
    http: &Client,
) -> Result<(), String> {
    set_phase("Downloading");
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    loop {
        check_cancel(path)?;
        let cursor: i64 = sql(conn
            .query_row(
                "SELECT cursor FROM replication_checkpoint WHERE vault_id=?",
                [&config.vault_id],
                |r| r.get(0),
            )
            .optional())?
        .unwrap_or(0);
        let feed: Feed = response(
            http.get(format!("{}/v1/records", config.relay_url))
                .query(&[("after", cursor), ("limit", 32)])
                .bearer_auth(&secrets.device_token)
                .send(),
        )?;
        if feed.cursor < cursor
            || feed.records.len() > 100
            || (feed.has_more && feed.records.is_empty())
        {
            return Err("Relay returned an invalid change feed".into());
        }
        let _guard = db::background_write();
        let tx = sql(conn.unchecked_transaction())?;
        let mut previous = cursor;
        for record in &feed.records {
            check_cancel(path)?;
            if record.sequence != previous.saturating_add(1) || record.sequence > feed.cursor {
                return Err("Relay returned out-of-order records".into());
            }
            apply_chunk(&tx, config, key, record)?;
            previous = record.sequence;
        }
        if feed.cursor != previous {
            return Err("Relay skipped records in its checkpoint".into());
        }
        sql(tx.execute("INSERT INTO replication_checkpoint(vault_id,cursor) VALUES(?,?) ON CONFLICT(vault_id) DO UPDATE SET cursor=excluded.cursor",params![config.vault_id,feed.cursor]))?;
        sql(tx.commit())?;
        progress()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .downloaded += feed.records.len() as u64;
        if !feed.has_more {
            break;
        }
    }
    Ok(())
}

/// Scope always applies to pending ciphertext as well as newly captured rows.
/// Dropped snapshots remain in the local library; clearing their publication
/// marker lets an explicit later opt-in queue the current snapshot again.
fn prune_outbox(conn: &Connection, config: &Config) -> Result<(), String> {
    let selected = workspace_sessions(conn, &config.scope)?;
    let mut statement=sql(conn.prepare("SELECT o.record_id,r.local_session_id,r.portable_id,i.portable_id,o.workspace,o.revision_id FROM replication_outbox o LEFT JOIN replication_revision r ON r.revision_id=o.revision_id LEFT JOIN replication_identity i ON i.local_session_id=r.local_session_id WHERE o.vault_id=?"))?;
    let rows = sql(sql(statement.query_map([&config.vault_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    }))?
    .collect::<rusqlite::Result<Vec<_>>>())?;
    for (record, id, portable, current, workspace, revision) in rows {
        let allowed = id.is_some_and(|id| selected.contains(&id))
            && config.scope.contains(&workspace)
            && portable.is_some()
            && portable == current;
        if !allowed {
            sql(conn.execute(
                "DELETE FROM replication_outbox WHERE record_id=? AND vault_id=?",
                params![record, config.vault_id],
            ))?;
            sql(conn.execute(
                "DELETE FROM replication_publication WHERE vault_id=? AND revision_id=?",
                params![config.vault_id, revision],
            ))?;
        }
    }
    Ok(())
}

fn push(
    path: &Path,
    config: &Config,
    key: &LibraryKey,
    secrets: &Secrets,
    http: &Client,
) -> Result<(), String> {
    set_phase("Uploading");
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    {
        let _guard = db::background_write();
        let tx = sql(conn.unchecked_transaction())?;
        prune_outbox(&tx, config)?;
        sql(tx.commit())?;
    }
    loop {
        check_cancel(path)?;
        // One bounded record per request keeps memory and HTTP bodies predictable.
        let row:Option<(String,String)>=sql(conn.query_row("SELECT record_id,record_json FROM replication_outbox WHERE vault_id=? ORDER BY rowid LIMIT 1",[&config.vault_id],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
        let Some((id, encoded)) = row else {
            break;
        };
        let mut record: RelayRecord =
            serde_json::from_str(&encoded).map_err(|_| "Queued encrypted record is invalid")?;
        if record.device_id != config.device_id {
            // A re-paired device can preserve its old pending snapshots after
            // authenticating them with the same library key. Replace only the
            // envelope writer and random relay ID, never the snapshot itself.
            let plaintext = Zeroizing::new(hession_sync::decrypt(key, &config.vault_id, &record)?);
            let replacement = hession_sync::encrypt(
                key,
                &config.vault_id,
                &uuid::Uuid::new_v4().to_string(),
                &config.device_id,
                &plaintext,
            )?;
            let _guard = db::background_write();
            let tx = sql(conn.unchecked_transaction())?;
            sql(tx.execute("UPDATE replication_outbox SET record_id=?,record_json=? WHERE record_id=? AND vault_id=?",params![replacement.id,serde_json::to_string(&replacement).map_err(|_|"Could not rewrap queued record")?,id,config.vault_id]))?;
            sql(tx.commit())?;
            record = replacement;
        }
        let reply: Value = response(
            http.post(format!("{}/v1/records", config.relay_url))
                .bearer_auth(&secrets.device_token)
                .json(&json!({"records":[record]}))
                .send(),
        )?;
        if reply["accepted"].as_u64() != Some(1) {
            return Err("Relay did not acknowledge the upload; it remains queued".into());
        }
        let _guard = db::background_write();
        sql(conn.execute(
            "DELETE FROM replication_outbox WHERE record_id=? AND vault_id=?",
            params![record.id, config.vault_id],
        ))?;
        progress()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .uploaded += 1;
    }
    Ok(())
}

fn transfer(path: &Path, config: &Config, secrets: &Secrets) -> Result<(), String> {
    let key = LibraryKey::from_recovery(&secrets.recovery_key)?;
    if key.fingerprint() != config.key_fingerprint {
        return Err("Stored recovery key fingerprint changed. Sync stopped.".into());
    }
    let http = client()?;
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    {
        let _guard = db::background_write();
        init_schema(&conn)?;
    }
    // Authenticate the key before preparing uploads, but do not apply remote
    // changes until local offline edits have become immutable revisions.
    check_cancel(path)?;
    let probe: Feed = response(
        http.get(format!("{}/v1/records", config.relay_url))
            .query(&[("after", 0), ("limit", 1)])
            .bearer_auth(&secrets.device_token)
            .send(),
    )?;
    if let Some(record) = probe.records.first() {
        let _plaintext = Zeroizing::new(hession_sync::decrypt(
            &key,
            &config.vault_id,
            &RelayRecord {
                id: record.id.clone(),
                device_id: record.device_id.clone(),
                key_epoch: record.key_epoch,
                payload: record.payload.clone(),
            },
        )?);
    }
    set_phase("Preparing uploads");
    capture(path, config, &key)?;
    pull(path, config, &key, secrets, &http)?;
    push(path, config, &key, secrets, &http)?;
    pull(path, config, &key, secrets, &http)
}

fn run(path: &Path) -> Result<Value, String> {
    run_with_store(path, &OsSecretStore)
}

fn run_with_store(path: &Path, store: &dyn SecretStore) -> Result<Value, String> {
    let config = {
        let _lifecycle = config_lock().lock().map_err(|_| "Settings are busy")?;
        let config = load(path)?;
        require_enabled(&config)?;
        if RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Device sync is already running".into());
        }
        STOP.store(false, Ordering::Release);
        *progress().lock().unwrap_or_else(|e| e.into_inner()) = Progress {
            running: true,
            phase: "Preparing".into(),
            ..Default::default()
        };
        config
    };
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _lifecycle = config_lock().lock().unwrap_or_else(|e| e.into_inner());
            progress().lock().unwrap_or_else(|e| e.into_inner()).running = false;
            RUNNING.store(false, Ordering::Release);
        }
    }
    let _running = Guard;
    let result = (|| {
        // Share the existing maintenance gate for the whole operation, not
        // only individual SQLite writes. Imports, reset/backup/restore, and
        // updater restart must not invalidate a network sync's live state.
        let _maintenance = crate::sync::acquire_data_management_for(path).map_err(|_| {
            "Finish refreshing histories or managing saved data before syncing devices."
        })?;
        let secrets = get_secrets_in(store, &config)?;
        transfer(path, &config, &secrets)?;
        let _lock = config_lock().lock().map_err(|_| "Settings are busy")?;
        let mut current = load(path)?;
        if current.vault_id == config.vault_id && current.device_id == config.device_id {
            current.last_sync = Some(chrono::Utc::now().to_rfc3339());
            save(path, &current)?;
        }
        crate::analytics::request_refresh(path, false);
        Ok(())
    })();
    {
        let mut status = progress().lock().unwrap_or_else(|e| e.into_inner());
        status.running = false;
        status.phase = if result.is_ok() {
            "Complete"
        } else {
            "Stopped"
        }
        .into();
        status.error = result.as_ref().err().cloned();
    }
    result?;
    settings(path)
}

#[tauri::command]
pub async fn run_device_sync(state: State<'_, AppState>) -> Result<Value, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || run(&path))
        .await
        .map_err(|_| "Device sync stopped unexpectedly")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{atomic::AtomicUsize, Arc},
    };
    static TEST_NETWORK_LOCK: Mutex<()> = Mutex::new(());
    type SecretWriteHook = Box<dyn Fn(usize) + Send + Sync>;

    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, Zeroizing<String>>>,
        writes: AtomicUsize,
        fail_write: AtomicUsize,
        on_write: Mutex<Option<SecretWriteHook>>,
    }
    impl SecretStore for MemoryStore {
        fn read(&self, reference: &str) -> Result<String, String> {
            self.values
                .lock()
                .unwrap()
                .get(reference)
                .map(|s| s.to_string())
                .ok_or_else(|| "Injected credential is missing".into())
        }
        fn write(&self, reference: &str, value: &str) -> Result<(), String> {
            let n = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_write.load(Ordering::SeqCst) == n {
                return Err("Injected credential write failure".into());
            }
            self.values
                .lock()
                .unwrap()
                .insert(reference.into(), Zeroizing::new(value.into()));
            if let Some(callback) = self.on_write.lock().unwrap().as_ref() {
                callback(n);
            }
            Ok(())
        }
        fn remove(&self, reference: &str) -> Result<(), String> {
            self.values.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn fixture(path: &Path, native: &str) -> i64 {
        let conn = db::open_db(path).unwrap();
        let agent = db::get_or_create_agent(&conn, "claude", "Claude Code", None).unwrap();
        let workspace = db::get_or_create_workspace(&conn, "/work/project").unwrap();
        conn.execute("INSERT INTO session(agent_id,workspace_id,external_id,source_uri,cwd,title,started_at,last_event_at) VALUES(?,?,?,?,'/work/project','Private task','2026-11-12T10:00:00Z','2026-11-12T10:01:00Z')",params![agent,workspace,native,format!("claude:///local/{native}.jsonl")]).unwrap();
        let id = conn.last_insert_rowid();
        let model = db::get_or_create_model(&conn, "anthropic", "claude-test").unwrap();
        conn.execute("INSERT INTO session_context(session_id,cwd,branch,commit_sha,model_id) VALUES(?,'/work/project','feature/private-sync','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',?)",params![id,model]).unwrap();
        append(&conn, id, 0, "user", "Please fix the parser.");
        append(&conn, id, 1, "assistant", "I found the boundary case.");
        id
    }

    fn append(conn: &Connection, id: i64, ordinal: i64, role: &str, text: &str) {
        conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text,payload_json) VALUES(?,?,'message',?,?,?)",params![id,ordinal,role,text,json!({"neverExportRaw":"hidden-raw-secret"}).to_string()]).unwrap();
        let entry = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            params![entry, text],
        )
        .unwrap();
    }

    fn config_for(
        path: &Path,
        url: &str,
        enrollment: Enrollment,
        key: &LibraryKey,
    ) -> (Config, Secrets) {
        let config = Config {
            enabled: true,
            relay_url: url.into(),
            vault_id: enrollment.vault_id,
            device_id: enrollment.device_id,
            device_name: "Test device".into(),
            credential_ref: "test-injected-no-keychain".into(),
            key_fingerprint: key.fingerprint(),
            scope: vec!["/work/project".into()],
            last_sync: None,
        };
        save(path, &config).unwrap();
        let secrets = Secrets {
            device_token: enrollment.device_token,
            recovery_key: key.recovery_key(),
        };
        (config, secrets)
    }

    struct Server {
        url: String,
        shutdown: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn start(path: &Path) -> Self {
            let relay = hession_relay::Relay::open(
                path,
                "a-test-administrator-token-at-least-32-characters",
                hession_relay::DEFAULT_QUOTA,
            )
            .unwrap();
            Self::with_router(hession_relay::router(relay))
        }

        fn with_router(router: axum::Router) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("bind disposable test relay on loopback");
            let url = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let (shutdown, rx) = tokio::sync::oneshot::channel();
            let thread = std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, router)
                        .with_graceful_shutdown(async {
                            let _ = rx.await;
                        })
                        .await
                        .unwrap();
                });
            });
            Self {
                url,
                shutdown: Some(shutdown),
                thread: Some(thread),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(tx) = self.shutdown.take() {
                let _ = tx.send(());
            }
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    #[test]
    fn disabled_default_denies_before_credentials_or_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let settings = settings(&path).unwrap();
        assert_eq!(settings["enabled"], false);
        assert_eq!(settings["configured"], false);
        assert!(run(&path).unwrap_err().contains("disabled"));
        assert!(!config_path(&path).unwrap().exists());
        let conn = db::open_db(&path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'replication_%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn relay_url_requires_authenticated_transport_without_redirect_credentials() {
        assert!(relay_url("https://sync.example.test/prefix/").is_ok());
        assert!(relay_url("http://127.0.0.1:8744").is_ok());
        for url in [
            "http://example.test",
            "file:///tmp/foo",
            "https://user:secret@example.test",
            "https://example.test?token=secret",
            "https://example.test/#secret",
        ] {
            assert!(relay_url(url).is_err(), "{url}");
        }
        assert!(validate_scope(vec![String::new()]).is_err());
        assert!(validate_scope(Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn oversized_settings_cannot_replace_a_readable_disabled_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        save(&path, &Config::default()).unwrap();
        let original = fs::read(config_path(&path).unwrap()).unwrap();
        let too_large = Config {
            device_name: "x".repeat(MAX_SETTINGS),
            ..Default::default()
        };
        assert!(save(&path, &too_large).unwrap_err().contains("size limit"));
        assert_eq!(fs::read(config_path(&path).unwrap()).unwrap(), original);
        assert!(!load(&path).unwrap().enabled);
        let scope = (0..20)
            .map(|i| format!("/{i}{}", "x".repeat(3000)))
            .collect();
        assert!(validate_scope(scope).is_err());
    }

    #[test]
    fn large_unicode_preview_counts_bytes_and_events_instead_of_text_parts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("library.sqlite");
        let id = fixture(&path, "unicode-preview");
        let conn = db::open_db(&path).unwrap();
        conn.execute("DELETE FROM entry WHERE session_id=?", [id])
            .unwrap();
        let text = "🦀漢字\0Ω".repeat(100_000);
        append(&conn, id, 0, "user", &text);
        let entry = conn.last_insert_rowid();
        conn.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,1,'text','second text part')", [entry]).unwrap();
        let value = preview_scope(&path, vec!["/work/project".into()]).unwrap();
        assert_eq!(value["sessions"], 1);
        assert_eq!(value["entries"], 1, "multiple parts belong to one event");
        assert_eq!(
            value["estimatedBytes"],
            text.len() + "second text part".len(),
            "UTF-8 bytes, including embedded NULs, must match transfer accounting"
        );
        assert_eq!(
            preview_scope(&path, Vec::new()).unwrap()["estimatedBytes"],
            0
        );
    }

    #[test]
    fn large_unicode_history_resumes_partial_chunks_across_feed_pages_without_duplicates() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        STOP.store(false, Ordering::Release);
        let root = tempfile::tempdir().unwrap();
        let server = Server::start(&root.path().join("relay.sqlite"));
        let http = client().unwrap();
        let enrollment: Enrollment = response(http.post(format!("{}/v1/bootstrap",server.url))
            .bearer_auth("a-test-administrator-token-at-least-32-characters")
            .json(&json!({"deviceId":"large-history-writer","deviceName":"Large history fixture"})).send()).unwrap();
        let path = root.path().join("sender/library.sqlite");
        let id = fixture(&path, "large-native-history");
        let conn = db::open_db(&path).unwrap();
        let text = "🦀 Unicode monorepo context 漢字\0\n".repeat(250_000);
        {
            let tx = conn.unchecked_transaction().unwrap();
            for ordinal in 2..10_002 {
                append(
                    &tx,
                    id,
                    ordinal,
                    "assistant",
                    &format!("Synthetic progress update {ordinal}"),
                );
            }
            append(&tx, id, 10_002, "user", &text);
            tx.commit().unwrap();
        }
        let key = LibraryKey::generate();
        let (sender, secrets) = config_for(&path, &server.url, enrollment, &key);
        transfer(&path, &sender, &secrets).unwrap();
        let feed: Feed = response(
            http.get(format!("{}/v1/records", server.url))
                .query(&[("after", 0), ("limit", 32)])
                .bearer_auth(&secrets.device_token)
                .send(),
        )
        .unwrap();
        assert!(
            feed.has_more,
            "large histories must exercise byte-bounded feed pagination"
        );
        // Restore this same authorized device's local cache after an interrupted
        // first page. No enrollment or OS credential storage is involved.
        let restored = root.path().join("restored/library.sqlite");
        let mut receiver = sender.clone();
        receiver.scope.clear();
        save(&restored, &receiver).unwrap();
        {
            let conn = db::open_db(&restored).unwrap();
            init_schema(&conn).unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            let record = &feed.records[0];
            apply_chunk(&tx, &receiver, &key, record).unwrap();
            tx.execute(
                "INSERT INTO replication_checkpoint(vault_id,cursor) VALUES(?,?)",
                params![receiver.vault_id, record.sequence],
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0,
                "partial snapshots must not appear as sessions"
            );
        }
        transfer(&restored, &receiver, &secrets).unwrap();
        transfer(&restored, &receiver, &secrets).unwrap();
        let conn = db::open_db(&restored).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            10_003
        );
        let recovered: String = conn.query_row("SELECT text FROM content_part cp JOIN entry e ON e.id=cp.entry_id WHERE e.ordinal=10002",[],|r|r.get(0)).unwrap();
        assert_eq!(
            recovered, text,
            "multi-byte Unicode and embedded NULs must survive chunk and page boundaries"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM replication_chunk", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn enrollment_join_and_enable_use_isolated_credentials_and_never_implicitly_sync() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(&dir.path().join("relay.sqlite"));
        let store = MemoryStore::default();
        let path = dir.path().join("a/library.sqlite");
        fixture(&path, "full-enrollment-native");
        let created = enroll_with_store(
            &path,
            EnrollRequest {
                url: server.url.clone(),
                auth: "a-test-administrator-token-at-least-32-characters".into(),
                device_name: "Laptop".into(),
                workspaces: vec!["/work/project".into()],
                recovery_key: None,
            },
            &store,
        )
        .unwrap();
        assert_eq!(created["settings"]["enabled"], false);
        assert_eq!(created["settings"]["configured"], true);
        let config = load(&path).unwrap();
        let secrets = get_secrets_in(&store, &config).unwrap();
        let disk = fs::read_to_string(config_path(&path).unwrap()).unwrap();
        for value in [&secrets.device_token, &secrets.recovery_key] {
            assert!(!disk.contains(value));
        }
        assert!(!disk.contains("a-test-administrator-token"));
        assert!(run_with_store(&path, &store)
            .unwrap_err()
            .contains("disabled"));
        let http = client().unwrap();
        let feed: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets.device_token)
                .send(),
        )
        .unwrap();
        assert!(
            feed["records"].as_array().unwrap().is_empty(),
            "setup must never upload selected histories"
        );
        set_enabled(&path, true, Some(Vec::new())).unwrap();
        run_with_store(&path, &store).unwrap();
        let feed: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets.device_token)
                .send(),
        )
        .unwrap();
        assert!(
            feed["records"].as_array().unwrap().is_empty(),
            "empty scope must remain download-only after enable"
        );
        set_enabled(&path, true, Some(vec!["/work/project".into()])).unwrap();
        run_with_store(&path, &store).unwrap();
        let invitation: Value = response(
            http.post(format!("{}/v1/invitations", server.url))
                .bearer_auth(&secrets.device_token)
                .json(&json!({}))
                .send(),
        )
        .unwrap();
        let paired = dir.path().join("b/library.sqlite");
        let joined = enroll_with_store(
            &paired,
            EnrollRequest {
                url: server.url.clone(),
                auth: invitation["invitation"].as_str().unwrap().into(),
                device_name: "Desktop".into(),
                workspaces: Vec::new(),
                recovery_key: Some(created["recoveryKey"].as_str().unwrap().into()),
            },
            &store,
        )
        .unwrap();
        assert_eq!(joined["enabled"], false);
        assert_eq!(
            joined["keyFingerprint"],
            created["settings"]["keyFingerprint"]
        );
        assert!(run_with_store(&paired, &store)
            .unwrap_err()
            .contains("disabled"));
        set_enabled(&paired, true, None).unwrap();
        run_with_store(&paired, &store).unwrap();
        let conn = db::open_db(&paired).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!RUNNING.load(Ordering::Acquire));
        assert!(!crate::sync::is_managing_data());
        assert!(!progress().lock().unwrap().running);
    }

    #[test]
    fn setup_failures_preserve_recovery_and_never_initialize_before_durable_local_setup() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(&dir.path().join("relay.sqlite"));
        let request = || EnrollRequest {
            url: server.url.clone(),
            auth: "a-test-administrator-token-at-least-32-characters".into(),
            device_name: "Laptop".into(),
            workspaces: Vec::new(),
            recovery_key: None,
        };
        let store = MemoryStore::default();
        store.fail_write.store(1, Ordering::SeqCst);
        let first = dir.path().join("first/library.sqlite");
        assert!(enroll_with_store(&first, request(), &store)
            .unwrap_err()
            .contains("Injected credential"));
        assert!(!config_path(&first).unwrap().exists());
        assert!(store.values.lock().unwrap().is_empty());
        let http = client().unwrap();
        let status: Value = response(http.get(format!("{}/v1/status", server.url)).send()).unwrap();
        assert_eq!(status["initialized"], false);

        let blocked = dir.path().join("blocked/library.sqlite");
        let blocked_config = config_path(&blocked).unwrap();
        let blocked_store = MemoryStore::default();
        *blocked_store.on_write.lock().unwrap() = Some(Box::new(move |n| {
            if n == 1 {
                fs::create_dir_all(&blocked_config).unwrap();
            }
        }));
        assert!(enroll_with_store(&blocked, request(), &blocked_store)
            .unwrap_err()
            .contains("replace"));
        assert!(
            blocked_store.values.lock().unwrap().is_empty(),
            "failed local persistence must remove the unreferenced key"
        );
        let status: Value = response(http.get(format!("{}/v1/status", server.url)).send()).unwrap();
        assert_eq!(status["initialized"], false);

        // The relay can commit enrollment just before the credential store
        // becomes locked. A pending setup must still expose the original key
        // for recovery and must refuse enabling or generating a replacement.
        let pending = dir.path().join("pending/library.sqlite");
        let late_store = MemoryStore::default();
        late_store.fail_write.store(2, Ordering::SeqCst);
        assert!(enroll_with_store(&pending, request(), &late_store)
            .unwrap_err()
            .contains("Injected credential"));
        let state = settings(&pending).unwrap();
        assert_eq!(state["setupPending"], true);
        assert_eq!(state["enabled"], false);
        assert_eq!(state["configured"], false);
        let config = load(&pending).unwrap();
        let recovered = get_secrets_in(&late_store, &config).unwrap();
        assert_eq!(
            LibraryKey::from_recovery(&recovered.recovery_key)
                .unwrap()
                .fingerprint(),
            config.key_fingerprint
        );
        assert!(set_enabled(&pending, true, None).is_err());
        assert!(enroll_with_store(&pending, request(), &late_store)
            .unwrap_err()
            .contains("Export"));
        assert_eq!(late_store.writes.load(Ordering::SeqCst), 2);
        let status: Value = response(http.get(format!("{}/v1/status", server.url)).send()).unwrap();
        assert_eq!(status["initialized"], true);
    }

    #[test]
    fn malicious_feed_pages_cannot_skip_records_or_partially_commit() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        STOP.store(false, Ordering::Release);
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let id = fixture(&source, "feed-fixture");
        let source = db::open_db(&source).unwrap();
        let snapshot = Snapshot {
            version: 1,
            session_id: "feed-session".into(),
            revision_id: "feed-revision".into(),
            parent_revision: None,
            content: content(&source, id).unwrap(),
        };
        let key = LibraryKey::generate();
        let chunk = hession_sync::chunks(&snapshot).unwrap().remove(0);
        let record = hession_sync::encrypt(
            &key,
            "vault",
            "feed-record",
            "peer",
            &serde_json::to_vec(&chunk).unwrap(),
        )
        .unwrap();
        let row = |sequence: i64| json!({"sequence":sequence,"id":record.id,"deviceId":record.device_id,"keyEpoch":record.key_epoch,"payload":record.payload});
        let malformed = json!({"sequence":2,"id":"malformed","deviceId":"peer","keyEpoch":1,"payload":"bm90LWFuLWVudmVsb3Bl"});
        let pages = [
            json!({"records":[row(2)],"cursor":2,"hasMore":false}),
            json!({"records":[row(1)],"cursor":2,"hasMore":false}),
            json!({"records":[],"cursor":1,"hasMore":false}),
            json!({"records":[],"cursor":-1,"hasMore":false}),
            json!({"records":[],"cursor":0,"hasMore":true}),
            json!({"records":[row(1),row(1)],"cursor":1,"hasMore":false}),
            json!({"records":[row(1),malformed],"cursor":2,"hasMore":false}),
        ];
        for (index, page) in pages.into_iter().enumerate() {
            let server = Server::with_router(axum::Router::new().route(
                "/v1/records",
                axum::routing::get(move || {
                    let page = page.clone();
                    async move { axum::Json(page) }
                }),
            ));
            let path = dir.path().join(format!("receiver-{index}/library.sqlite"));
            let (mut config, secrets) = config_for(
                &path,
                &server.url,
                Enrollment {
                    vault_id: "vault".into(),
                    device_id: "receiver".into(),
                    device_token: "test-token-not-used-by-fixture-server".into(),
                },
                &key,
            );
            config.scope.clear();
            save(&path, &config).unwrap();
            let conn = db::open_db(&path).unwrap();
            init_schema(&conn).unwrap();
            assert!(
                pull(&path, &config, &key, &secrets, &client().unwrap()).is_err(),
                "accepted malicious page {index}"
            );
            for table in [
                "session",
                "replication_chunk",
                "replication_revision",
                "replication_checkpoint",
            ] {
                assert_eq!(
                    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0,
                    "partially committed {table} for page {index}"
                );
            }
        }
    }

    #[test]
    fn cancellation_and_scope_change_during_upload_stop_after_inflight_ack() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let id = fixture(&path, "cancel-upload");
        let conn = db::open_db(&path).unwrap();
        append(
            &conn,
            id,
            2,
            "user",
            &"Large cancellation fixture. ".repeat(30_000),
        );
        let posts = Arc::new(AtomicUsize::new(0));
        let handler_posts = posts.clone();
        let handler_path = path.clone();
        let router = axum::Router::new().route(
            "/v1/records",
            axum::routing::get(|| async {
                axum::Json(json!({"records":[],"cursor":0,"hasMore":false}))
            })
            .post(move |axum::Json(body): axum::Json<Value>| {
                let posts = handler_posts.clone();
                let path = handler_path.clone();
                async move {
                    if posts.fetch_add(1, Ordering::SeqCst) == 0 {
                        set_enabled(&path, false, Some(Vec::new())).unwrap();
                    }
                    axum::Json(json!({"accepted":body["records"].as_array().unwrap().len()}))
                }
            }),
        );
        let server = Server::with_router(router);
        let key = LibraryKey::generate();
        let (config, secrets) = config_for(
            &path,
            &server.url,
            Enrollment {
                vault_id: "vault".into(),
                device_id: "sender".into(),
                device_token: "unused-fake-relay-token".into(),
            },
            &key,
        );
        let store = MemoryStore::default();
        put_secrets_in(&store, &config.credential_ref, &secrets).unwrap();
        assert!(run_with_store(&path, &store)
            .unwrap_err()
            .contains("stopped"));
        assert_eq!(
            posts.load(Ordering::SeqCst),
            1,
            "no second upload may start after disable"
        );
        assert!(!RUNNING.load(Ordering::Acquire));
        assert!(!crate::sync::is_managing_data());
        assert!(!progress().lock().unwrap().running);
        assert!(
            conn.query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap()
                > 0
        );
        assert!(load(&path).unwrap().scope.is_empty());
        assert!(!load(&path).unwrap().enabled);
        set_enabled(&path, true, None).unwrap();
        run_with_store(&path, &store).unwrap();
        assert_eq!(
            posts.load(Ordering::SeqCst),
            1,
            "download-only restart must cancel the excluded queue"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        set_enabled(&path, true, Some(vec!["/work/project".into()])).unwrap();
        run_with_store(&path, &store).unwrap();
        assert!(posts.load(Ordering::SeqCst) > 1);
        assert!(!RUNNING.load(Ordering::Acquire));
        assert!(!crate::sync::is_managing_data());
        assert!(
            conn.query_row("SELECT COUNT(*) FROM entry WHERE session_id=?", [id], |r| r
                .get::<_, i64>(0))
                .unwrap()
                >= 3
        );
    }

    #[test]
    fn portable_export_uses_allowlisted_text_and_redacts_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let id = fixture(&path, "native");
        let conn = db::open_db(&path).unwrap();
        append(
            &conn,
            id,
            2,
            "user",
            "api_key=supersecret123456 and sk-abcdefghijklmnopqrstuvwxyz012345",
        );
        append(
            &conn,
            id,
            3,
            "system",
            "system-instructions-must-not-transfer",
        );
        conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(?,4,'message','assistant','private-reasoning-must-not-transfer')",[id]).unwrap();
        let entry = conn.last_insert_rowid();
        conn.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'thinking','private-reasoning-must-not-transfer')",[entry]).unwrap();
        conn.execute("INSERT INTO tool_invocation(session_id,entry_id,tool_name,command_text,result_text,status) VALUES(?,?,'Read','cat ~/.aws/credentials','tool-secret-must-not-transfer','success')",params![id,entry]).unwrap();
        let encoded = serde_json::to_string(&content(&conn, id).unwrap()).unwrap();
        for secret in [
            "hidden-raw-secret",
            "supersecret123456",
            "sk-abcdefghijklmnopqrstuvwxyz012345",
            "system-instructions-must-not-transfer",
            "private-reasoning-must-not-transfer",
            "tool-secret-must-not-transfer",
            "~/.aws/credentials",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}");
        }
        assert!(encoded.contains("Please fix the parser"));
        assert!(encoded.contains("[REDACTED]"));
        assert!(encoded.contains("Read"));
    }

    #[test]
    fn replay_rebinds_deleted_revisions_without_overwriting_reused_rows() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let original = fixture(&source, "native-original");
        let source = db::open_db(&source).unwrap();
        let snapshot = Snapshot {
            version: 1,
            session_id: "portable-original".into(),
            revision_id: "revision-original".into(),
            parent_revision: None,
            content: content(&source, original).unwrap(),
        };
        let path = dir.path().join("receiver.sqlite");
        let conn = db::open_db(&path).unwrap();
        init_schema(&conn).unwrap();
        apply_snapshot(&conn, &snapshot, "vault").unwrap();
        let deleted: i64 = conn
            .query_row("SELECT id FROM session", [], |r| r.get(0))
            .unwrap();
        conn.execute("DELETE FROM session WHERE id=?", [deleted])
            .unwrap();
        let agent = db::get_or_create_agent(&conn, "codex", "Codex", None).unwrap();
        conn.execute("INSERT INTO session(id,agent_id,source_uri,title) VALUES(?,?,'codex:///unrelated','Unrelated native session')",params![deleted,agent]).unwrap();
        apply_snapshot(&conn, &snapshot, "vault").unwrap();
        assert_eq!(
            conn.query_row("SELECT title FROM session WHERE id=?", [deleted], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            "Unrelated native session"
        );
        let restored: i64 = conn
            .query_row(
                "SELECT local_session_id FROM replication_revision WHERE revision_id=?",
                [&snapshot.revision_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(restored, deleted);
        apply_snapshot(&conn, &snapshot, "vault").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let mut conflict = snapshot.clone();
        conflict.content.title = "Conflicting payload".into();
        assert!(apply_snapshot(&conn, &conflict, "vault")
            .unwrap_err()
            .contains("Conflicting data"));
        conn.execute("DELETE FROM session", []).unwrap();
        apply_snapshot(&conn, &snapshot, "vault").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn revision_ancestry_rejects_cross_session_parents_and_late_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.sqlite");
        let id = fixture(&source, "ancestry-source");
        let source = db::open_db(&source).unwrap();
        let mut root = Snapshot {
            version: 1,
            session_id: "session-a".into(),
            revision_id: "root".into(),
            parent_revision: None,
            content: content(&source, id).unwrap(),
        };
        let receiver = db::open_db(&dir.path().join("receiver.sqlite")).unwrap();
        init_schema(&receiver).unwrap();
        apply_snapshot(&receiver, &root, "vault").unwrap();
        let mut foreign = root.clone();
        foreign.session_id = "session-b".into();
        foreign.revision_id = "foreign".into();
        foreign.parent_revision = Some("root".into());
        assert!(apply_snapshot(&receiver, &foreign, "vault")
            .unwrap_err()
            .contains("another session"));
        // Missing ancestors are valid for a self-contained snapshot (for
        // example after canceled chunks), but filling one must not form a loop.
        root.revision_id = "child-before-parent".into();
        root.parent_revision = Some("late-parent".into());
        root.content.title = "Child received first".into();
        apply_snapshot(&receiver, &root, "vault").unwrap();
        let mut late = root.clone();
        late.revision_id = "late-parent".into();
        late.parent_revision = Some("child-before-parent".into());
        assert!(apply_snapshot(&receiver, &late, "vault")
            .unwrap_err()
            .contains("Cyclic"));
        assert_eq!(
            receiver
                .query_row("SELECT COUNT(*) FROM replication_revision", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn replay_same_portable_row_must_materialize_each_missing_revision_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.sqlite");
        let id = fixture(&path, "ordered-replay");
        let source = db::open_db(&path).unwrap();
        let first = Snapshot {
            version: 1,
            session_id: "portable".into(),
            revision_id: "first".into(),
            parent_revision: None,
            content: content(&source, id).unwrap(),
        };
        let mut second = first.clone();
        second.revision_id = "second".into();
        second.parent_revision = Some("first".into());
        second.content.title = "Second revision must be materialized".into();
        let receiver = db::open_db(&dir.path().join("receiver.sqlite")).unwrap();
        init_schema(&receiver).unwrap();
        apply_snapshot(&receiver, &first, "vault").unwrap();
        apply_snapshot(&receiver, &second, "vault").unwrap();
        receiver.execute("DELETE FROM session", []).unwrap();
        apply_snapshot(&receiver, &first, "vault").unwrap();
        // The new first revision reuses the same numeric row ID and portable
        // identity. That alone is not evidence the second revision was restored.
        apply_snapshot(&receiver, &second, "vault").unwrap();
        assert_eq!(
            receiver
                .query_row("SELECT title FROM session", [], |r| r.get::<_, String>(0))
                .unwrap(),
            second.content.title
        );
        assert_eq!(
            receiver
                .query_row("SELECT head_revision FROM replication_identity", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            second.revision_id
        );
        apply_snapshot(&receiver, &first, "vault").unwrap();
        assert_eq!(
            receiver
                .query_row("SELECT title FROM session", [], |r| r.get::<_, String>(0))
                .unwrap(),
            second.content.title,
            "an older known ancestor must not regress the live head"
        );
    }

    #[test]
    fn shared_maintenance_gate_blocks_device_sync_before_credentials_or_network() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        fixture(&path, "native");
        let key = LibraryKey::generate();
        let (config, _) = config_for(
            &path,
            "https://unreachable.invalid",
            Enrollment {
                vault_id: "vault".into(),
                device_id: "device".into(),
                device_token: "unused".into(),
            },
            &key,
        );
        let guard = crate::sync::acquire_data_management_for(&path).unwrap();
        assert!(run(&path)
            .unwrap_err()
            .contains("Finish refreshing histories"));
        assert!(!RUNNING.load(Ordering::Acquire));
        assert!(crate::sync::is_managing_data());
        drop(guard);
        assert!(!crate::sync::is_managing_data());
        // The same shared gate prevents history imports while replication is
        // holding it; no worker thread should be created.
        let _guard = crate::sync::acquire_data_management_for(&path).unwrap();
        assert!(!crate::sync::start_sync(None, "incremental"));
        let mut config = config;
        config.enabled = false;
        save(&path, &config).unwrap();
    }

    #[test]
    fn real_relay_converges_preserves_offline_branches_and_rejects_wrong_key() {
        let _test = TEST_NETWORK_LOCK.lock().unwrap();
        // One test owns global progress/cancel flags; no OS credential store is
        // touched. Network, crypto, SQL, snapshots, outbox and cursors are real.
        STOP.store(false, Ordering::Release);
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(&dir.path().join("relay.sqlite"));
        let http = client().unwrap();
        let first: Enrollment = response(
            http.post(format!("{}/v1/bootstrap", server.url))
                .bearer_auth("a-test-administrator-token-at-least-32-characters")
                .json(&json!({"deviceId":"laptop","deviceName":"Laptop"}))
                .send(),
        )
        .unwrap();
        let path_a = dir.path().join("a/library.sqlite");
        let id_a = fixture(&path_a, "shared-native-id");
        let key = LibraryKey::generate();
        let (config_a, secrets_a) = config_for(&path_a, &server.url, first, &key);
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        let initial: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(initial["records"].as_array().unwrap().len(), 1);
        let serialized = initial.to_string();
        assert!(!serialized.contains("Please fix the parser"));
        assert!(!serialized.contains("Private task"));
        let invitation: Value = response(
            http.post(format!("{}/v1/invitations", server.url))
                .bearer_auth(&secrets_a.device_token)
                .json(&json!({}))
                .send(),
        )
        .unwrap();
        let second:Enrollment=response(http.post(format!("{}/v1/devices/join",server.url)).json(&json!({"deviceId":"desktop","deviceName":"Desktop","invitation":invitation["invitation"]})).send()).unwrap();
        let path_b = dir.path().join("b/library.sqlite");
        let (config_b, secrets_b) = config_for(&path_b, &server.url, second, &key);
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        let conn_b = db::open_db(&path_b).unwrap();
        let id_b: i64 = conn_b
            .query_row("SELECT id FROM session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            conn_b
                .query_row(
                    "SELECT branch FROM session_context WHERE session_id=?",
                    [id_b],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "feature/private-sync"
        );
        assert_eq!(
            conn_b
                .query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn_b
                .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        let unchanged: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(
            unchanged["records"].as_array().unwrap().len(),
            1,
            "downloaded content must not echo-upload"
        );
        let conn_a = db::open_db(&path_a).unwrap();
        append(&conn_a, id_a, 5, "user", "Laptop continued offline.");
        append(&conn_b, id_b, 5, "user", "Desktop continued offline.");
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        assert_eq!(
            conn_b
                .query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2,
            "offline histories must remain branches"
        );
        for marker in ["Laptop continued offline.", "Desktop continued offline."] {
            assert!(
                conn_b
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM entry WHERE search_text=?)",
                        [marker],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap(),
                "lost {marker}"
            );
            assert!(
                conn_a
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM entry WHERE search_text=?)",
                        [marker],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap(),
                "lost {marker}"
            );
        }
        // Both clients received each branch; retries produce no new records.
        let before: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        let after: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(before["cursor"], after["cursor"]);
        let bad_key = LibraryKey::generate();
        let mut bad_config = config_b.clone();
        bad_config.key_fingerprint = bad_key.fingerprint();
        let bad_secrets = Secrets {
            device_token: secrets_b.device_token.clone(),
            recovery_key: bad_key.recovery_key(),
        };
        let bad_path = dir.path().join("bad/library.sqlite");
        fixture(&bad_path, "should-never-upload");
        save(&bad_path, &bad_config).unwrap();
        assert!(transfer(&bad_path, &bad_config, &bad_secrets)
            .unwrap_err()
            .contains("authentication failed"));
        let bad_conn = db::open_db(&bad_path).unwrap();
        assert_eq!(
            bad_conn
                .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let final_feed: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(
            after["cursor"], final_feed["cursor"],
            "wrong key must not publish"
        );

        // Simulate a crash after the relay commits the first chunk but before
        // the client removes its outbox row. The receiver checkpoints that
        // partial transfer without showing an incomplete transcript.
        let large = "Large transcript checkpoint. ".repeat(45_000);
        append(&conn_a, id_a, 6, "assistant", &large);
        capture(&path_a, &config_a, &key).unwrap();
        let pending: i64 = conn_a
            .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r.get(0))
            .unwrap();
        assert!(
            pending > 1,
            "large snapshot must be split into bounded records"
        );
        let first: String = conn_a
            .query_row(
                "SELECT record_json FROM replication_outbox ORDER BY rowid LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let first: RelayRecord = serde_json::from_str(&first).unwrap();
        let acknowledged: Value = response(
            http.post(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .json(&json!({"records":[first]}))
                .send(),
        )
        .unwrap();
        assert_eq!(acknowledged["accepted"], 1);
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        assert!(conn_b
            .query_row("SELECT COUNT(*)>0 FROM replication_chunk", [], |r| r
                .get::<_, bool>(0))
            .unwrap());
        assert!(!conn_b
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entry WHERE search_text=?)",
                [&large],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        transfer(&path_a, &config_a, &secrets_a).unwrap();
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        assert_eq!(
            conn_a
                .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn_b
                .query_row("SELECT COUNT(*) FROM replication_chunk", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(conn_b
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entry WHERE search_text=?)",
                [&large],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        let large_feed: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&secrets_a.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(
            large_feed["cursor"].as_i64().unwrap(),
            final_feed["cursor"].as_i64().unwrap() + pending,
            "acknowledged chunk retry must be idempotent"
        );

        // Re-pairing changes credentials without stranding snapshots queued
        // under the previous writer ID. No old device token is needed to send.
        append(
            &conn_a,
            id_a,
            7,
            "user",
            "Queued work survived device re-pairing.",
        );
        capture(&path_a, &config_a, &key).unwrap();
        let invitation: Value = response(
            http.post(format!("{}/v1/invitations", server.url))
                .bearer_auth(&secrets_a.device_token)
                .json(&json!({}))
                .send(),
        )
        .unwrap();
        let repaired:Enrollment=response(http.post(format!("{}/v1/devices/join",server.url)).json(&json!({"deviceId":"laptop-repaired","deviceName":"Repaired laptop","invitation":invitation["invitation"]})).send()).unwrap();
        let (repaired_config, repaired_secrets) = config_for(&path_a, &server.url, repaired, &key);
        transfer(&path_a, &repaired_config, &repaired_secrets).unwrap();
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        assert_eq!(
            conn_a
                .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(conn_b.query_row("SELECT EXISTS(SELECT 1 FROM entry WHERE search_text='Queued work survived device re-pairing.')",[],|r|r.get::<_,bool>(0)).unwrap());

        // Narrowing scope applies to ciphertext already queued offline. Leave
        // one acknowledged-but-not-dequeued chunk on the relay, exclude the
        // project, and prove an empty scope cannot send the remaining chunks.
        append(
            &conn_a,
            id_a,
            8,
            "user",
            "Withheld while workspace is excluded.",
        );
        capture(&path_a, &repaired_config, &key).unwrap();
        let queued: String = conn_a
            .query_row(
                "SELECT record_json FROM replication_outbox ORDER BY rowid LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let queued: RelayRecord = serde_json::from_str(&queued).unwrap();
        let _: Value = response(
            http.post(format!("{}/v1/records", server.url))
                .bearer_auth(&repaired_secrets.device_token)
                .json(&json!({"records":[queued]}))
                .send(),
        )
        .unwrap();
        let before_exclusion: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&repaired_secrets.device_token)
                .send(),
        )
        .unwrap();
        let mut excluded = repaired_config.clone();
        excluded.scope.clear();
        save(&path_a, &excluded).unwrap();
        transfer(&path_a, &excluded, &repaired_secrets).unwrap();
        let after_exclusion: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&repaired_secrets.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(
            before_exclusion["cursor"], after_exclusion["cursor"],
            "download-only must not flush old queued histories"
        );
        assert_eq!(
            conn_a
                .query_row("SELECT COUNT(*) FROM replication_outbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        assert!(!conn_b.query_row("SELECT EXISTS(SELECT 1 FROM entry WHERE search_text='Withheld while workspace is excluded.')",[],|r|r.get::<_,bool>(0)).unwrap());
        save(&path_a, &repaired_config).unwrap();
        transfer(&path_a, &repaired_config, &repaired_secrets).unwrap();
        transfer(&path_b, &config_b, &secrets_b).unwrap();
        assert!(conn_b.query_row("SELECT EXISTS(SELECT 1 FROM entry WHERE search_text='Withheld while workspace is excluded.')",[],|r|r.get::<_,bool>(0)).unwrap(),"re-opting in must republish and complete the partial snapshot");

        // Download-only does not authorize outgoing annotation edits, but must
        // preserve them as a local branch instead of replacing them silently.
        let annotated:i64=conn_b.query_row("SELECT session_id FROM entry WHERE search_text='Withheld while workspace is excluded.' LIMIT 1",[],|r|r.get(0)).unwrap();
        conn_b
            .execute(
                "UPDATE session_annotation SET starred=1 WHERE session_id=?",
                [annotated],
            )
            .unwrap();
        conn_b
            .execute(
                "INSERT INTO session_tag(session_id,label) VALUES(?,'local-only-tag')",
                [annotated],
            )
            .unwrap();
        let mut download_only = config_b.clone();
        download_only.scope.clear();
        save(&path_b, &download_only).unwrap();
        append(
            &conn_a,
            id_a,
            9,
            "user",
            "Incoming update must preserve local annotations.",
        );
        transfer(&path_a, &repaired_config, &repaired_secrets).unwrap();
        let before_download: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&repaired_secrets.device_token)
                .send(),
        )
        .unwrap();
        transfer(&path_b, &download_only, &secrets_b).unwrap();
        assert_eq!(
            conn_b
                .query_row(
                    "SELECT starred FROM session_annotation WHERE session_id=?",
                    [annotated],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert!(conn_b.query_row("SELECT EXISTS(SELECT 1 FROM session_tag WHERE session_id=? AND label='local-only-tag')",[annotated],|r|r.get::<_,bool>(0)).unwrap());
        assert!(conn_b.query_row("SELECT EXISTS(SELECT 1 FROM entry WHERE search_text='Incoming update must preserve local annotations.' AND session_id<>?)",[annotated],|r|r.get::<_,bool>(0)).unwrap());
        let after_download: Value = response(
            http.get(format!("{}/v1/records", server.url))
                .bearer_auth(&repaired_secrets.device_token)
                .send(),
        )
        .unwrap();
        assert_eq!(
            before_download["cursor"], after_download["cursor"],
            "local-only annotation branch must never be uploaded"
        );

        // A local reset may leave remembered revision IDs bound to row IDs
        // SQLite reuses. Replay must restore histories, preserving an unrelated
        // native row even when the old revision's numeric row ID now names it.
        conn_b.execute("DELETE FROM session", []).unwrap();
        let unrelated_agent = db::get_or_create_agent(&conn_b, "codex", "Codex", None).unwrap();
        conn_b.execute("INSERT INTO session(id,agent_id,source_uri,title) VALUES(1,?,'codex:///unrelated-after-reset','Unrelated after reset')",[unrelated_agent]).unwrap();
        let mut replay = download_only.clone();
        replay.enabled = false;
        save(&path_b, &replay).unwrap();
        reset_checkpoint(&path_b).unwrap();
        replay.enabled = true;
        save(&path_b, &replay).unwrap();
        transfer(&path_b, &replay, &secrets_b).unwrap();
        assert_eq!(
            conn_b
                .query_row("SELECT title FROM session WHERE id=1", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "Unrelated after reset"
        );
        for marker in [
            "Incoming update must preserve local annotations.",
            "Desktop continued offline.",
        ] {
            assert!(
                conn_b
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM entry WHERE search_text=?)",
                        [marker],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap(),
                "replay lost {marker}"
            );
        }

        // Authentication failure rolls back the page cursor and cannot create
        // a transcript, even for an authenticated writer's malformed record.
        let cursor_before: i64 = conn_b
            .query_row(
                "SELECT cursor FROM replication_checkpoint WHERE vault_id=?",
                [&config_b.vault_id],
                |r| r.get(0),
            )
            .unwrap();
        let malicious:Value=response(http.post(format!("{}/v1/records",server.url)).bearer_auth(&secrets_a.device_token).json(&json!({"records":[{"id":"malformed-record","deviceId":config_a.device_id,"keyEpoch":1,"payload":"bm90LWFuLWVudmVsb3Bl"}]})).send()).unwrap();
        assert_eq!(malicious["accepted"], 1);
        assert!(transfer(&path_b, &config_b, &secrets_b).is_err());
        assert_eq!(
            conn_b
                .query_row(
                    "SELECT cursor FROM replication_checkpoint WHERE vault_id=?",
                    [&config_b.vault_id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            cursor_before
        );
    }
}
