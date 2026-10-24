//! Remote hosts over SSH. Agent session stores on each enabled host are
//! mirrored into `<data_dir>/remotes/<host>/root/<remote absolute path>` with
//! rsync, then imported by the normal importer like local sources. rsync keeps
//! remote modification times, so the importer's size + mtime fingerprint skips
//! unchanged files exactly as it does locally. Mirrors are append-only (no
//! `--delete`): sessions survive an agent pruning its own history on the host.
//!
//! Hosts are SSH aliases resolved by the user's own `ssh` (and therefore their
//! `~/.ssh/config`: ProxyJump, keys, Kerberos, ControlMaster all apply).
//! Connections use `BatchMode`, so a host that needs a password fails fast
//! instead of hanging a sync.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// SSH options for every non-interactive connection.
const SSH_OPTS: &[&str] = &["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"];
/// Upper bound for one host's probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound for one host's mirror transfer (first syncs can be large).
const MIRROR_TIMEOUT: Duration = Duration::from_secs(30 * 60);

// Host configuration (`<data_dir>/remotes.json`)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    /// SSH destination: an alias from `~/.ssh/config` or `user@host`.
    pub name: String,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
}

fn enabled_default() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    hosts: Vec<HostConfig>,
}

fn config_path() -> PathBuf {
    crate::db::data_dir().join("remotes.json")
}

pub fn load_hosts() -> Vec<HostConfig> {
    fs::read_to_string(config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Config>(&text).ok())
        .map(|c| c.hosts)
        .unwrap_or_default()
}

fn save_hosts(hosts: &[HostConfig]) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not create data directory: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(&json!({ "hosts": hosts })).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text).map_err(|e| format!("Could not save remote hosts: {e}"))?;
    fs::rename(&temp, &path).map_err(|e| format!("Could not save remote hosts: {e}"))
}

/// Host names are passed to `ssh` as a destination. Refuse anything that could
/// be read as an option or break out of the mirror directory.
pub fn validate_host(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@:[]".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!("Invalid SSH host name: {name}"))
    }
}

/// Filesystem-safe directory name for a host's mirror.
fn host_dir_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn remotes_dir() -> PathBuf {
    crate::db::data_dir().join("remotes")
}

fn host_dir(name: &str) -> PathBuf {
    remotes_dir().join(host_dir_name(name))
}

fn mirror_root(name: &str) -> PathBuf {
    host_dir(name).join("root")
}

/// Local mirror path of an absolute remote path.
pub fn to_mirror(host: &str, remote: &str) -> PathBuf {
    mirror_root(host).join(remote.trim_start_matches('/'))
}

/// `(host, remote absolute path)` when `path` lies inside a host mirror.
pub fn from_mirror(path: &Path) -> Option<(String, String)> {
    let rest = path.strip_prefix(remotes_dir()).ok()?;
    let mut parts = rest.components();
    let dir = parts.next()?.as_os_str().to_str()?.to_string();
    if parts.next()?.as_os_str() != "root" {
        return None;
    }
    let host = load_probe_by_dir(&dir).map(|p| p.host).unwrap_or(dir);
    let remote = Path::new("/").join(parts.as_path());
    Some((host, remote.to_string_lossy().into_owned()))
}

// Probe: resolve the host's agent stores with its own environment

/// One agent store on a remote host. `root`/`sub` follow the importer's
/// `Source` layout; `aux` entries are mirrored for metadata only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRoot {
    pub family: String,
    pub kind: String,
    pub root: String,
    pub sub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Probe {
    pub host: String,
    pub home: String,
    pub roots: Vec<RemoteRoot>,
    pub synced_at: Option<String>,
}

