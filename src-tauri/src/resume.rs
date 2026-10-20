//! Resume any selected session in its native CLI.
//!
//! Backend for the "Resume" action: maps a stored session row
//! (`family`, `external_id`, `source_uri`, `cwd`) to the correct native
//! resume invocation, then spawns a user-visible terminal cross-platform.
//!
//! Design notes:
//! - All spawning happens in backend `std::process::Command` (never frontend
//!   shell plugin, never `sh -c` string interpolation). Session keys travel
//!   as `argv` entries; only the Linux/macOS terminal wrapper builds a POSIX
//!   shell line, using single-quote escaping.
//! - `get_resume_info` is side-effect free (preflight + copyable command).
//! - `resume_session` reuses the same plan builder, then spawns detached.
//! - Expected failures (unknown family, missing CLI/terminal/session files)
//!   return `Ok({ ok: false, ... })` so the frontend can show the copyable
//!   command. Only unexpected failures return `Err`.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::AppState;
use crate::db;
use tauri::State;

/// Families accepted for resume (mirrors `ALLOWED_FAMILIES` in commands.rs).
const RESUME_FAMILIES: &[&str] = &[
    "cline",
    "codex",
    "claude",
    "opencode",
    "pi",
    "commandcode",
    "muse",
    "hermes",
    "antigravity",
];

/// Candidate binaries per family (first existing wins).
fn cli_candidates(family: &str) -> &'static [&'static str] {
    match family {
        "cline" => &["cline"],
        "codex" => &["codex"],
        "claude" => &["claude"],
        "opencode" => &["opencode"],
        "pi" => &["pi"],
        #[cfg(not(windows))]
        "commandcode" => &["commandcode", "cmd"],
        // Windows cmd.exe is the OS shell, not a CommandCode CLI.
        #[cfg(windows)]
        "commandcode" => &["commandcode", "cmd.cmd", "cmd.bat"],
        "muse" => &["muse"],
        "hermes" => &["hermes"],
        "antigravity" => &["agy", "antigravity"],
        _ => &[],
    }
}

fn home_dir() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return PathBuf::from(v.trim());
            }
        }
    }
    PathBuf::from(".")
}

fn default_codex_home() -> PathBuf {
    match std::env::var("CODEX_HOME") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => home_dir().join(".codex"),
    }
}

fn default_claude_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => home_dir().join(".claude"),
    }
}

fn default_xdg_data() -> PathBuf {
    match std::env::var("XDG_DATA_HOME") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => home_dir().join(".local").join("share"),
    }
}

fn default_pi_dir() -> PathBuf {
    match std::env::var("PI_CODING_AGENT_DIR") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => home_dir().join(".pi").join("agent"),
    }
}

fn default_commandcode_home() -> PathBuf {
    match std::env::var("COMMANDCODE_HOME") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => home_dir().join(".commandcode"),
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn db_path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|p| p.clone())
        .map_err(|_| "App state lock was poisoned".to_string())
}

fn open_db(path: &Path) -> Result<rusqlite::Connection, String> {
    db::open_db(path).map_err(|e| e.to_string())
}

/// Reject empty / control-containing / flag-like keys. Argv passing makes
/// spaces safe, but a leading `-` would parse as a flag.
fn validate_resume_key(key: &str) -> Result<(), String> {
    let t = key.trim();
    if t.is_empty() {
        return Err("Session has no resume identifier.".to_string());
    }
    if t.len() > 256 {
        return Err("Session identifier is unexpectedly long.".to_string());
    }
    if t.starts_with('-') {
        return Err("Session identifier is not resumable.".to_string());
    }
    if t.chars().any(|c| c.is_control()) {
        return Err("Session identifier is not resumable.".to_string());
    }
    Ok(())
}

fn looks_like_uuid(s: &str) -> bool {
    let t = s.trim();
    (8..=64).contains(&t.len())
        && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && t.contains('-')
}

/// POSIX single-quote escaping for the terminal wrapper script only.
fn posix_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
    });
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Literal value in a Windows batch file. Delayed expansion is disabled by
/// the launcher; percent signs must be doubled in batch source. Quotes and
/// control characters cannot be safely passed through npm batch shims.
#[cfg(windows)]
fn batch_value(s: &str) -> Result<String, String> {
    if s.chars().any(|c| c == '"' || c.is_control()) {
        return Err(
            "The resume command contains characters that Windows batch shims cannot safely handle."
                .into(),
        );
    }
    Ok(s.replace('%', "%%"))
}

#[cfg(windows)]
fn write_windows_launcher(plan: &ResumePlan, cli_path: &Path) -> Result<PathBuf, String> {
    use std::io::Write;
    let mut script =
        String::from("@echo off\r\nsetlocal DisableDelayedExpansion\r\nchcp 65001 >nul\r\n");
    script.push_str(&format!(
        "cd /d \"{}\"\r\nif errorlevel 1 exit /b 1\r\n",
        batch_value(&plan.cwd.to_string_lossy())?
    ));
    script.push_str(&format!(
        "set \"PATH={}\"\r\n",
        batch_value(&augmented_path())?
    ));
    let mut env: Vec<_> = plan.env.iter().collect();
    env.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in env {
        script.push_str(&format!(
            "set \"{}={}\"\r\n",
            batch_value(key)?,
            batch_value(value)?
        ));
    }
    script.push_str(&format!(
        "\"{}\"",
        batch_value(&cli_path.to_string_lossy())?
    ));
    for arg in &plan.args {
        script.push_str(&format!(" \"{}\"", batch_value(arg)?));
    }
    script.push_str("\r\n");
    // Keep the file until the terminal has read it; filenames are unique and
    // created exclusively rather than overwriting predictable temp paths.
    let mut file = tempfile::Builder::new()
        .prefix(&format!("{}-resume-", crate::brand::SLUG))
        .suffix(".cmd")
        .tempfile()
        .map_err(|e| format!("Could not create launcher script: {e}"))?;
    file.write_all(script.as_bytes())
        .map_err(|e| format!("Could not write launcher script: {e}"))?;
    let (_, path) = file
        .keep()
        .map_err(|e| format!("Could not keep launcher script: {e}"))?;
    Ok(path)
}

/// Copyable command shown in the UI.
fn display_command(cwd: &Path, binary: &str, args: &[String]) -> String {
    #[cfg(windows)]
    {
        // PowerShell literal strings preserve Windows paths, spaces and shell
        // metacharacters in the clipboard command. The terminal launcher uses
        // a separate batch wrapper for Command Prompt.
        let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let mut invocation = vec![quote(binary)];
        invocation.extend(args.iter().map(|s| quote(s)));
        return format!(
            "Set-Location -LiteralPath {}; if ($?) {{ & {} }}",
            quote(&cwd.to_string_lossy()),
            invocation.join(" ")
        );
    }
    #[cfg(not(windows))]
    {
        let mut invocation = vec![posix_quote(binary)];
        invocation.extend(args.iter().map(|s| posix_quote(s)));
        format!(
            "cd {} && {}",
            posix_quote(&cwd.to_string_lossy()),
            invocation.join(" ")
        )
    }
}

// ---------------------------------------------------------------------------
// Binary + terminal discovery (same PATH as launched terminals)
// ---------------------------------------------------------------------------

fn path_dirs() -> Vec<PathBuf> {
    std::env::split_paths(&augmented_path()).collect()
}

