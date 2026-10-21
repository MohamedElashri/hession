//! Manual update check. Nothing here runs unless the user asks: there is no
//! check at startup, no background polling, and no automatic install.
//!
//! A check is one HTTPS request to GitHub's public releases API with a
//! generic User-Agent; it carries no app version, identifiers, or library
//! data. Updating runs the release's own install script in a visible
//! terminal, so the user sees (and approves) every step, and the download is
//! verified against the release's SHA256SUMS like a fresh install.

use std::time::Duration;

use serde_json::{json, Value};

/// The running app's version.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub requires a User-Agent; this one says nothing about the user.
const USER_AGENT: &str = "update-check";
const TIMEOUT: Duration = Duration::from_secs(15);

/// `(major, minor, patch, is_prerelease)` from `v1.2.3` or `1.2.3-beta.1`.
fn parse_version(raw: &str) -> Option<(u64, u64, u64, bool)> {
    let raw = raw.trim().trim_start_matches('v');
    let (core, pre) = match raw.split_once(['-', '+']) {
        Some((core, rest)) => (core, raw.as_bytes()[core.len()] == b'-' && !rest.is_empty()),
        None => (raw, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts
        .next()
        .is_none()
        .then_some((version.0, version.1, version.2, pre))
}

/// Whether `latest` is a newer release than `current` (a pre-release sorts
/// before its release).
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => (l.0, l.1, l.2, !l.3) > (c.0, c.1, c.2, !c.3),
        _ => false,
    }
}

/// Release tags reach a URL and a shell command, so only `vMAJOR.MINOR.PATCH`
/// is accepted: a tag from the network can never inject anything.
fn validate_tag(tag: &str) -> Result<&str, String> {
    let digits = tag.strip_prefix('v').unwrap_or("");
    let ok = digits.split('.').count() == 3
        && digits
            .split('.')
            .all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()));
    if ok {
        Ok(tag)
    } else {
        Err(format!("Unexpected release tag: {tag:?}"))
    }
}

/// Summary of the latest release for the About dialog.
fn update_info(release: &Value) -> Result<Value, String> {
    let tag = validate_tag(release["tag_name"].as_str().unwrap_or_default())?;
    let latest = tag.trim_start_matches('v');
    Ok(json!({
        "current": CURRENT_VERSION,
        "latest": latest,
        "tag": tag,
        "newer": is_newer(latest, CURRENT_VERSION),
        "notes": release["body"].as_str().unwrap_or_default(),
        "publishedAt": release["published_at"].as_str(),
        "url": release_page(tag),
    }))
}

fn release_page(tag: &str) -> String {
    format!(
        "https://github.com/{}/releases/tag/{tag}",
        crate::brand::REPOSITORY
    )
}

/// The command that installs `tag` with that release's own install script.
/// An AppImage install stays an AppImage install.
fn installer_command(tag: &str, appimage: bool) -> Result<String, String> {
    let tag = validate_tag(tag)?;
    Ok(format!(
        "curl -fsSL https://raw.githubusercontent.com/{repo}/{tag}/install.sh | sh -s -- --version {tag}{flag}",
        repo = crate::brand::REPOSITORY,
        flag = if appimage { " --appimage" } else { "" },
    ))
}

/// AppImages set `$APPIMAGE` to their own path while running.
fn running_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some()
}

#[tauri::command]
pub fn app_version() -> Value {
    json!({ "name": crate::brand::NAME, "version": CURRENT_VERSION })
}

/// Ask GitHub for the latest published release. Only called from the About
/// dialog's "Check for updates" button.
#[tauri::command]
pub async fn check_for_update() -> Result<Value, String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        crate::brand::REPOSITORY
    );
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .https_only(true)
        .build()
        .map_err(|e| format!("Could not create the HTTP client: {e}"))?;
    let response = client
        .get(&url)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err("No published release was found.".into()),
        403 | 429 => return Err("GitHub's rate limit was reached. Try again later.".into()),
        status => return Err(format!("GitHub answered with HTTP {status}.")),
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("Could not read GitHub's answer: {e}"))?;
    let release: Value =
        serde_json::from_str(&body).map_err(|e| format!("Could not read GitHub's answer: {e}"))?;
    update_info(&release)
}

/// The installer command for `tag`, for the user to copy.
#[tauri::command]
pub fn update_command(tag: String) -> Result<String, String> {
    installer_command(&tag, running_appimage())
}

/// Open a terminal running the installer for `tag`. Only called when the
/// user clicks "Update in terminal".
#[tauri::command]
pub async fn run_update(tag: String) -> Result<Value, String> {
    let command = installer_command(&tag, running_appimage())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::resume::run_in_terminal("sh", vec!["-c".into(), command])
    })
    .await
    .map_err(|e| format!("Could not start the update: {e}"))?
    .map(|terminal| json!({ "terminal": terminal }))
}

/// Open the release page for `tag` in the default browser.
#[tauri::command]
pub fn open_release_page(tag: String) -> Result<(), String> {
    let url = release_page(validate_tag(&tag)?);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn accepts_only_plain_release_tags() {
        assert!(validate_tag("v0.1.0").is_ok());
        for bad in [
            "0.1.0",
            "v0.1",
            "v0.1.0-beta",
            "v0.1.0; rm -rf ~",
            "v0.1.0 && x",
            "v1.2.3\n",
            "",
            "v..",
        ] {
            assert!(validate_tag(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn builds_the_installer_command_for_the_release() {
        let cmd = installer_command("v0.2.0", true).unwrap();
        assert!(
            cmd.ends_with("/v0.2.0/install.sh | sh -s -- --version v0.2.0 --appimage"),
            "{cmd}"
        );
        assert!(cmd.starts_with(&format!(
            "curl -fsSL https://raw.githubusercontent.com/{}/",
            crate::brand::REPOSITORY
        )));
        assert!(installer_command("v0.2.0;id", false).is_err());
    }

    #[test]
    fn summarizes_a_release() {
        let release =
            json!({"tag_name": "v99.0.0", "body": "Notes", "published_at": "2026-10-21T00:00:00Z"});
        let info = update_info(&release).unwrap();
        assert_eq!(info["latest"], "99.0.0");
        assert_eq!(info["newer"], true);
        assert!(info["url"]
            .as_str()
            .unwrap()
            .ends_with("/releases/tag/v99.0.0"));
        assert!(update_info(&json!({"tag_name": "$(id)"})).is_err());
    }
}
