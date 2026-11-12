//! Opt-in black-box checks for the built relay image, with disposable volumes.
//! Run after building hession-relay:test; no production credentials are used.
use hession_sync::{LibraryKey, RelayRecord};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const ADMIN: &str = "container-test-only-administrator-secret-never-user-data";

struct Runtime {
    executable: OsString,
    image: String,
    containers: Vec<String>,
    volumes: Vec<String>,
}

impl Runtime {
    fn command(&self, args: &[&str]) -> Output {
        let mut child = Command::new(&self.executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start container CLI");
        let deadline = Instant::now() + Duration::from_secs(45);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("container command timed out: {args:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }

    fn checked(&self, args: &[&str]) -> String {
        let result = self.command(args);
        assert!(
            result.status.success(),
            "container {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().into()
    }

    fn volume(&mut self) -> String {
        let name = format!("hession-qa-{}", uuid::Uuid::new_v4());
        self.volumes.push(name.clone());
        self.checked(&[
            "volume",
            "create",
            "--label",
            "io.hession.test=relay-container",
            &name,
        ]);
        name
    }

    fn ready(&self, name: &str, stage: &str) -> SocketAddr {
        // A runtime may allocate a different ephemeral host port on restart.
        let address = self
            .checked(&["port", name, "8744/tcp"])
            .parse()
            .expect("loopback published address");
        println!("Checking container {stage} at {address}");
        if let Err(error) = wait_ready(address) {
            let state = self.command(&[
                "inspect",
                "--format",
                "{{.State.Status}} exit={{.State.ExitCode}} error={{.State.Error}}",
                name,
            ]);
            let logs = self.command(&["logs", "--tail", "20", name]);
            panic!(
                "Container {stage} failed: {error}\nState: {}{}\nLogs: {}{}",
                String::from_utf8_lossy(&state.stdout),
                String::from_utf8_lossy(&state.stderr),
                String::from_utf8_lossy(&logs.stdout),
                String::from_utf8_lossy(&logs.stderr)
            );
        }
        address
    }

    fn start(&mut self, volume: &str, secret: &Path) -> (String, SocketAddr) {
        let name = format!("hession-qa-{}", uuid::Uuid::new_v4());
        self.containers.push(name.clone());
        let mount = format!(
            "type=bind,source={},target=/run/secrets/admin-token,readonly",
            secret.display()
        );
        let data = format!("{volume}:/data");
        self.checked(&[
            "run",
            "--detach",
            "--name",
            &name,
            "--label",
            "io.hession.test=relay-container",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges:true",
            "--memory",
            "512m",
            "--pids-limit",
            "128",
            "--publish",
            "127.0.0.1::8744",
            "--mount",
            &mount,
            "--volume",
            &data,
            "--env",
            "HESSION_RELAY_ADMIN_TOKEN_FILE=/run/secrets/admin-token",
            "--env",
            "HESSION_RELAY_QUOTA_BYTES=8388608",
            &self.image,
        ]);
        let address = self.ready(&name, "startup");
        (name, address)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        for name in &self.containers {
            let _ = self.command(&["rm", "--force", "--volumes", name]);
        }
        for name in &self.volumes {
            let _ = self.command(&["volume", "rm", "--force", name]);
        }
    }
}

fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    value: Value,
) -> (u16, Value) {
    try_request(address, method, path, token, value).expect("container HTTP request")
}

fn try_request(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    value: Value,
) -> Result<(u16, Value), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let body = serde_json::to_vec(&value)?;
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth}\r\n",body.len())?;
    stream.write_all(&body)?;
    let mut bytes = Vec::new();
    stream.take(12 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    assert!(
        bytes.len() <= 12 * 1024 * 1024,
        "container response exceeded size bound"
    );
    let boundary = bytes
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("missing HTTP headers")?;
    let headers = std::str::from_utf8(&bytes[..boundary])?;
    assert!(headers
        .to_ascii_lowercase()
        .contains("cache-control: no-store"));
    let code = headers
        .split_whitespace()
        .nth(1)
        .ok_or("missing HTTP status")?
        .parse()?;
    Ok((code, serde_json::from_slice(&bytes[boundary + 4..])?))
}

fn wait_ready(address: SocketAddr) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let error = match try_request(address, "GET", "/v1/status", None, Value::Null) {
            Ok((200, _)) => return Ok(()),
            Ok((status, _)) => format!("HTTP {status}"),
            Err(error) => error.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(format!("{address} did not become ready: {error}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires a built relay image and a running Docker-compatible engine"]
fn relay_image_auth_encryption_limits_restart_and_backup_restore() {
    let directory = tempfile::tempdir().unwrap();
    let secret = directory.path().join("admin-token");
    std::fs::write(&secret, format!("{ADMIN}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Disposable fixture only. Match Compose's documented UID 10001 read access.
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut runtime = Runtime {
        executable: std::env::var_os("HESSION_CONTAINER_ENGINE").unwrap_or_else(|| "docker".into()),
        image: std::env::var("HESSION_RELAY_TEST_IMAGE")
            .unwrap_or_else(|_| "hession-relay:test".into()),
        containers: Vec::new(),
        volumes: Vec::new(),
    };
    let missing = runtime.command(&["run", "--rm", "--read-only", &runtime.image]);
    assert!(
        !missing.status.success(),
        "missing credentials must prevent startup"
    );
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Set HESSION_RELAY_ADMIN_TOKEN"));
    let volume = runtime.volume();
    let (name, address) = runtime.start(&volume, &secret);
    assert_eq!(runtime.checked(&["exec", &name, "id", "-u"]), "10001");
    let config: Value = serde_json::from_str(&runtime.checked(&[
        "inspect",
        "--format",
        "{{json .HostConfig}}",
        &name,
    ]))
    .unwrap();
    assert_eq!(config["ReadonlyRootfs"], true);
    assert_eq!(config["Memory"], 512 * 1024 * 1024);
    assert_eq!(config["PidsLimit"], 128);
    assert!(config["SecurityOpt"].as_array().unwrap().iter().any(|v| v
        .as_str()
        .is_some_and(|s| s.starts_with("no-new-privileges"))));
    // Engines may expand --cap-drop ALL into individual names in inspect.
    // Check the running process's actual kernel-enforced capability sets.
    let process_status = runtime.checked(&["exec", &name, "cat", "/proc/1/status"]);
    for field in ["CapEff:", "CapPrm:", "CapBnd:", "CapAmb:"] {
        let value = process_status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .expect("process capability set");
        assert_eq!(
            u64::from_str_radix(value.trim(), 16).unwrap(),
            0,
            "{field} must be empty"
        );
    }
    assert!(process_status.lines().any(|line| line
        .strip_prefix("NoNewPrivs:")
        .is_some_and(|s| s.trim() == "1")));
    assert_eq!(
        request(address, "GET", "/v1/status", None, Value::Null).1["initialized"],
        false
    );
    assert_eq!(
        request(address, "GET", "/v1/records", None, Value::Null).0,
        401
    );
    assert_eq!(
        request(
            address,
            "POST",
            "/v1/bootstrap",
            Some("wrong-test-token"),
            json!({"deviceId":"writer","deviceName":"Test writer"})
        )
        .0,
        401
    );
    let (status, enrollment) = request(
        address,
        "POST",
        "/v1/bootstrap",
        Some(ADMIN),
        json!({"deviceId":"writer","deviceName":"Test writer"}),
    );
    assert_eq!(status, 200);
    let token = enrollment["deviceToken"].as_str().unwrap();
    let vault = enrollment["vaultId"].as_str().unwrap();
    assert_eq!(
        request(address, "GET", "/v1/records", Some(ADMIN), Value::Null).0,
        401
    );
    let key = LibraryKey::generate();
    let plaintext = b"CONTAINER_SYNTHETIC_PRIVATE_CONTEXT_Unicode_Test";
    let record = hession_sync::encrypt(&key, vault, "first", "writer", plaintext).unwrap();
    let batch = json!({"records":[record]});
    for _ in 0..2 {
        assert_eq!(
            request(address, "POST", "/v1/records", Some(token), batch.clone()),
            (200, json!({"accepted":1}))
        );
    }
    let conflict =
        hession_sync::encrypt(&key, vault, "first", "writer", b"different content").unwrap();
    assert_eq!(
        request(
            address,
            "POST",
            "/v1/records",
            Some(token),
            json!({"records":[conflict]})
        )
        .0,
        409
    );
    let large = vec![b'x'; hession_sync::MAX_PLAINTEXT];
    for id in ["large-a", "large-b"] {
        let value = hession_sync::encrypt(&key, vault, id, "writer", &large).unwrap();
        assert_eq!(
            request(
                address,
                "POST",
                "/v1/records",
                Some(token),
                json!({"records":[value]})
            )
            .0,
            200
        );
    }
    let too_many = hession_sync::encrypt(&key, vault, "quota", "writer", &large).unwrap();
    assert_eq!(
        request(
            address,
            "POST",
            "/v1/records",
            Some(token),
            json!({"records":[too_many]})
        )
        .0,
        507
    );
    let oversized = json!({"records":[{"id":"oversized","deviceId":"writer","keyEpoch":1,"payload":"A".repeat(hession_relay::MAX_PAYLOAD+4)}]});
    assert_eq!(
        request(address, "POST", "/v1/records", Some(token), oversized).0,
        413
    );
    let (_, feed) = request(address, "GET", "/v1/records", Some(token), Value::Null);
    assert_eq!(
        feed["records"].as_array().unwrap().len(),
        3,
        "retries and rejected quota writes must not add records"
    );
    let mut value = feed["records"][0].clone();
    value.as_object_mut().unwrap().remove("sequence");
    let received: RelayRecord = serde_json::from_value(value).unwrap();
    assert_eq!(
        hession_sync::decrypt(&key, vault, &received).unwrap(),
        plaintext
    );
    let output = runtime.command(&["logs", &name]);
    assert!(output.status.success());
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !logs.contains(token)
            && !logs.contains(ADMIN)
            && !logs.contains(std::str::from_utf8(plaintext).unwrap())
    );
    runtime.checked(&["restart", "--time", "5", &name]);
    let address = runtime.ready(&name, "restart");
    assert_eq!(
        request(address, "GET", "/v1/records", Some(token), Value::Null).1,
        feed
    );
    runtime.checked(&[
        "exec",
        &name,
        "hession-relay",
        "backup",
        "/data/container-backup.sqlite",
    ]);
    let backup = directory.path().join("backup.sqlite");
    runtime.checked(&[
        "cp",
        &format!("{name}:/data/container-backup.sqlite"),
        backup.to_str().unwrap(),
    ]);
    let bytes = std::fs::read(&backup).unwrap();
    for secret in [plaintext.as_slice(), ADMIN.as_bytes(), token.as_bytes()] {
        assert!(
            !bytes.windows(secret.len()).any(|part| part == secret),
            "backup must not retain plaintext secrets"
        );
    }
    let overwrite = runtime.command(&[
        "exec",
        &name,
        "hession-relay",
        "backup",
        "/data/container-backup.sqlite",
    ]);
    assert!(
        !overwrite.status.success(),
        "backup must refuse overwriting an existing file"
    );
    let restored_volume = runtime.volume();
    let copy = format!("{restored_volume}:/data");
    let source = format!("{volume}:/source:ro");
    runtime.checked(&[
        "run",
        "--rm",
        "--entrypoint",
        "/bin/sh",
        "--volume",
        &source,
        "--volume",
        &copy,
        &runtime.image,
        "-c",
        "cp /source/container-backup.sqlite /data/relay.sqlite",
    ]);
    let (_restored, restored_address) = runtime.start(&restored_volume, &secret);
    assert_eq!(
        request(
            restored_address,
            "GET",
            "/v1/records",
            Some(token),
            Value::Null
        )
        .1,
        feed
    );
    assert_eq!(
        request(
            restored_address,
            "POST",
            "/v1/bootstrap",
            Some(ADMIN),
            json!({"deviceId":"other","deviceName":"Do not reset"})
        )
        .0,
        409
    );
    println!(
        "Container auth, E2EE, quota, hardening, persistence, and online backup/restore passed."
    );
}