fn find_cli_binary(candidates: &[&str]) -> Option<PathBuf> {
    let dirs = path_dirs();
    for name in candidates {
        #[cfg(windows)]
        let names: Vec<String> = if Path::new(name).extension().is_some() {
            vec![name.to_string()]
        } else {
            vec![
                format!("{name}.exe"),
                format!("{name}.com"),
                format!("{name}.cmd"),
                format!("{name}.bat"),
                name.to_string(),
            ]
        };
        #[cfg(not(windows))]
        let names: Vec<String> = vec![name.to_string()];
        for d in &dirs {
            for n in &names {
                let p = d.join(n);
                if p.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if !p
                            .metadata()
                            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
                        {
                            continue;
                        }
                    }
                    return Some(p);
                }
            }
        }
        // Bare lookup as last resort (relies on OS search).
        // Confirm it resolves by checking `PATH` dirs only above; skip bare.
    }
    None
}

/// Probe the user's login shell for its PATH (picks up homebrew, nvm, conda,
/// volta, etc. that GUI-launched apps don't inherit). Only stdout's last line
/// is used; anything not shaped like a PATH is ignored.
#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    let mut shells = Vec::new();
    if let Ok(s) = std::env::var("SHELL") {
        if !s.trim().is_empty() {
            shells.push(s.trim().to_string());
        }
    }
    for s in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if !shells.contains(&s.to_string()) {
            shells.push(s.to_string());
        }
    }
    for sh in shells {
        let Ok(out) = Command::new(&sh)
            .arg("-l")
            .arg("-c")
            .arg("printf '%s' \"$PATH\"")
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if let Some(line) = text.lines().rfind(|l| !l.trim().is_empty()) {
            let line = line.trim();
            if line.contains('/') && line.len() < 32 * 1024 {
                return Some(line.to_string());
            }
        }
    }
    None
}

/// Newest `~/.nvm/versions/node/v*/bin` (nvm lives in shell init, not on the
/// GUI PATH, so probe it explicitly).
#[cfg(unix)]
fn nvm_latest_bin() -> Option<PathBuf> {
    let base = home_dir().join(".nvm").join("versions").join("node");
    let entries = std::fs::read_dir(&base).ok()?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let v = name.strip_prefix('v').unwrap_or(&name);
        let nums: Option<Vec<u64>> = v
            .split('.')
            .map(|p| p.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>();
        let Some(nums) = nums else { continue };
        let bin = entry.path().join("bin");
        if !bin.is_dir() {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| nums > *b) {
            best = Some((nums, bin));
        }
    }
    best.map(|(_, p)| p)
}

/// PATH for launched terminals: login-shell PATH first, then anything extra
/// from the current process, then well-known tool dirs. GUI-launched apps get
/// a skeletal PATH, which is why `node` (nvm) and similar shims fail inside
/// resumed windows without this. Cached per process.
fn augmented_path() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut seen = std::collections::HashSet::new();
            let mut out: Vec<String> = Vec::new();
            let mut push = |d: &str| {
                let t = d.trim();
                if !t.is_empty() && seen.insert(t.to_string()) {
                    out.push(t.to_string());
                }
            };
            #[cfg(unix)]
            if let Some(p) = login_shell_path() {
                #[cfg(windows)]
                let sep = ';';
                #[cfg(not(windows))]
                let sep = ':';
                for d in p.split(sep) {
                    push(d);
                }
            }
            if let Ok(p) = std::env::var("PATH") {
                #[cfg(windows)]
                let sep = ';';
                #[cfg(not(windows))]
                let sep = ':';
                for d in p.split(sep) {
                    push(d);
                }
            }
            let home = home_dir();
            #[cfg(windows)]
            if let Ok(appdata) = std::env::var("APPDATA") {
                push(&PathBuf::from(appdata).join("npm").to_string_lossy());
            }
            let mut extras = vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/opt/homebrew/sbin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/local/sbin"),
                home.join(".local").join("bin"),
                home.join(".cargo").join("bin"),
                home.join(".volta").join("bin"),
                home.join(".asdf").join("shims"),
                home.join(".pyenv").join("shims"),
                PathBuf::from("/opt/local/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                PathBuf::from("/sbin"),
            ];
            #[cfg(unix)]
            if let Some(nvm) = nvm_latest_bin() {
                extras.push(nvm);
            }
            for d in extras {
                // Only add tool dirs that exist, except the system defaults
                // which are required for `cd`/`exec` even in odd environments.
                let s = d.to_string_lossy().to_string();
                if d.is_dir() || matches!(s.as_str(), "/usr/bin" | "/bin" | "/usr/sbin" | "/sbin") {
                    push(&s);
                }
            }
            out.join(
                #[cfg(windows)]
                {
                    ";"
                },
                #[cfg(not(windows))]
                {
                    ":"
                },
            )
        })
        .clone()
}

/// Run `<cli> --version` with the augmented PATH. Returns
/// `(version_text, error_text)`: version on success, stderr on failure
/// so callers can surface *why* a found CLI doesn't run (deleted conda env,
/// missing node, …) instead of opening a dead terminal.
fn cli_check(cli: &Path) -> (Option<String>, Option<String>) {
    let out = match Command::new(cli)
        .arg("--version")
        .env("PATH", augmented_path())
        .output()
    {
        Ok(o) => o,
        Err(e) => return (None, Some(format!("could not execute: {e}"))),
    };
    if out.status.success() {
        let mut s = String::from_utf8_lossy(&out.stdout).to_string();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        let t = s.trim().replace(|c: char| c.is_control(), " ");
        if t.is_empty() {
            (Some("ok".to_string()), None)
        } else {
            (Some(t.chars().take(160).collect()), None)
        }
    } else {
        let mut s = String::from_utf8_lossy(&out.stderr).to_string();
        if s.trim().is_empty() {
            s = String::from_utf8_lossy(&out.stdout).to_string();
        }
        if let Some(code) = out.status.code() {
            s.push_str(&format!(" (exit {code})"));
        }
        let t = s.trim().replace(|c: char| c.is_control(), " ");
        if t.is_empty() {
            (None, Some("exited unsuccessfully".to_string()))
        } else {
            (None, Some(t.chars().take(300).collect()))
        }
    }
}

/// (terminal_label, terminal_binary)
/// Resolved user terminal choice. IDs are stable per OS: macOS uses
/// `terminal|iterm|wezterm|kitty|alacritty|ghostty`, Windows uses `wt|cmd`,
/// Linux uses the emulator binary name. The choice persists in
/// `<data_dir>/terminal-preference.json` so future invocations reuse it.
#[cfg(target_os = "macos")]
const KNOWN_TERMINAL_IDS: &[&str] = &[
    "terminal",
    "iterm",
    "wezterm",
    "kitty",
    "alacritty",
    "ghostty",
];
#[cfg(target_os = "windows")]
const KNOWN_TERMINAL_IDS: &[&str] = &["wt", "cmd"];