/// POSIX sh, run by the remote login user's shell. Mirrors the importer's
/// local defaults and environment overrides; prints only stores that exist.
const PROBE_SCRIPT: &str = r#"
h=$HOME; x=${XDG_DATA_HOME:-$h/.local/share}
# Roots are printed as physical paths: rsync starting at / copies symlinks
# (macOS /var, Silverblue /home) instead of descending into them.
e() { [ -e "$3/$4" ] && r=$(cd "$3" && pwd -P) && printf 'R\t%s\t%s\t%s\t%s\n' "$1" "$2" "$r" "$4"; return 0; }
printf 'H\t%s\n' "$h"
e codex jsonl "${CODEX_HOME:-$h/.codex}" sessions
e claude jsonl "${CLAUDE_CONFIG_DIR:-$h/.claude}" projects
e opencode opencode "$x/opencode" opencode.db
e pi jsonl "${PI_CODING_AGENT_DIR:-$h/.pi/agent}" sessions
e commandcode jsonl "${COMMANDCODE_HOME:-$h/.commandcode}" projects
e muse jsonl "$x/muse" sessions
e hermes hermes "${HERMES_HOME:-$h/.hermes${HERMES_DATA_DIR_SUFFIX:-}}" .
cl=${CLINE_DIR:-$h/.cline}; cd=${CLINE_DATA_DIR:-$cl/data}
e cline cline "${CLINE_SESSION_DATA_DIR:-$cd/sessions}" .
e cline cline "$cd/tasks" .
e cline aux "${CLINE_DB_DATA_DIR:-$cd/db}" .
for s in .vscode-server .vscode-server-insiders .cursor-server .windsurf-server; do
  e cline cline "$h/$s/data/User/globalStorage/saoudrizwan.claude-dev/tasks" .
done
if [ -n "${ANTIGRAVITY_HOME:-}" ]; then e antigravity antigravity "$ANTIGRAVITY_HOME" brain
else for l in antigravity-cli antigravity; do e antigravity antigravity "$h/.gemini/$l" brain; done; fi
command -v rsync >/dev/null 2>&1 && printf 'S\trsync\n'
exit 0
"#;

/// Run a child to completion, killing it on timeout or a sync stop request.
fn run_bounded(
    mut cmd: Command,
    input: Option<&str>,
    timeout: Duration,
    what: &str,
) -> Result<String, String> {
    let mut child = cmd
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run {what}: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    // Drain pipes on threads so a chatty child cannot block on a full pipe.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_string(&mut s);
        }
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if crate::importer::is_stop_requested() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Import stopped".into());
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{what} timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let detail = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        Err(match status.code() {
            Some(255) => format!("SSH connection failed: {detail}"),
            Some(code) => format!("{what} exited with status {code}: {detail}"),
            None => format!("{what} was terminated"),
        })
    }
}

fn ssh_command(host: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(SSH_OPTS).arg(host);
    cmd
}

fn parse_probe(host: &str, text: &str) -> Result<(Probe, bool), String> {
    let mut home = None;
    let mut roots = Vec::new();
    let mut has_rsync = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["H", h] => home = Some(h.to_string()),
            ["S", "rsync"] => has_rsync = true,
            ["R", family, kind, root, sub] if root.starts_with('/') => roots.push(RemoteRoot {
                family: family.to_string(),
                kind: kind.to_string(),
                root: root.trim_end_matches('/').to_string(),
                sub: if *sub == "." {
                    String::new()
                } else {
                    sub.to_string()
                },
            }),
            // Login banners and shell noise are ignored.
            _ => {}
        }
    }
    let home = home
        .ok_or("The host did not report a home directory (is its login shell POSIX-compatible?)")?;
    Ok((
        Probe {
            host: host.to_string(),
            home,
            roots,
            synced_at: None,
        },
        has_rsync,
    ))
}

fn probe(host: &str) -> Result<(Probe, bool), String> {
    // The script goes over stdin so the remote login shell (which may be csh)
    // never has to parse it.
    let mut cmd = ssh_command(host);
    cmd.args(["sh", "-s"]);
    let text = run_bounded(cmd, Some(PROBE_SCRIPT), PROBE_TIMEOUT, "ssh")?;
    parse_probe(host, &text)
}

