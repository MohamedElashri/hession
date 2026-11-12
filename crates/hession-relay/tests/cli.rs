//! Exercise the packaged server executable rather than an in-process Router.
//! Every credential, database, listener, and subprocess belongs to this test.
use hession_sync::{LibraryKey, RelayRecord};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_hession-relay");
const ADMIN: &str = "test-only-relay-administrator-token-not-a-user-secret";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

fn command() -> Command {
    let mut command = Command::new(BINARY);
    // Tests must not inherit a real administrator token, data directory,
    // proxy, or credentials from the developer's environment.
    command
        .env_clear()
        .env("TOKIO_WORKER_THREADS", "2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

struct Process {
    child: Child,
    output_reader: Option<JoinHandle<()>>,
}

impl Process {
    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap();
        if let Some(reader) = self.output_reader.take() {
            reader.join().unwrap();
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Never leave a relay running when an assertion fails.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.output_reader.take() {
            let _ = reader.join();
        }
    }
}

fn completed(mut command: Command) -> Output {
    let mut process = Process {
        child: command.spawn().expect("start disposable relay CLI"),
        output_reader: None,
    };
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if process.child.try_wait().unwrap().is_some() {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            process
                .child
                .stdout
                .take()
                .unwrap()
                .take(16 * 1024 + 1)
                .read_to_end(&mut stdout)
                .unwrap();
            process
                .child
                .stderr
                .take()
                .unwrap()
                .take(16 * 1024 + 1)
                .read_to_end(&mut stderr)
                .unwrap();
            assert!(
                stdout.len() <= 16 * 1024 && stderr.len() <= 16 * 1024,
                "CLI output exceeded test bound"
            );
            return Output {
                status: process.child.wait().unwrap(),
                stdout,
                stderr,
            };
        }
        assert!(
            Instant::now() < deadline,
            "relay CLI did not exit within five seconds"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Server {
    process: Process,
    address: SocketAddr,
}

impl Server {
    fn start(data: &Path, token_file: &Path) -> Self {
        let mut command = command();
        command
            .env("HESSION_RELAY_BIND", "127.0.0.1:0")
            .env("HESSION_RELAY_DATA_DIR", data)
            .env("HESSION_RELAY_ADMIN_TOKEN_FILE", token_file)
            .arg("serve");
        let mut child = command.spawn().expect("start disposable relay server");
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout.take(1024))
                .read_line(&mut line)
                .map(|_| line);
            let _ = send.send(result);
        });
        let mut process = Process {
            child,
            output_reader: Some(reader),
        };
        let line = receive
            .recv_timeout(PROCESS_TIMEOUT)
            .expect("relay did not report its bound address")
            .expect("read relay startup");
        let address = line
            .strip_prefix("Hession relay listening on ")
            .and_then(|value| value.split(';').next());
        let Some(address) = address else {
            process.stop();
            let mut diagnostic = String::new();
            process
                .child
                .stderr
                .take()
                .unwrap()
                .take(16 * 1024)
                .read_to_string(&mut diagnostic)
                .unwrap();
            panic!("Relay exited before reporting readiness: {diagnostic}");
        };
        let address: SocketAddr = address
            .parse()
            .expect("startup must report the actual bound address");
        assert!(address.ip().is_loopback());
        assert_ne!(
            address.port(),
            0,
            "port zero must resolve before reporting readiness"
        );
        Self { process, address }
    }

    fn request(&self, method: &str, path: &str, token: Option<&str>, value: Value) -> (u16, Value) {
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let body = serde_json::to_vec(&value).unwrap();
        let authorization = token
            .map(|value| format!("Authorization: Bearer {value}\r\n"))
            .unwrap_or_default();
        let head=format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{authorization}\r\n",self.address,body.len());
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        let mut bytes = Vec::new();
        stream
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(
            bytes.len() <= 2 * 1024 * 1024,
            "unexpectedly large packaged relay response"
        );
        let boundary = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .expect("HTTP response headers");
        let headers = std::str::from_utf8(&bytes[..boundary]).unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(headers
            .to_ascii_lowercase()
            .contains("cache-control: no-store"));
        let value =
            serde_json::from_slice(&bytes[boundary + 4..]).expect("packaged relay JSON response");
        (status, value)
    }
}

#[test]
fn help_version_and_argument_errors_do_not_require_credentials_or_start_storage() {
    let mut help = command();
    help.arg("--help");
    let result = completed(help);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("Hession private relay"));
    assert!(output.contains("HESSION_RELAY_ADMIN_TOKEN_FILE"));
    let mut version = command();
    version.arg("--version");
    let result = completed(version);
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("hession-relay {}\n", env!("CARGO_PKG_VERSION"))
    );
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("never-created");
    let mut invalid = command();
    invalid
        .arg("unknown-action")
        .env("HESSION_RELAY_DATA_DIR", &data);
    let result = completed(invalid);
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("Unknown arguments"));
    assert!(!data.exists());
}