fn terminal_label(id: &str) -> String {
    match id {
        "terminal" => "Terminal".to_string(),
        "iterm" => "iTerm2".to_string(),
        "wezterm" => "WezTerm".to_string(),
        "kitty" => "Kitty".to_string(),
        "alacritty" => "Alacritty".to_string(),
        "ghostty" => "Ghostty".to_string(),
        "wt" => "Windows Terminal".to_string(),
        "cmd" => "Command Prompt".to_string(),
        "gnome-terminal" => "GNOME Terminal".to_string(),
        "konsole" => "Konsole".to_string(),
        "xfce4-terminal" => "Xfce Terminal".to_string(),
        "x-terminal-emulator" => "Default terminal".to_string(),
        "xterm" => "XTerm".to_string(),
        "foot" => "Foot".to_string(),
        other => other.to_string(),
    }
}

/// Application bundle directory on macOS (`/Applications` or `~/Applications`).
#[cfg(target_os = "macos")]
fn mac_app_dir(bundle: &str) -> Option<PathBuf> {
    for base in [
        PathBuf::from("/Applications"),
        home_dir().join("Applications"),
    ] {
        let p = base.join(bundle);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// CLI inside a macOS app bundle, falling back to `PATH`.
#[cfg(target_os = "macos")]
fn mac_bundle_cli(bundle: &str, rel: &str, name: &str) -> Option<PathBuf> {
    if let Some(p) = find_cli_binary(&[name]) {
        return Some(p);
    }
    mac_app_dir(bundle)
        .map(|d| d.join(rel))
        .filter(|p| p.is_file())
}

/// Whether a terminal choice can be launched right now.
fn is_terminal_available(id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        match id {
            "terminal" => {
                find_cli_binary(&["open"]).is_some() || PathBuf::from("/usr/bin/open").is_file()
            }
            "iterm" => {
                mac_app_dir("iTerm.app").is_some()
                    && (find_cli_binary(&["osascript"]).is_some()
                        || PathBuf::from("/usr/bin/osascript").is_file())
            }
            "ghostty" => mac_app_dir("Ghostty.app").is_some(),
            "wezterm" => {
                mac_bundle_cli("WezTerm.app", "Contents/MacOS/wezterm", "wezterm").is_some()
            }
            "kitty" => mac_bundle_cli("kitty.app", "Contents/MacOS/kitty", "kitty").is_some(),
            "alacritty" => {
                mac_bundle_cli("Alacritty.app", "Contents/MacOS/alacritty", "alacritty").is_some()
            }
            _ => false,
        }
    }
    #[cfg(target_os = "windows")]
    {
        match id {
            "wt" => find_cli_binary(&["wt"]).is_some(),
            "cmd" => true,
            _ => false,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if id.contains('/') {
            return PathBuf::from(id).is_file();
        }
        find_cli_binary(&[id]).is_some()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_terminal_candidates() -> Vec<String> {
    let mut ids = Vec::new();
    if let Ok(t) = std::env::var("TERMINAL") {
        let t = t.trim();
        if !t.is_empty() {
            ids.push(t.to_string());
        }
    }
    for n in [
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "alacritty",
        "kitty",
        "wezterm",
        "foot",
        "xterm",
    ] {
        ids.push(n.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids.into_iter()
        .filter(|id| is_terminal_available(id))
        .collect()
}

/// Every choice shown in the picker, with live availability.
fn available_terminals() -> Vec<Value> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let choices: Vec<(String, bool)> = KNOWN_TERMINAL_IDS
        .iter()
        .map(|id| (id.to_string(), is_terminal_available(id)))
        .collect();
    // Linux candidates are only the emulators found on PATH.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let choices: Vec<(String, bool)> = linux_terminal_candidates()
        .into_iter()
        .map(|id| (id, true))
        .collect();
    choices
        .into_iter()
        .map(|(id, available)| json!({ "id": id, "label": terminal_label(&id), "available": available }))
        .collect()
}

fn first_available_terminal() -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let first = KNOWN_TERMINAL_IDS
        .iter()
        .find(|id| is_terminal_available(id))
        .map(|s| s.to_string());
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let first = linux_terminal_candidates().into_iter().next();
    first
}

/// Built-in default when nothing is saved.
fn default_terminal_id() -> String {
    #[cfg(target_os = "macos")]
    let id = "terminal".to_string();
    #[cfg(target_os = "windows")]
    let id = if is_terminal_available("wt") {
        "wt"
    } else {
        "cmd"
    }
    .to_string();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let id = first_available_terminal().unwrap_or_default();
    id
}

fn terminal_pref_path() -> PathBuf {
    db::data_dir().join("terminal-preference.json")
}

fn load_terminal_pref() -> Option<String> {
    let raw = std::fs::read_to_string(terminal_pref_path()).ok()?;
    let id = serde_json::from_str::<Value>(&raw)
        .ok()?
        .get("terminal")?
        .as_str()?
        .trim()
        .to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn validate_terminal_id(id: &str) -> Result<(), String> {
    let t = id.trim();
    if t.is_empty() || t.len() > 64 || t.chars().any(|c| c.is_control()) {
        return Err("Unknown terminal.".to_string());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if !KNOWN_TERMINAL_IDS.contains(&t) {
            return Err("Unknown terminal.".to_string());
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
        {
            return Err("Unknown terminal.".to_string());
        }
    }
    Ok(())
}

fn save_terminal_pref(id: &str) -> Result<(), String> {
    validate_terminal_id(id)?;
    let id = id.trim().to_string();
    let dir = db::data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not save terminal preference: {e}"))?;
    let path = terminal_pref_path();
    std::fs::write(&path, format!("{{\"terminal\":{}}}", json!(id)))
        .map_err(|e| format!("Could not save terminal preference: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Effective terminal: explicit request → saved preference → default.
/// Returns `(id, fell_back)`; `None` when nothing can launch.
fn resolve_terminal(requested: Option<&str>) -> (Option<String>, bool) {
    let saved = load_terminal_pref();
    let want = requested
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(saved);
    match want {
        Some(id) if is_terminal_available(&id) => (Some(id), false),
        Some(id) => {
            let def = default_terminal_id();
            if def != id && is_terminal_available(&def) {
                return (Some(def), true);
            }
            match first_available_terminal() {
                Some(first) if first != id => (Some(first), true),
                other => (other, true),
            }
        }
        None => {
            let def = default_terminal_id();
            if def.is_empty() || !is_terminal_available(&def) {
                return (first_available_terminal(), false);
            }
            (Some(def), false)
        }
    }
}

// ---------------------------------------------------------------------------
// Session row + resume plan
// ---------------------------------------------------------------------------

struct SessionRow {
    family: String,
    product: String,
    external_id: String,
    source_uri: String,
    cwd: String,
    project: String,
    /// SSH host for sessions mirrored from a remote machine.
    host: Option<String>,
}

fn load_session_row(conn: &rusqlite::Connection, id: i64) -> Result<SessionRow, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.external_id, s.source_uri, s.source_format, s.cwd, \
              a.family, a.product, w.path AS project, s.host \
             FROM session s JOIN agent a ON a.id=s.agent_id \
             LEFT JOIN workspace w ON w.id=s.workspace_id \
             WHERE s.id=?1",
        )
        .map_err(|e| format!("Could not prepare query: {e}"))?;
    let row = stmt
        .query_row([id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(|_| "Session not found".to_string())?;
    Ok(SessionRow {
        external_id: row.0.unwrap_or_default(),
        source_uri: row.1,
        cwd: row.2.unwrap_or_default(),
        family: row.3,
        product: row.4.unwrap_or_default(),
        project: row.5.unwrap_or_default(),
        host: row.6.filter(|h| !h.is_empty()),
    })
}

struct ResumePlan {
    binary: String,
    args: Vec<String>,
    cwd: PathBuf,
    cwd_fallback: bool,
    env: HashMap<String, String>,
    display: String,
    warnings: Vec<String>,
    source_file: PathBuf,
    resume_key: String,
    profile: Option<String>,
    /// Set for remote sessions: the plan runs `ssh -t <host> …` locally.
    remote: Option<RemoteTarget>,
}

struct RemoteTarget {
    host: String,
    /// Agent binary on the host (`binary` is `ssh` for remote plans).
    agent: String,
}

fn split_source_uri(source_uri: &str) -> Result<(String, PathBuf, Option<String>), String> {
    let pos = source_uri
        .find("://")
        .ok_or_else(|| "Session source is unavailable.".to_string())?;
    let family = source_uri[..pos].to_string();
    let rest = &source_uri[pos + 3..];
    if family.is_empty() || rest.is_empty() {
        return Err("Session source is unavailable.".to_string());
    }
    // opencode/hermes append `#{id}` to a `.db` path.
    if family == "opencode" || family == "hermes" {
        if let Some((path, frag)) = rest.rsplit_once('#') {
            if !path.is_empty() && !frag.is_empty() {
                return Ok((family, PathBuf::from(path), Some(frag.to_string())));
            }
        }
    }
    Ok((family, PathBuf::from(rest), None))
}

fn normalized_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn prefix_before<'a>(hay: &'a str, needle: &str) -> Option<&'a str> {
    hay.find(needle).map(|i| &hay[..i])
}

fn derive_resume_key(
    family: &str,
    external_id: &str,
    source_file: &Path,
    fragment: Option<&str>,
) -> Result<String, String> {
    let ext = external_id.trim().to_string();
    match family {
        "cline" => {
            if !source_file
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".messages.json"))
            {
                return Err("Resume this Cline task from History in its original editor. The current Cline CLI resumes SDK sessions, not editor task files.".into());
            }
            validate_resume_key(&ext)?;
            Ok(ext)
        }
        "opencode" => {
            let key = fragment
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&ext)
                .to_string();
            if !key.starts_with("ses_") {
                return Err("OpenCode session identifier is missing.".to_string());
            }
            validate_resume_key(&key)?;
            Ok(key)
        }
        "hermes" => {
            let key = fragment
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&ext)
                .to_string();
            validate_resume_key(&key)?;
            Ok(key)
        }
        "muse" => {
            // Never the file stem ("session"). Parent dir is the UUID.
            let uuid = source_file
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !looks_like_uuid(&uuid) {
                return Err("Muse session identifier is missing.".to_string());
            }
            Ok(uuid)
        }
        "antigravity" => {
            if looks_like_uuid(&ext)
                || (!ext.is_empty()
                    && !ext.eq_ignore_ascii_case("transcript")
                    && !ext.starts_with('-'))
            {
                // Prefer stored id, but it must be UUID-shaped when present.
                if looks_like_uuid(&ext) {
                    return Ok(ext);
                }
            }
            // Fall back to brain/<uuid>/ segment.
            let norm = normalized_slashes(source_file);
            if let Some(after) = norm.split("/brain/").nth(1) {
                let seg = after.split('/').next().unwrap_or("").trim().to_string();
                if looks_like_uuid(&seg) {
                    return Ok(seg);
                }
            }
            // Last resort: any UUID-looking path segment.
            for seg in source_file
                .components()
                .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
            {
                if looks_like_uuid(&seg) {
                    return Ok(seg);
                }
            }
            Err("Antigravity conversation identifier is missing.".to_string())
        }
        "codex" => {
            if !ext.is_empty() {
                validate_resume_key(&ext)?;
                return Ok(ext);
            }
            // Derive from rollout-<ts>-<uuid>.jsonl
            let stem = source_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let parts: Vec<&str> = stem.split('-').collect();
            if parts.len() >= 5 {
                let candidate = parts[parts.len() - 5..].join("-");
                if looks_like_uuid(&candidate) {
                    return Ok(candidate);
                }
            }
            Err("Codex session identifier is missing.".to_string())
        }
        "claude" => {
            if !ext.is_empty() {
                validate_resume_key(&ext)?;
                return Ok(ext);
            }
            let stem = source_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if looks_like_uuid(&stem) {
                return Ok(stem);
            }
            Err("Claude session identifier is missing.".to_string())
        }
        "pi" => {
            if !ext.is_empty() {
                validate_resume_key(&ext)?;
                return Ok(ext);
            }
            // <timestamp>_<uuid>.jsonl → UUID suffix.
            let stem = source_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if let Some((_, uuid)) = stem.rsplit_once('_') {
                if !uuid.trim().is_empty() {
                    validate_resume_key(uuid.trim())?;
                    return Ok(uuid.trim().to_string());
                }
            }
            // Absolute path always works with `pi --session <path>`.
            Ok(source_file.to_string_lossy().to_string())
        }
        "commandcode" => {
            if !ext.is_empty() {
                validate_resume_key(&ext)?;
                return Ok(ext);
            }
            let stem = source_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !stem.is_empty() {
                validate_resume_key(&stem)?;
                return Ok(stem);
            }
            Ok(source_file.to_string_lossy().to_string())
        }
        _ => Err("Unknown agent family.".to_string()),
    }
}

fn build_plan(row: &SessionRow) -> Result<ResumePlan, String> {
    if !RESUME_FAMILIES.contains(&row.family.as_str()) {
        return Err("Unknown agent family.".to_string());
    }
    let (_uri_family, source_file, fragment) = split_source_uri(&row.source_uri)?;
    // Remote sessions are read from a local mirror, but identifiers and
    // environment overrides must use the path on the host.
    let key_file = match &row.host {
        Some(_) => crate::remote::from_mirror(&source_file)
            .map(|(_, remote)| PathBuf::from(remote))
            .unwrap_or_else(|| source_file.clone()),
        None => source_file.clone(),
    };
    let resume_key = derive_resume_key(
        &row.family,
        &row.external_id,
        &key_file,
        fragment.as_deref(),
    )?;

    // Binary + args per family (verbatim flag dialects).
    let (binary, args, profile): (String, Vec<String>, Option<String>) = match row.family.as_str() {
        "cline" => (
            "cline".into(),
            vec!["--id".into(), resume_key.clone(), "--tui".into()],
            None,
        ),
        "codex" => (
            "codex".into(),
            vec!["resume".into(), resume_key.clone()],
            None,
        ),
        "claude" => (
            "claude".into(),
            vec!["--resume".into(), resume_key.clone()],
            None,
        ),
        "opencode" => (
            "opencode".into(),
            vec!["--session".into(), resume_key.clone()],
            None,
        ),
        "hermes" => {
            // Profile comes from .../profiles/<name>/state.db
            let norm = normalized_slashes(&key_file);
            let mut prof = None;
            if let Some(after) = norm.split("/profiles/").nth(1) {
                let name = after.split('/').next().unwrap_or("").trim();
                if !name.is_empty() && name != "state.db" {
                    prof = Some(name.to_string());
                }
            }
            if prof.is_none() && key_file.file_name().and_then(|s| s.to_str()) == Some("state.db") {
                prof = Some("default".into());
            }
            let a = if let Some(p) = prof.clone() {
                vec!["-p".into(), p, "--resume".into(), resume_key.clone()]
            } else {
                vec!["--resume".into(), resume_key.clone()]
            };
            ("hermes".into(), a, prof)
        }
        "pi" => (
            "pi".into(),
            vec!["--session".into(), resume_key.clone()],
            None,
        ),
        "commandcode" => (
            "commandcode".into(),
            vec!["--session".into(), resume_key.clone()],
            None,
        ),
        "muse" => (
            "muse".into(),
            vec!["resume".into(), resume_key.clone()],
            None,
        ),
        "antigravity" => (
            "agy".into(),
            vec!["--conversation".into(), resume_key.clone()],
            None,
        ),
        _ => return Err("Unknown agent family.".to_string()),
    };

    if let Some(host) = &row.host {
        return Ok(remote_plan(
            row,
            host,
            &key_file,
            binary,
            args,
            profile,
            source_file,
            resume_key,
        ));
    }

    // Working directory: recorded cwd → workspace → HOME.
    let mut warnings = Vec::new();
    let mut cwd = PathBuf::from(row.cwd.trim());
    if cwd.as_os_str().is_empty() {
        cwd = PathBuf::from(row.project.trim());
    }
    let mut cwd_fallback = false;
    if cwd.as_os_str().is_empty() || !cwd.is_dir() {
        if !row.cwd.trim().is_empty() || !row.project.trim().is_empty() {
            warnings.push("Workspace folder is unavailable; using home directory.".to_string());
        }
        cwd = home_dir();
        cwd_fallback = true;
    }

    // Env overrides so the CLI looks at the same root that was imported.
    let mut env: HashMap<String, String> = HashMap::new();
    let norm = normalized_slashes(&source_file);
    match row.family.as_str() {
        "codex" => {
            if let Some(root) = prefix_before(&norm, "/sessions/") {
                let def = normalized_slashes(&default_codex_home());
                if !root.is_empty() && root != def {
                    env.insert("CODEX_HOME".into(), root.into());
                }
            }
        }
        "claude" => {
            if let Some(root) = prefix_before(&norm, "/projects/") {
                let def = normalized_slashes(&default_claude_dir());
                if !root.is_empty() && root != def {
                    env.insert("CLAUDE_CONFIG_DIR".into(), root.into());
                }
            }
        }
        "pi" => {
            if let Some(root) = prefix_before(&norm, "/sessions/") {
                let def = normalized_slashes(&default_pi_dir());
                if !root.is_empty() && root != def {
                    env.insert("PI_CODING_AGENT_DIR".into(), root.into());
                }
            }
        }
        "commandcode" => {
            if let Some(root) = prefix_before(&norm, "/projects/") {
                let def = normalized_slashes(&default_commandcode_home());
                if !root.is_empty() && root != def {
                    env.insert("COMMANDCODE_HOME".into(), root.into());
                }
            }
        }
        "muse" => {
            if let Some(root) = prefix_before(&norm, "/muse/sessions/") {
                let def = normalized_slashes(&default_xdg_data());
                if !root.is_empty() && root != def {
                    env.insert("XDG_DATA_HOME".into(), root.into());
                }
            }
        }
        "antigravity" => {
            if let Some(root) = prefix_before(&norm, "/brain/") {
                if !root.is_empty() {
                    env.insert("ANTIGRAVITY_HOME".into(), root.into());
                }
            }
        }
        "opencode" => {
            // <xdg>/opencode/opencode.db
            if let Some(root) = prefix_before(&norm, "/opencode/opencode.db") {
                let def = normalized_slashes(&default_xdg_data());
                if !root.is_empty() && root != def {
                    env.insert("XDG_DATA_HOME".into(), root.into());
                }
            }
        }
        "cline" => {
            env.extend(crate::importer::cline_resume_environment(&source_file));
        }
        "hermes" => {
            // Pin the database's home even for the default profile: the CLI's
            // active profile or inherited HERMES_HOME may point elsewhere.
            if source_file.file_name().and_then(|s| s.to_str()) == Some("state.db") {
                if let Some(parent) = source_file.parent() {
                    env.insert("HERMES_HOME".into(), normalized_slashes(parent));
                }
            } else if let Some(root) = prefix_before(&norm, "/profiles/") {
                if !root.is_empty() {
                    env.insert("HERMES_HOME".into(), root.into());
                }
            }
        }
        _ => {}
    }

    let display = display_command(&cwd, &binary, &args);
    if cwd_fallback {
        warnings.push("Launching outside the original workspace.".to_string());
    }

    Ok(ResumePlan {
        binary,
        args,
        cwd,
        cwd_fallback,
        env,
        display,
        warnings,
        source_file,
        resume_key,
        profile: profile.clone(),
        remote: None,
    })
}

/// Plan for a session mirrored from `host`: a local terminal runs
/// `ssh -t host …`, which resumes the agent in the recorded remote cwd.
#[allow(clippy::too_many_arguments)]
fn remote_plan(
    row: &SessionRow,
    host: &str,
    remote_file: &Path,
    agent: String,
    agent_args: Vec<String>,
    profile: Option<String>,
    source_file: PathBuf,
    resume_key: String,
) -> ResumePlan {
    // Workspace paths of remote sessions are stored as `host:/path`.
    let project = row.project.trim();
    let project = project.strip_prefix(&format!("{host}:")).unwrap_or(project);
    let cwd = Some(row.cwd.trim())
        .filter(|c| !c.is_empty())
        .unwrap_or(project);
    let mut warnings = Vec::new();
    if cwd.is_empty() {
        warnings.push(
            "Workspace folder is unknown; the session opens in the home directory on the host."
                .to_string(),
        );
    }
    // Only Hermes is pinned explicitly: for the other agents, the host's own
    // environment already points at the store the session was mirrored from.
    let mut env = HashMap::new();
    if row.family == "hermes" {
        let home = if remote_file.file_name().and_then(|s| s.to_str()) == Some("state.db") {
            remote_file.parent().map(normalized_slashes)
        } else {
            prefix_before(&normalized_slashes(remote_file), "/profiles/").map(str::to_string)
        };
        if let Some(home) = home.filter(|h| !h.is_empty()) {
            env.insert("HERMES_HOME".to_string(), home);
        }
    }
    let mut env_list: Vec<(String, String)> = env.clone().into_iter().collect();
    env_list.sort();
    let remote_cwd = if cwd.is_empty() { "~" } else { cwd };
    let command = crate::remote::resume_command(remote_cwd, &env_list, &agent, &agent_args);
    // The copyable form stays readable; the launched form also handles csh
    // login shells and keeps the terminal open afterwards.
    let mut readable = String::new();
    for (key, value) in &env_list {
        readable.push_str(&format!("{key}={} ", posix_quote(value)));
    }
    readable.push_str(&posix_quote(&agent));
    for arg in &agent_args {
        readable.push(' ');
        readable.push_str(&posix_quote(arg));
    }
    let display = format!(
        "ssh -t {} {}",
        posix_quote(host),
        posix_quote(&format!("cd {} && {readable}", posix_quote(remote_cwd)))
    );
    ResumePlan {
        binary: "ssh".into(),
        args: vec!["-t".into(), host.to_string(), command],
        cwd: home_dir(),
        cwd_fallback: cwd.is_empty(),
        env: HashMap::new(),
        display,
        warnings,
        source_file,
        resume_key,
        profile,
        remote: Some(RemoteTarget {
            host: host.to_string(),
            agent,
        }),
    }
}

/// Local executable for a plan and the binary name to show: the agent CLI,
/// or `ssh` for remote plans.
fn plan_binary(plan: &ResumePlan, family: &str) -> (Option<PathBuf>, String) {
    if plan.remote.is_some() {
        return (find_cli_binary(&["ssh"]), "ssh".to_string());
    }
    let cli_path = find_cli_binary(cli_candidates(family));
    let binary_used = cli_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .filter(|_| matches!(family, "commandcode" | "antigravity"))
        .unwrap_or(&plan.binary)
        .to_string();
    (cli_path, binary_used)
}

// ---------------------------------------------------------------------------
// Preflight + spawn
// ---------------------------------------------------------------------------

fn preflight(db_path: &Path, id: i64, terminal: Option<&str>) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let row = load_session_row(&conn, id)?;
    let plan = build_plan(&row)?;

    let (cli_path, binary_used) = plan_binary(&plan, &row.family);
    let cli_found = cli_path.is_some();
    let mut warnings = plan.warnings.clone();
    let mut version: Option<String> = None;
    let mut cli_error: Option<String> = None;
    if let Some(remote) = &plan.remote {
        if !cli_found {
            warnings.push("`ssh` was not found on this computer.".to_string());
        }
        match crate::remote::remote_command_exists(&remote.host, &remote.agent) {
            Ok(true) => {}
            Ok(false) => warnings.push(format!(
                "`{}` was not found on {}.",
                remote.agent, remote.host
            )),
            Err(error) => warnings.push(format!("Could not check {}: {error}", remote.host)),
        }
    } else if let Some(ref p) = cli_path {
        let (v, e) = cli_check(p);
        version = v;
        cli_error = e;
        if let Some(ref err) = cli_error {
            warnings.push(format!("Agent CLI failed to run: {err}"));
        }
        if row.family == "muse" {
            let v = version.clone().unwrap_or_default().to_lowercase();
            if !v.contains("muse") {
                warnings.push("The `muse` command found may not be Muse Code.".to_string());
            }
        }
    }
    let source_exists = plan.source_file.is_file();
    if !source_exists {
        warnings.push("Original session file is missing; resume may fail.".to_string());
    }
    // A remote cwd cannot be checked locally; the launcher falls back on the host.
    let cwd_exists = plan.remote.is_some() || plan.cwd.is_dir();
    let (terminal_id, terminal_fallback) = resolve_terminal(terminal);
    if terminal_fallback {
        if let Some(ref tid) = terminal_id {
            warnings.push(format!(
                "Saved terminal is not installed; using {} instead.",
                terminal_label(tid)
            ));
        }
    }
    let terminal_label = terminal_id.as_deref().map(terminal_label);

    // Rebuild display with the actual binary that will run.
    let args = plan.args.clone();
    let display = if plan.remote.is_some() {
        plan.display.clone()
    } else {
        display_command(&plan.cwd, &binary_used, &args)
    };

    Ok(json!({
        "ok": true,
        "id": id,
        "family": row.family,
        "product": if row.product.is_empty() { Value::Null } else { Value::String(row.product) },
        "binary": binary_used,
        "args": args,
        "cwd": plan.cwd.to_string_lossy(),
        "host": plan.remote.as_ref().map(|r| r.host.clone()),
        "cwdExists": cwd_exists,
        "cwdFallback": plan.cwd_fallback,
        "cliFound": cli_found,
        "cliPath": cli_path.map(|p| p.to_string_lossy().to_string()),
        "cliVersion": version,
        "cliWorks": cli_error.is_none(),
        "cliError": cli_error,
        "sourceExists": source_exists,
        "terminal": terminal_label,
        "terminalId": terminal_id,
        "terminalFallback": terminal_fallback,
        "terminalPreference": load_terminal_pref(),
        "displayCommand": display,
        "env": plan.env,
        "resumeKey": plan.resume_key,
        "profile": plan.profile,
        "warnings": warnings,
    }))
}

/// AppleScript double-quoted string escaping (AppleScript path only).
#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Shell launcher body shared by every script-file launcher: cd, env, CLI,
/// then a login shell so the window stays open after the agent exits.
#[cfg(target_os = "macos")]
fn launcher_script_text(plan: &ResumePlan, cli_path: &Path) -> String {
    let mut script = String::from("#!/bin/sh\n");
    // `open` strips our environment (LaunchServices), and GUI apps inherit a
    // skeletal PATH, so pin the full user PATH here. Without it, shims like
    // nvm's `node` fail with `env: node: No such file or directory`.
    script.push_str(&format!("export PATH={}\n", posix_quote(&augmented_path())));
    script.push_str(&format!(
        "cd {} || exit 1\n",
        posix_quote(&plan.cwd.to_string_lossy())
    ));
    // Deterministic order for stable output.
    let mut env: Vec<(&String, &String)> = plan.env.iter().collect();
    env.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in env {
        script.push_str(&format!("export {}={}\n", posix_quote(k), posix_quote(v)));
    }
    script.push_str(&posix_quote(&cli_path.to_string_lossy()));
    for a in &plan.args {
        script.push(' ');
        script.push_str(&posix_quote(a));
    }
    // Keep the window open after the agent exits so errors stay visible.
    script.push_str("\nexec \"${SHELL:-/bin/sh}\"\n");
    script
}

/// Write a launcher script to the temp dir (`.command` for Terminal.app,
/// `.sh` for wrappers run by other terminals).
#[cfg(target_os = "macos")]
fn write_launcher_file(plan: &ResumePlan, cli_path: &Path, ext: &str) -> Result<PathBuf, String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let script = launcher_script_text(plan, cli_path);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file = std::env::temp_dir().join(format!(
        "{}-resume-{}-{}.{}",
        crate::brand::SLUG,
        std::process::id(),
        nanos,
        ext
    ));
    std::fs::write(&file, script).map_err(|e| format!("Could not write launcher script: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700));
    }
    // Best-effort cleanup of stale launchers (older than 7 days).
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !name.starts_with(&format!("{}-resume-", crate::brand::SLUG)) {
                continue;
            }
            if !(name.ends_with(".command") || name.ends_with(".sh")) {
                continue;
            }
            if path == file {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|d| d.as_secs() > 7 * 24 * 3600);
            if old {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(file)
}

/// macOS `.command` launcher: opening a script file with `open` runs it in
/// Terminal.app without Apple Events, so no Automation permission prompt
/// (osascript → Terminal fails with error -1743 when denied).
#[cfg(target_os = "macos")]
fn write_macos_command_file(plan: &ResumePlan, cli_path: &Path) -> Result<PathBuf, String> {
    write_launcher_file(plan, cli_path, "command")
}

/// iTerm2 via AppleScript (`create window ... command`). iTerm2 exposes no
/// permission-free CLI for running a command, so an explicit user choice of
/// iTerm2 is the one case that needs the one-time Automation grant. The
/// wrapper path is the only interpolated value (single-quoted by construction
/// of the temp path plus AppleScript string escaping).
#[cfg(target_os = "macos")]
fn spawn_iterm(plan: &ResumePlan, cli_path: &Path) -> Result<String, String> {
    if mac_app_dir("iTerm.app").is_none() {
        return Err("iTerm2 is not installed.".to_string());
    }
    let wrapper = write_launcher_file(plan, cli_path, "sh")?;
    let quoted = applescript_quote(&wrapper.to_string_lossy());
    let script = format!(
        "tell application \"iTerm\" to create window with default profile command {quoted}"
    );
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Could not open iTerm2: {e}"))?;
    if out.status.success() {
        return Ok("iTerm2".to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    if err.contains("-1743") || err.contains("Not authorised") || err.contains("not authorized") {
        return Err("iTerm2 blocked automation. Allow it in System Settings → Privacy & Security → Automation, then try again.".to_string());
    }
    let short: String = err.trim().chars().take(200).collect();
    if short.is_empty() {
        return Err("Could not open iTerm2.".to_string());
    }
    Err(format!("Could not open iTerm2: {short}"))
}

fn spawn_plan(plan: &ResumePlan, cli_path: &Path, terminal_id: &str) -> Result<String, String> {
    // Spawn uses the resolved absolute CLI path; display keeps the binary name.

    #[cfg(target_os = "macos")]
    {
        match terminal_id {
            "iterm" => return spawn_iterm(plan, cli_path),
            "wezterm" => {
                let bin = mac_bundle_cli("WezTerm.app", "Contents/MacOS/wezterm", "wezterm")
                    .ok_or_else(|| "WezTerm is not installed.".to_string())?;
                let mut cmd = Command::new(&bin);
                cmd.arg("start")
                    .arg("--cwd")
                    .arg(&plan.cwd)
                    .arg("--")
                    .arg(cli_path);
                for a in &plan.args {
                    cmd.arg(a);
                }
                for (k, v) in &plan.env {
                    cmd.env(k, v);
                }
                cmd.env("PATH", augmented_path());
                cmd.spawn()
                    .map_err(|e| format!("Could not open WezTerm: {e}"))?;
                return Ok("WezTerm".to_string());
            }
            "kitty" => {
                let bin = mac_bundle_cli("kitty.app", "Contents/MacOS/kitty", "kitty")
                    .ok_or_else(|| "Kitty is not installed.".to_string())?;
                let mut cmd = Command::new(&bin);
                cmd.arg("--directory").arg(&plan.cwd).arg(cli_path);
                for a in &plan.args {
                    cmd.arg(a);
                }
                for (k, v) in &plan.env {
                    cmd.env(k, v);
                }
                cmd.env("PATH", augmented_path());
                cmd.spawn()
                    .map_err(|e| format!("Could not open Kitty: {e}"))?;
                return Ok("Kitty".to_string());
            }
            "alacritty" => {
                let bin = mac_bundle_cli("Alacritty.app", "Contents/MacOS/alacritty", "alacritty")
                    .ok_or_else(|| "Alacritty is not installed.".to_string())?;
                let mut cmd = Command::new(&bin);
                cmd.arg("--working-directory")
                    .arg(&plan.cwd)
                    .arg("-e")
                    .arg(cli_path);
                for a in &plan.args {
                    cmd.arg(a);
                }
                for (k, v) in &plan.env {
                    cmd.env(k, v);
                }
                cmd.env("PATH", augmented_path());
                cmd.spawn()
                    .map_err(|e| format!("Could not open Alacritty: {e}"))?;
                return Ok("Alacritty".to_string());
            }
            "ghostty" => {
                let app = mac_app_dir("Ghostty.app")
                    .ok_or_else(|| "Ghostty is not installed.".to_string())?;
                // `open` cannot pass env, so env rides in the wrapper script.
                let wrapper = write_launcher_file(plan, cli_path, "sh")?;
                let open =
                    find_cli_binary(&["open"]).unwrap_or_else(|| PathBuf::from("/usr/bin/open"));
                Command::new(&open)
                    .arg("-n")
                    .arg("-a")
                    .arg(&app)
                    .arg("--args")
                    .arg(format!(
                        "--working-directory={}",
                        plan.cwd.to_string_lossy()
                    ))
                    .arg("-e")
                    .arg("/bin/sh")
                    .arg(&wrapper)
                    .spawn()
                    .map_err(|e| format!("Could not open Ghostty: {e}"))?;
                return Ok("Ghostty".to_string());
            }
            _ => {
                // No AppleScript: a `.command` script opened with `open` needs no
                // Automation permission (fixes -1743 "Not authorised to send Apple
                // events to Terminal").
                let file = write_macos_command_file(plan, cli_path)?;
                let open =
                    find_cli_binary(&["open"]).unwrap_or_else(|| PathBuf::from("/usr/bin/open"));
                Command::new(&open)
                    .arg(&file)
                    .spawn()
                    .map_err(|e| format!("Could not open Terminal: {e}"))?;
                return Ok("Terminal".to_string());
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Use the same batch wrapper for native executables and npm shims.
        // It avoids interpolating session arguments into cmd /C start.
        let wrapper = write_windows_launcher(plan, cli_path)?;
        if terminal_id == "wt" {
            let wt = find_cli_binary(&["wt"])
                .ok_or_else(|| "Windows Terminal is not installed.".to_string())?;
            let mut cmd = Command::new(&wt);
            cmd.arg("-d").arg(&plan.cwd);
            cmd.arg("cmd.exe")
                .arg("/D")
                .arg("/V:OFF")
                .arg("/K")
                .arg(&wrapper);
            for (k, v) in &plan.env {
                cmd.env(k, v);
            }
            cmd.env("PATH", augmented_path());
            cmd.spawn()
                .map_err(|e| format!("Could not open Windows Terminal: {e}"))?;
            return Ok("Windows Terminal".to_string());
        }
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/D")
            .arg("/V:OFF")
            .arg("/K")
            .arg(&wrapper)
            .current_dir(&plan.cwd)
            .creation_flags(0x00000010); // CREATE_NEW_CONSOLE
        for (k, v) in &plan.env {
            cmd.env(k, v);
        }
        cmd.env("PATH", augmented_path());
        cmd.spawn()
            .map_err(|e| format!("Could not open Command Prompt: {e}"))?;
        return Ok("Command Prompt".to_string());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if !is_terminal_available(terminal_id) {
            return Err("Selected terminal is not installed.".to_string());
        }
        let term = if terminal_id.contains('/') {
            PathBuf::from(terminal_id)
        } else {
            find_cli_binary(&[terminal_id])
                .ok_or_else(|| "Selected terminal is not installed.".to_string())?
        };
        let label = terminal_label(terminal_id);
        let bin = cli_path.to_string_lossy().to_string();
        let mut inner = format!(
            "cd {} && exec {}",
            posix_quote(&plan.cwd.to_string_lossy()),
            posix_quote(&bin)
        );
        for a in &plan.args {
            inner.push(' ');
            inner.push_str(&posix_quote(a));
        }
        // Keep the window open after exit for error inspection.
        let script = format!("{inner}; exec \"${{SHELL:-/bin/bash}}\"");
        let mut cmd = Command::new(&term);
        // Labels are presentation strings (e.g. "GNOME Terminal"). Match the
        // executable name, including when $TERMINAL is an absolute path.
        match term.file_name().and_then(|s| s.to_str()).unwrap_or("") {
            "gnome-terminal" => {
                cmd.arg("--").arg("bash").arg("-c").arg(&script);
            }
            "wezterm" => {
                cmd.arg("start")
                    .arg("--cwd")
                    .arg(&plan.cwd)
                    .arg("--")
                    .arg("bash")
                    .arg("-c")
                    .arg(&script);
            }
            "kitty" | "foot" => {
                cmd.arg("--").arg("bash").arg("-c").arg(&script);
            }
            "xfce4-terminal" => {
                cmd.arg("--execute").arg("bash").arg("-c").arg(&script);
            }
            _ => {
                cmd.arg("-e").arg("bash").arg("-c").arg(&script);
            }
        }
        cmd.current_dir(&plan.cwd);
        for (k, v) in &plan.env {
            cmd.env(k, v);
        }
        cmd.env("PATH", augmented_path());
        cmd.spawn()
            .map_err(|e| format!("Could not open {label}: {e}"))?;
        Ok(label)
    }
}

fn resume_inner(db_path: &Path, id: i64, terminal: Option<&str>) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let row = load_session_row(&conn, id)?;
    let mut plan = build_plan(&row)?;
    drop(conn);

    let (cli_path, binary_used) = plan_binary(&plan, &row.family);
    if plan.remote.is_none() {
        plan.display = display_command(&plan.cwd, &binary_used, &plan.args);
    }

    let Some(cli) = cli_path else {
        return Ok(json!({
            "ok": false,
            "reason": "CLI_NOT_FOUND",
            "message": format!("`{}` was not found on PATH.", plan.binary),
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "displayCommand": plan.display,
            "env": plan.env,
            "warnings": plan.warnings,
        }));
    };

    if plan.remote.is_none() && !plan.source_file.is_file() {
        return Ok(json!({
            "ok": false,
            "reason": "SESSION_FILE_MISSING",
            "message": "Original session file is missing; the agent may report it as not found.",
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "displayCommand": plan.display,
            "env": plan.env,
            "warnings": plan.warnings,
        }));
    }

    // The CLI exists but may not run (deleted conda env, missing node, …).
    // Fail here with the underlying error instead of opening a dead terminal.
    let cli_error = if plan.remote.is_none() {
        cli_check(&cli).1
    } else {
        None
    };
    if let Some(err) = cli_error {
        return Ok(json!({
            "ok": false,
            "reason": "CLI_BROKEN",
            "message": format!("`{binary_used}` failed to run: {err}"),
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "displayCommand": plan.display,
            "env": plan.env,
            "warnings": plan.warnings,
        }));
    }

    let (terminal_id_opt, terminal_fallback) = resolve_terminal(terminal);
    let Some(terminal_id) = terminal_id_opt else {
        return Ok(json!({
            "ok": false,
            "reason": "NO_TERMINAL",
            "message": "No terminal emulator found to launch.",
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "displayCommand": plan.display,
            "env": plan.env,
            "warnings": plan.warnings,
        }));
    };
    if terminal_fallback {
        plan.warnings.push(format!(
            "Saved terminal is not installed; using {} instead.",
            terminal_label(&terminal_id)
        ));
    }

    match spawn_plan(&plan, &cli, terminal_id.as_str()) {
        Ok(terminal) => Ok(json!({
            "ok": true,
            "launched": true,
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "terminal": terminal,
            "terminalId": terminal_id,
            "displayCommand": plan.display,
        })),
        Err(message) => Ok(json!({
            "ok": false,
            "reason": "SPAWN_FAILED",
            "message": message,
            "family": row.family,
            "binary": binary_used,
            "args": plan.args,
            "cwd": plan.cwd.to_string_lossy(),
            "displayCommand": plan.display,
            "env": plan.env,
            "warnings": plan.warnings,
        })),
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Side-effect free preflight for the Resume dialog. `terminal` is an
/// optional override; otherwise the saved preference, then the default.
#[tauri::command]
pub async fn get_resume_info(
    state: State<'_, AppState>,
    id: String,
    terminal: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let session_id: i64 = id
        .trim()
        .parse()
        .map_err(|_| "Session not found".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        preflight(&db_path, session_id, terminal.as_deref())
    })
    .await
    .map_err(|e| format!("get_resume_info failed: {e}"))?
}

/// Launch the native CLI in a visible terminal.
#[tauri::command]
pub async fn resume_session(
    state: State<'_, AppState>,
    id: String,
    terminal: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let session_id: i64 = id
        .trim()
        .parse()
        .map_err(|_| "Session not found".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        resume_inner(&db_path, session_id, terminal.as_deref())
    })
    .await
    .map_err(|e| format!("resume_session failed: {e}"))?
}

/// Terminal choices with live availability for the picker.
#[tauri::command]
pub fn list_terminals() -> Result<Value, String> {
    Ok(json!({
        "terminals": available_terminals(),
        "defaultId": default_terminal_id(),
        "saved": load_terminal_pref(),
    }))
}

/// Saved terminal preference (`{ terminal: String|null }`).
#[tauri::command]
pub fn get_terminal_preference() -> Result<Value, String> {
    Ok(json!({ "terminal": load_terminal_pref() }))
}

/// Persist the terminal preference for future invocations.
#[tauri::command]
pub fn set_terminal_preference(terminal: String) -> Result<Value, String> {
    save_terminal_pref(&terminal)?;
    Ok(json!({ "ok": true, "terminal": terminal.trim() }))
}

// ---------------------------------------------------------------------------
// Open the session's workspace in VS Code (Remote-SSH for remote sessions)
// ---------------------------------------------------------------------------

/// Editor CLIs that understand `--folder-uri vscode-remote://…`.
const EDITOR_CANDIDATES: &[&str] = &["code", "code-insiders", "codium", "cursor", "windsurf"];

fn open_in_editor_inner(db_path: &Path, id: i64) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let row = load_session_row(&conn, id)?;
    drop(conn);
    let editor = find_cli_binary(EDITOR_CANDIDATES)
        .ok_or("VS Code's `code` command was not found. In VS Code, run “Shell Command: Install 'code' command in PATH”.")?;
    let project = row.project.trim();
    let mut cmd = Command::new(&editor);
    cmd.env("PATH", augmented_path());
    let target = match &row.host {
        Some(host) => {
            let project = project.strip_prefix(&format!("{host}:")).unwrap_or(project);
            let folder = Some(row.cwd.trim())
                .filter(|c| !c.is_empty())
                .unwrap_or(project);
            if folder.is_empty() {
                return Err("This session has no recorded workspace folder.".into());
            }
            let uri = crate::remote::vscode_folder_uri(host, folder);
            cmd.arg("--folder-uri").arg(&uri);
            uri
        }
        None => {
            let folder = Some(row.cwd.trim())
                .filter(|c| !c.is_empty())
                .unwrap_or(project);
            if folder.is_empty() || !Path::new(folder).is_dir() {
                return Err("The session's workspace folder is unavailable.".into());
            }
            cmd.arg(folder);
            folder.to_string()
        }
    };
    cmd.spawn()
        .map_err(|e| format!("Could not start VS Code: {e}"))?;
    Ok(json!({ "ok": true, "target": target }))
}

/// Open the session's workspace folder in VS Code; remote sessions open
/// through Remote-SSH on their host.
#[tauri::command]
pub async fn open_in_editor(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let session_id: i64 = id
        .trim()
        .parse()
        .map_err(|_| "Session not found".to_string())?;
    tauri::async_runtime::spawn_blocking(move || open_in_editor_inner(&db_path, session_id))
        .await
        .map_err(|e| format!("open_in_editor failed: {e}"))?
}