fn probe_path(host: &str) -> PathBuf {
    host_dir(host).join("probe.json")
}

pub fn load_probe(host: &str) -> Option<Probe> {
    let text = fs::read_to_string(probe_path(host)).ok()?;
    serde_json::from_str(&text).ok()
}

fn load_probe_by_dir(dir: &str) -> Option<Probe> {
    let text = fs::read_to_string(remotes_dir().join(dir).join("probe.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_probe(probe: &Probe) -> Result<(), String> {
    let path = probe_path(&probe.host);
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("Could not create mirror directory: {e}"))?;
    let text = serde_json::to_string_pretty(probe).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not save host probe: {e}"))
}

// Mirroring

/// Something to copy from a store: a directory (only transcript formats inside
/// it) or a single file (a SQLite database or one of its sidecars).
#[derive(Debug, PartialEq)]
enum Transfer {
    Dir(String),
    File(String),
}

/// What to copy for one store: only what the importer reads.
fn transfers(root: &RemoteRoot) -> Vec<Transfer> {
    let r = &root.root;
    let with_sidecars = |db: String| {
        ["", "-wal", "-shm", "-journal"]
            .map(|s| Transfer::File(format!("{db}{s}")))
            .into_iter()
    };
    match root.kind.as_str() {
        "opencode" => with_sidecars(format!("{r}/opencode.db")).collect(),
        "hermes" => with_sidecars(format!("{r}/state.db"))
            .chain([
                Transfer::Dir(format!("{r}/profiles")),
                Transfer::Dir(format!("{r}/sessions")),
            ])
            .collect(),
        "antigravity" => with_sidecars(format!("{r}/conversation_summaries.db"))
            .chain([Transfer::Dir(format!("{r}/brain"))])
            .collect(),
        _ if root.sub.is_empty() => vec![Transfer::Dir(r.clone())],
        _ => vec![Transfer::Dir(format!("{r}/{}", root.sub))],
    }
}

/// File types copied from transfer directories: transcripts and metadata.
const TRANSCRIPT_TYPES: &[&str] = &["jsonl", "json", "db", "db-wal", "db-shm", "db-journal"];

/// Escape rsync wildcard characters in a literal path.
fn pattern_literal(path: &str) -> String {
    path.chars()
        .flat_map(|c| {
            if "*?[\\".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Filter rules for `rsync host:/ mirror/`: include each transfer and its parent
/// directories, exclude everything else, so rsync only walks the agent stores.
/// Missing paths match nothing. Only rules that both GNU rsync and macOS's
/// openrsync support are used.
fn filter_rules(transfers: &[Transfer]) -> Vec<String> {
    let mut parents = BTreeSet::new();
    let mut contents = Vec::new();
    for transfer in transfers {
        let (path, is_dir) = match transfer {
            Transfer::Dir(p) => (p, true),
            Transfer::File(p) => (p, false),
        };
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let ancestors = if is_dir {
            parts.len()
        } else {
            parts.len().saturating_sub(1)
        };
        for n in 1..=ancestors {
            parents.insert(format!("/{}/", parts[..n].join("/")));
        }
        let literal = pattern_literal(&format!("/{}", parts.join("/")));
        if is_dir {
            contents.push(format!("--include={literal}/**/"));
            contents.extend(
                TRANSCRIPT_TYPES
                    .iter()
                    .map(|ext| format!("--include={literal}/**.{ext}")),
            );
        } else {
            contents.push(format!("--include={literal}"));
        }
    }
    let mut rules: Vec<String> = parents
        .iter()
        .map(|p| format!("--include={}", pattern_literal(p)))
        .collect();
    rules.extend(contents);
    rules.push("--exclude=*".into());
    rules
}

fn mirror_host(host: &str) -> Result<Probe, String> {
    validate_host(host)?;
    let (mut probe, has_rsync) = probe(host)?;
    if !has_rsync {
        return Err("rsync is not installed on the host".into());
    }
    let dest = mirror_root(host);
    fs::create_dir_all(&dest).map_err(|e| format!("Could not create mirror directory: {e}"))?;
    let wanted: Vec<Transfer> = probe.roots.iter().flat_map(transfers).collect();
    if !wanted.is_empty() {
        // One source (`host:/`) keeps absolute paths in the mirror. Compression
        // is SSH's, since macOS's openrsync has no -z.
        let mut cmd = Command::new("rsync");
        cmd.args(["-a", "--timeout=120", "-e"])
            .arg(format!("ssh -C {}", SSH_OPTS.join(" ")))
            .args(filter_rules(&wanted))
            .arg(format!("{host}:/"))
            .arg(format!("{}/", dest.display()));
        if let Err(error) = run_bounded(cmd, None, MIRROR_TIMEOUT, "rsync") {
            // 24 = files vanished during transfer (agents writing); the
            // transferred set is still consistent per file.
            if !error.contains("status 24") {
                return Err(error);
            }
        }
    }
    probe.synced_at = Some(chrono::Utc::now().to_rfc3339());
    save_probe(&probe)?;
    Ok(probe)
}

/// Mirror every enabled host in parallel. Returns `(host, error)` failures;
/// a failed host keeps its previous mirror so its sessions stay importable.
pub fn mirror_enabled_hosts(on_host_done: &(dyn Fn(&str) + Sync)) -> Vec<(String, String)> {
    let hosts: Vec<String> = load_hosts()
        .into_iter()
        .filter(|h| h.enabled)
        .map(|h| h.name)
        .collect();
    if hosts.is_empty() {
        return Vec::new();
    }
    if which("ssh").is_none() || which("rsync").is_none() {
        return hosts
            .into_iter()
            .map(|h| {
                (
                    h,
                    "Remote sync needs `ssh` and `rsync` installed on this computer.".to_string(),
                )
            })
            .collect();
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = hosts
            .iter()
            .map(|host| {
                scope.spawn(move || {
                    let result = mirror_host(host);
                    on_host_done(host);
                    result.err().map(|e| (host.clone(), e))
                })
            })
            .collect();
        workers
            .into_iter()
            .filter_map(|w| w.join().ok().flatten())
            .collect()
    })
}

/// Stores of enabled hosts that have been mirrored at least once, as
/// `(family, kind, local mirror root, sub)`.
pub fn mirrored_roots() -> Vec<(String, String, PathBuf, String)> {
    let mut out = Vec::new();
    for host in load_hosts().into_iter().filter(|h| h.enabled) {
        let Some(probe) = load_probe(&host.name) else {
            continue;
        };
        for root in probe.roots {
            if root.kind != "aux" {
                out.push((
                    root.family,
                    root.kind,
                    to_mirror(&host.name, &root.root),
                    root.sub,
                ));
            }
        }
    }
    out
}

fn which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// Single-quote a string for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// Running agents on a host

/// The command string `ssh -t <host> <this>` runs to resume an agent.
///
/// The remote login shell may be csh, so everything is one line handed to
/// `sh -c`. The agent itself runs under the user's own shell as a login,
/// interactive shell so PATH additions from rc files (nvm, conda, ~/.local/bin)
/// apply; csh variants get `-c` only because they reject `-l` with other flags.
/// Afterwards a login shell keeps the session open in the project directory.
pub fn resume_command(
    cwd: &str,
    env: &[(String, String)],
    binary: &str,
    args: &[String],
) -> String {
    let mut agent = shell_quote(binary);
    for arg in args {
        agent.push(' ');
        agent.push_str(&shell_quote(arg));
    }
    let mut script = format!(
        "cd {} 2>/dev/null || {{ echo 'Workspace folder is unavailable; using home directory.' >&2; cd; }}; ",
        shell_quote(cwd)
    );
    for (key, value) in env {
        script.push_str(&format!("{key}={}; export {key}; ", shell_quote(value)));
    }
    script.push_str(&format!(
        "c={}; case \"${{SHELL:-}}\" in '') sh -c \"$c\";; *csh) \"$SHELL\" -c \"$c\";; *) \"$SHELL\" -lic \"$c\";; esac; exec \"${{SHELL:-/bin/sh}}\" -l",
        shell_quote(&agent)
    ));
    format!("sh -c {}", shell_quote(&script))
}

/// Whether `binary` resolves in the host user's shell. `Err` when the host
/// cannot be reached.
pub fn remote_command_exists(host: &str, binary: &str) -> Result<bool, String> {
    validate_host(host)?;
    let lookup = format!(
        "command -v {0} >/dev/null 2>&1 || which {0} >/dev/null 2>&1",
        shell_quote(binary)
    );
    let script = format!(
        "c={}; case \"${{SHELL:-}}\" in '') sh -c \"$c\";; *csh) \"$SHELL\" -c \"$c\";; *) \"$SHELL\" -lic \"$c\" </dev/null;; esac && echo __FOUND__; exit 0\n",
        shell_quote(&lookup)
    );
    let mut cmd = ssh_command(host);
    cmd.args(["sh", "-s"]);
    let out = run_bounded(cmd, Some(&script), Duration::from_secs(15), "ssh")?;
    Ok(out.lines().any(|l| l.trim() == "__FOUND__"))
}

/// `code --folder-uri` value opening `path` on `host` via Remote-SSH.
pub fn vscode_folder_uri(host: &str, path: &str) -> String {
    let mut encoded = String::new();
    for c in path.chars() {
        match c {
            ' ' => encoded.push_str("%20"),
            '#' => encoded.push_str("%23"),
            '?' => encoded.push_str("%3F"),
            '%' => encoded.push_str("%25"),
            _ => encoded.push(c),
        }
    }
    format!("vscode-remote://ssh-remote+{host}{encoded}")
}

// Host suggestions: ~/.ssh/config and VS Code Remote-SSH history

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Concrete `Host` aliases (no wildcards or negations), following `Include`.
fn ssh_config_hosts() -> Vec<String> {
    let mut out = Vec::new();
    let mut seen_files = BTreeSet::new();
    read_ssh_config(&home().join(".ssh/config"), &mut out, &mut seen_files, 0);
    out
}

fn read_ssh_config(path: &Path, out: &mut Vec<String>, seen: &mut BTreeSet<PathBuf>, depth: usize) {
    if depth > 8 || !seen.insert(path.to_path_buf()) {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        let (key, rest) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, r)) => (
                k,
                r.trim_start_matches(|c: char| c.is_whitespace() || c == '='),
            ),
            None => continue,
        };
        if key.eq_ignore_ascii_case("Host") {
            for name in rest.split_whitespace() {
                if !name.contains(['*', '?', '!'])
                    && validate_host(name).is_ok()
                    && !out.iter().any(|h| h == name)
                {
                    out.push(name.to_string());
                }
            }
        } else if key.eq_ignore_ascii_case("Include") {
            for pattern in rest.split_whitespace() {
                let expanded = if let Some(tail) = pattern.strip_prefix("~/") {
                    home().join(tail)
                } else if pattern.starts_with('/') {
                    PathBuf::from(pattern)
                } else {
                    home().join(".ssh").join(pattern)
                };
                for file in glob_simple(&expanded) {
                    read_ssh_config(&file, out, seen, depth + 1);
                }
            }
        }
    }
}