#[test]
fn invalid_configuration_fails_before_binding_and_never_echoes_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let token_file = directory.path().join("admin-token");
    std::fs::write(&token_file, format!("{ADMIN}\n")).unwrap();
    let data = directory.path().join("never-created");
    let mut absent = command();
    absent.env("HESSION_RELAY_DATA_DIR", &data);
    let result = completed(absent);
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("Set HESSION_RELAY_ADMIN_TOKEN"));
    assert!(!data.exists());
    for invalid_token in [
        "too-short-admin-secret",
        "secret with whitespace is not an allowed relay token",
    ] {
        let mut invalid = command();
        invalid
            .env("HESSION_RELAY_ADMIN_TOKEN", invalid_token)
            .env("HESSION_RELAY_DATA_DIR", &data)
            .env(
                "HESSION_RELAY_BIND",
                "invalid address that must not be reached",
            );
        let result = completed(invalid);
        assert!(!result.status.success());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains("32–256 printable ASCII"));
        assert!(!error.contains(invalid_token));
        assert!(!data.exists());
    }
    let mut conflict = command();
    conflict
        .env("HESSION_RELAY_ADMIN_TOKEN", ADMIN)
        .env("HESSION_RELAY_ADMIN_TOKEN_FILE", &token_file)
        .env("HESSION_RELAY_DATA_DIR", &data);
    let result = completed(conflict);
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("not both"));
    assert!(!error.contains(ADMIN));
    assert!(!data.exists());
    let mut missing = command();
    missing
        .env(
            "HESSION_RELAY_ADMIN_TOKEN_FILE",
            directory.path().join("missing-secret"),
        )
        .env("HESSION_RELAY_DATA_DIR", &data);
    let result = completed(missing);
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("Cannot read administrator token file"));
    assert!(!data.exists());
}

#[test]
fn packaged_server_handles_encrypted_history_and_online_cli_backup_restores_access() {
    let directory = tempfile::tempdir().unwrap();
    let token_file = directory.path().join("admin-token");
    std::fs::write(&token_file, format!("{ADMIN}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let data = directory.path().join("live");
    let mut server = Server::start(&data, &token_file);
    assert_eq!(
        server.request("GET", "/v1/status", None, Value::Null),
        (200, json!({"protocolVersion":1,"initialized":false}))
    );
    assert_eq!(
        server.request("GET", "/v1/records", None, Value::Null).0,
        401
    );
    let (status, enrollment) = server.request(
        "POST",
        "/v1/bootstrap",
        Some(ADMIN),
        json!({"deviceId":"cli-laptop","deviceName":"CLI test laptop"}),
    );
    assert_eq!(status, 200);
    let token = enrollment["deviceToken"].as_str().unwrap();
    let vault = enrollment["vaultId"].as_str().unwrap();
    let key = LibraryKey::generate();
    let plaintext = b"CLI-only encrypted session, never a real user's history.";
    let record =
        hession_sync::encrypt(&key, vault, "cli-encrypted-record", "cli-laptop", plaintext)
            .unwrap();
    assert_eq!(
        server.request(
            "POST",
            "/v1/records",
            Some(token),
            json!({"records":[record]})
        ),
        (200, json!({"accepted":1}))
    );
    let (status, feed) = server.request(
        "GET",
        "/v1/records?after=0&limit=1",
        Some(token),
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(feed["cursor"], 1);
    let mut encoded = feed["records"][0].clone();
    encoded.as_object_mut().unwrap().remove("sequence");
    let received: RelayRecord = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        hession_sync::decrypt(&key, vault, &received).unwrap(),
        plaintext
    );
    let backup_path = directory.path().join("online-backup.sqlite");
    let mut backup = command();
    backup
        .arg("backup")
        .arg(&backup_path)
        .env("HESSION_RELAY_DATA_DIR", &data);
    let result = completed(backup);
    assert!(result.status.success(), "online backup CLI failed");
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("Consistent relay backup created"));
    let bytes = std::fs::read(&backup_path).unwrap();
    for secret in [plaintext.as_slice(), ADMIN.as_bytes(), token.as_bytes()] {
        assert!(
            !bytes.windows(secret.len()).any(|window| window == secret),
            "relay backup retained a plaintext secret"
        );
    }
    server.process.stop();
    let restored_data = directory.path().join("restored");
    std::fs::create_dir(&restored_data).unwrap();
    std::fs::copy(&backup_path, restored_data.join("relay.sqlite")).unwrap();
    let restored = Server::start(&restored_data, &token_file);
    assert_eq!(
        restored.request("GET", "/v1/status", None, Value::Null),
        (200, json!({"protocolVersion":1,"initialized":true}))
    );
    assert_eq!(
        restored
            .request(
                "POST",
                "/v1/bootstrap",
                Some(ADMIN),
                json!({"deviceId":"other","deviceName":"Must not reset"})
            )
            .0,
        409
    );
    let (status, restored_feed) = restored.request(
        "GET",
        "/v1/records?after=0&limit=1",
        Some(token),
        Value::Null,
    );
    assert_eq!(status, 200, "restored device credential must remain valid");
    assert_eq!(restored_feed, feed);
    let next = hession_sync::encrypt(
        &key,
        vault,
        "cli-next-record",
        "cli-laptop",
        b"after restore",
    )
    .unwrap();
    assert_eq!(
        restored
            .request(
                "POST",
                "/v1/records",
                Some(token),
                json!({"records":[next]})
            )
            .0,
        200
    );
    let (status, next_feed) =
        restored.request("GET", "/v1/records?after=1", Some(token), Value::Null);
    assert_eq!(status, 200);
    assert_eq!(next_feed["cursor"], 2);
    assert_eq!(next_feed["records"][0]["id"], "cli-next-record");
}