/// Expand a single trailing-component `*` glob (the common `config.d/*` form).
fn glob_simple(path: &Path) -> Vec<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.contains('*') {
        return vec![path.to_path_buf()];
    }
    let (prefix, suffix) = name.split_once('*').unwrap_or((name, ""));
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect();
    files.sort();
    files
}

/// VS Code state databases for the common editor builds.
fn vscode_state_dbs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = home().join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    ["Code", "Code - Insiders", "VSCodium", "Cursor", "Windsurf"]
        .iter()
        .map(|editor| base.join(editor).join("User/globalStorage/state.vscdb"))
        .filter(|p| p.is_file())
        .collect()
}

/// Hosts from VS Code's recently opened `vscode-remote://ssh-remote+<host>/…`.
fn vscode_recent_hosts() -> Vec<String> {
    let mut out = Vec::new();
    for db in vscode_state_dbs() {
        let Ok(conn) =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            continue;
        };
        let _ = conn.busy_timeout(Duration::from_secs(2));
        let Ok(text) = conn.query_row(
            "SELECT value FROM ItemTable WHERE key='history.recentlyOpenedPathsList'",
            [],
            |r| r.get::<_, String>(0),
        ) else {
            continue;
        };
        for host in remote_hosts_in(&text) {
            if !out.contains(&host) {
                out.push(host);
            }
        }
    }
    out
}

/// Extract SSH hosts from VS Code remote URIs found anywhere in `text`.
fn remote_hosts_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["ssh-remote%2B", "ssh-remote+"] {
        for chunk in text.split(marker).skip(1) {
            let authority: String = chunk
                .chars()
                .take_while(|c| !"/\"?#".contains(*c))
                .collect();
            if let Some(host) = decode_remote_authority(&percent_decode(&authority)) {
                if validate_host(&host).is_ok() && !out.contains(&host) {
                    out.push(host);
                }
            }
        }
    }
    out
}

/// Remote-SSH writes plain hosts, or hex-encoded JSON `{"hostName": …}`
/// when the destination contains characters invalid in a URI authority.
fn decode_remote_authority(authority: &str) -> Option<String> {
    if authority.len() > 2
        && authority.len() % 2 == 0
        && authority.starts_with("7b")
        && authority.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let bytes: Option<Vec<u8>> = (0..authority.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&authority[i..i + 2], 16).ok())
            .collect();
        let value: Value = serde_json::from_slice(&bytes?).ok()?;
        let host = value.get("hostName")?.as_str()?;
        let user = value.get("user").and_then(Value::as_str);
        return Some(match user {
            Some(u) if !u.is_empty() => format!("{u}@{host}"),
            _ => host.to_string(),
        });
    }
    (!authority.is_empty()).then(|| authority.to_string())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Tauri commands

fn host_status(host: &HostConfig) -> Value {
    let probe = load_probe(&host.name);
    json!({
        "name": host.name,
        "enabled": host.enabled,
        "lastSync": probe.as_ref().and_then(|p| p.synced_at.clone()),
        "agents": probe.as_ref().map(|p| {
            p.roots.iter().filter(|r| r.kind != "aux").map(|r| r.family.clone())
                .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>()
        }).unwrap_or_default(),
    })
}

/// Configured hosts plus suggestions from SSH config and VS Code history.
#[tauri::command]
pub async fn get_remote_hosts() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let hosts = load_hosts();
        let configured: BTreeSet<String> = hosts.iter().map(|h| h.name.clone()).collect();
        let vscode = vscode_recent_hosts();
        let mut suggestions: Vec<Value> = Vec::new();
        for name in vscode.iter().chain(ssh_config_hosts().iter()) {
            if configured.contains(name) || suggestions.iter().any(|s| s["name"] == *name) {
                continue;
            }
            suggestions.push(json!({ "name": name, "source": if vscode.contains(name) { "vscode" } else { "ssh-config" } }));
        }
        Ok(json!({
            "hosts": hosts.iter().map(host_status).collect::<Vec<_>>(),
            "suggestions": suggestions,
            "tools": { "ssh": which("ssh").is_some(), "rsync": which("rsync").is_some() },
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Replace the configured host list (`[{ name, enabled }]`).
#[tauri::command]
pub fn set_remote_hosts(hosts: Vec<HostConfig>) -> Result<Value, String> {
    let mut clean: Vec<HostConfig> = Vec::new();
    for mut host in hosts {
        host.name = host.name.trim().to_string();
        validate_host(&host.name)?;
        if !clean.iter().any(|h| h.name == host.name) {
            clean.push(host);
        }
    }
    save_hosts(&clean)?;
    Ok(json!({ "ok": true }))
}

/// Connect to a host and report which agent stores it has, without syncing.
#[tauri::command]
pub async fn test_remote_host(name: String) -> Result<Value, String> {
    let name = name.trim().to_string();
    validate_host(&name)?;
    tauri::async_runtime::spawn_blocking(move || match probe(&name) {
        Ok((probe, has_rsync)) => Ok(json!({
            "ok": true,
            "home": probe.home,
            "rsync": has_rsync,
            "agents": probe.roots.iter().filter(|r| r.kind != "aux").map(|r| r.family.clone())
                .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>(),
        })),
        Err(error) => Ok(json!({ "ok": false, "error": error })),
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_output_and_ignores_noise() {
        let text = "Welcome to devbox!\nH\t/home/u\nR\tclaude\tjsonl\t/home/u/.claude\tprojects\nR\thermes\thermes\t/home/u/.hermes/\t.\nS\trsync\n";
        let (probe, rsync) = parse_probe("lx", text).unwrap();
        assert!(rsync);
        assert_eq!(probe.home, "/home/u");
        assert_eq!(probe.roots.len(), 2);
        assert_eq!(probe.roots[1].root, "/home/u/.hermes");
        assert_eq!(probe.roots[1].sub, "");
        assert_eq!(
            transfers(&probe.roots[0]),
            vec![Transfer::Dir("/home/u/.claude/projects".into())]
        );
    }

    #[test]
    fn filters_include_only_stores_and_their_parents() {
        let rules = filter_rules(&[
            Transfer::Dir("/home/u/.claude/projects".into()),
            Transfer::File("/home/u/.hermes/state.db".into()),
        ]);
        assert_eq!(
            rules[..4],
            [
                "--include=/home/",
                "--include=/home/u/",
                "--include=/home/u/.claude/",
                "--include=/home/u/.claude/projects/"
            ]
        );
        assert!(rules.contains(&"--include=/home/u/.hermes/".to_string()));
        assert!(rules.contains(&"--include=/home/u/.claude/projects/**.jsonl".to_string()));
        assert!(rules.contains(&"--include=/home/u/.hermes/state.db".to_string()));
        assert_eq!(rules.last().unwrap(), "--exclude=*");
        assert_eq!(pattern_literal("/a [b]/c*"), r"/a \[b]/c\*");
    }

    #[test]
    fn validates_hosts() {
        for ok in ["devbox", "user@host.example.org", "build-01", "[::1]"] {
            assert!(validate_host(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-oProxyCommand=x", "a b", "h;rm", "..", "a/b"] {
            assert!(validate_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn extracts_vscode_remote_hosts() {
        // {"hostName":"box.example","user":"me"} hex-encoded.
        let hex: String = br#"{"hostName":"box.example","user":"me"}"#.iter().map(|b| format!("{b:02x}")).collect();
        let text = format!(
            r#"{{"entries":[{{"folderUri":"vscode-remote://ssh-remote%2Bdevbox/home/u/p"}},{{"folderUri":"vscode-remote://ssh-remote%2B{hex}/srv"}},{{"folderUri":"file:///tmp"}}]}}"#
        );
        assert_eq!(
            remote_hosts_in(&text),
            vec!["devbox".to_string(), "me@box.example".to_string()]
        );
    }

    #[test]
    fn quotes_for_shell() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// Runs the generated remote command through a real `sh` with a stub
    /// agent, checking quoting of awkward paths and arguments end to end.
    #[cfg(unix)]
    #[test]
    fn resume_command_runs_agent_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("my 'proj' $x");
        fs::create_dir(&work).unwrap();
        let agent = dir.path().join("agent");
        fs::write(
            &agent,
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$PWD\" \"$1\" \"$HERMES_HOME\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        let line = resume_command(
            work.to_str().unwrap(),
            &[("HERMES_HOME".into(), "/h o".into())],
            agent.to_str().unwrap(),
            &["--resume=a'b".into()],
        );
        // `exec $SHELL -l` would wait for input; a non-interactive stub shell exits.
        let out = Command::new("sh")
            .arg("-c")
            .arg(&line)
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains(&format!("{}|--resume=a'b|/h o", work.display())),
            "{text}"
        );
    }

    /// End to end: probe, rsync mirror, and import through a stand-in `ssh`
    /// that runs commands locally against a fake remote home. Mutates
    /// process-wide environment (HOME, PATH, data dir), so it is the only test
    /// that does.
    #[cfg(unix)]
    #[test]
    fn mirrors_and_imports_remote_sessions() {
        use std::os::unix::fs::PermissionsExt;
        if which("rsync").is_none() {
            eprintln!("skipping: rsync not installed");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        // Physical path: on macOS the temp dir is under the /var symlink.
        let root = fs::canonicalize(tmp.path()).unwrap();
        let (bin, local_home, remote_home, data) = (
            root.join("bin"),
            root.join("local"),
            root.join("remote"),
            root.join("data"),
        );
        for dir in [&bin, &local_home, &data] {
            fs::create_dir_all(dir).unwrap();
        }
        let ssh = bin.join("ssh");
        fs::write(
            &ssh,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) shift 2;; -*) shift;; *) break;; esac; done\nshift\nHOME=\"$FAKE_REMOTE_HOME\"; export HOME\nexec sh -c \"$*\"\n",
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        let project = remote_home.join(".claude/projects/-work-proj");
        fs::create_dir_all(&project).unwrap();
        let id = "0b7e8a52-1c1d-4c55-9a1e-3f8d2f6f0a11";
        fs::write(
            project.join(format!("{id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{id}\",\"cwd\":\"/work/proj\",\"timestamp\":\"2026-10-01T10:00:00Z\",\"uuid\":\"u1\",\"message\":{{\"role\":\"user\",\"content\":\"hello from the host\"}}}}\n"
            ),
        )
        .unwrap();
        // Not an importer format; must not be mirrored.
        fs::write(project.join("notes.txt"), "skip").unwrap();

        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::env::set_var("PATH", path);
        std::env::set_var("HOME", &local_home);
        // The host's HOME is reached through a symlink (like macOS /var or
        // Silverblue /home); the mirror must still find the stores.
        std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
        std::env::set_var("FAKE_REMOTE_HOME", root.join("link/remote"));
        std::env::set_var(crate::brand::env_var("DATA_DIR"), &data);

        set_remote_hosts(vec![HostConfig {
            name: "box".into(),
            enabled: true,
        }])
        .unwrap();
        let sync = || crate::importer::sync_all_with_progress(None, "incremental", &|_, _, _| {});
        let summary = sync();
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);

        let remote_file = project.join(format!("{id}.jsonl"));
        let mirrored = to_mirror("box", remote_file.to_str().unwrap());
        assert!(mirrored.is_file());
        assert!(!to_mirror("box", project.join("notes.txt").to_str().unwrap()).exists());
        assert_eq!(
            from_mirror(&mirrored),
            Some(("box".into(), remote_file.display().to_string()))
        );

        let conn = crate::db::open_db(&crate::db::db_path()).unwrap();
        let (host, cwd, workspace): (String, String, String) = conn
            .query_row(
                "SELECT s.host, s.cwd, w.path FROM session s JOIN workspace w ON w.id=s.workspace_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (host.as_str(), cwd.as_str(), workspace.as_str()),
            ("box", "/work/proj", "box:/work/proj")
        );

        // Unchanged remote files are skipped on the next sync.
        let again = sync();
        assert!(again.errors.is_empty(), "{:?}", again.errors);
        assert_eq!(again.records, 0);
    }

    #[test]
    fn builds_vscode_uri() {
        assert_eq!(
            vscode_folder_uri("devbox", "/home/u/my proj"),
            "vscode-remote://ssh-remote+devbox/home/u/my%20proj"
        );
    }
}
