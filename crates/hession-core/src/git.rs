//! Bounded local Git evidence. Fixed built-in commands only: no shell, network, hooks,
//! fsmonitor, external diff, textconv, or working-tree clean filters are invoked.
//! Working-tree comparison deliberately compares raw bytes with index blobs, ignoring
//! .gitattributes transformations; it never asks Git to refresh or filter a worktree file.
use crate::projects::{open_project_file, EvidenceCategory, ProjectError};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::{ffi::OsStringExt, process::CommandExt};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const HARD_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_COMMANDS: usize = 256;
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitLimits {
    pub max_commits: u32,
    pub max_paths: u32,
    pub max_output_bytes: usize,
    pub timeout_ms: u64,
}
impl Default for GitLimits {
    fn default() -> Self {
        Self {
            max_commits: 10,
            max_paths: 100,
            max_output_bytes: 1024 * 1024,
            timeout_ms: 5000,
        }
    }
}
impl GitLimits {
    fn validate(self) -> Result<Self, GitError> {
        if self.max_commits > 50
            || self.max_paths > 100
            || self.max_paths == 0
            || self.max_output_bytes == 0
            || self.max_output_bytes > HARD_OUTPUT
            || self.timeout_ms == 0
            || self.timeout_ms > 10000
        {
            Err(GitError::Limits)
        } else {
            Ok(self)
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{0}")]
    Project(#[from] ProjectError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid Git inspection limits")]
    Limits,
    #[error("Git inspection exceeded its deadline or command budget")]
    Timeout,
    #[error("Git inspection exceeded its output or path budget; request smaller bounds")]
    OutputLimit,
    #[error("Local Git metadata unavailable")]
    Unavailable,
    #[error("Unexpected Git metadata format")]
    Format,
}
/// Git filenames are byte strings; hex retains non-UTF8 identity while display stays readable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitPath {
    pub display: String,
    pub bytes_hex: String,
}
impl GitPath {
    fn new(bytes: &[u8]) -> Self {
        Self {
            display: String::from_utf8_lossy(bytes).into_owned(),
            bytes_hex: bytes.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitChange {
    pub category: EvidenceCategory,
    pub status: String,
    pub path: GitPath,
    pub previous_path: Option<GitPath>,
    pub commit_sha: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitInspection {
    pub canonical_root: PathBuf,
    pub common_dir: PathBuf,
    pub worktree_git_dir: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub observed_at_unix_ms: u64,
    pub changes: Vec<GitChange>,
    pub inspected_commits: u32,
    pub inspected_tracked_paths: u32,
    pub worktree_scan_truncated: bool,
    pub worktree_scan_complete: bool,
    pub skipped_paths: u32,
    pub worktree_comparison: String,
}
struct Runner {
    root: PathBuf,
    limits: GitLimits,
    deadline: Instant,
    remaining: usize,
    commands: usize,
}
impl Runner {
    fn run(&mut self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, GitError> {
        if Instant::now() >= self.deadline || self.commands >= MAX_COMMANDS {
            return Err(GitError::Timeout);
        }
        self.commands += 1;
        // Absolute system executable prevents repository PATH shadowing. env_clear removes injected
        // GIT_CONFIG_COUNT, LD_PRELOAD, GIT_EXTERNAL_DIFF and inherited credential/transport helpers.
        let mut command = Command::new("/usr/bin/git");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_ALLOW_PROTOCOL", "")
            .env("GIT_PROTOCOL_FROM_USER", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "--no-pager",
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.commitGraph=false",
                "-c",
                "log.showSignature=false",
                "-c",
                "diff.external=",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "protocol.allow=never",
            ])
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = command.spawn()?;
        if let Some(input) = input {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(input)?;
            }
        }
        let mut stdout = child.stdout.take().ok_or(GitError::Unavailable)?;
        let cap = self.remaining.min(self.limits.max_output_bytes);
        let overflow = Arc::new(AtomicBool::new(false));
        let flag = overflow.clone();
        let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut out = Vec::new();
            let mut buf = [0_u8; 8192];
            loop {
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                if out.len() + n > cap {
                    flag.store(true, Ordering::Release);
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
            Ok(out)
        });
        let status = loop {
            if overflow.load(Ordering::Acquire) || Instant::now() >= self.deadline {
                // SAFETY: the child owns this fresh process group; no shell or helper is started.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                let _ = reader.join();
                return Err(if overflow.load(Ordering::Acquire) {
                    GitError::OutputLimit
                } else {
                    GitError::Timeout
                });
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let out = reader.join().map_err(|_| GitError::Unavailable)??;
        if overflow.load(Ordering::Acquire) {
            return Err(GitError::OutputLimit);
        }
        self.remaining = self.remaining.saturating_sub(out.len());
        if !status.success() {
            return Err(GitError::Unavailable);
        }
        Ok(out)
    }
    fn text(&mut self, args: &[&str]) -> Result<String, GitError> {
        String::from_utf8(self.run(args, None)?)
            .map(|s| s.strip_suffix('\n').unwrap_or(&s).to_string())
            .map_err(|_| GitError::Format)
    }
}
fn oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn split_nul(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|b| *b == 0).filter(|p| !p.is_empty())
}
fn bytes_path(bytes: &[u8]) -> PathBuf {
    std::ffi::OsString::from_vec(bytes.to_vec()).into()
}

pub fn inspect(root: &Path, limits: GitLimits) -> Result<GitInspection, GitError> {
    let limits = limits.validate()?;
    if !root.is_absolute()
        || root.as_os_str().len() > crate::projects::MAX_PATH_BYTES
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(GitError::Unavailable);
    }
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(GitError::Unavailable);
    }
    let mut runner = Runner {
        root: root.clone(),
        limits,
        deadline: Instant::now() + Duration::from_millis(limits.timeout_ms),
        remaining: limits.max_output_bytes,
        commands: 0,
    };
    // Missing promisor objects must never trigger transport or remote helpers, including on
    // Git versions that predate GIT_NO_LAZY_FETCH. Refuse any partial-clone configuration.
    let config = runner.run(&["config", "--null", "--list", "--includes"], None)?;
    for field in split_nul(&config) {
        let key = field.split(|b| *b == b'\n').next().unwrap_or_default();
        let key = String::from_utf8_lossy(key).to_ascii_lowercase();
        if key == "extensions.partialclone"
            || (key.starts_with("remote.") && key.ends_with(".promisor"))
        {
            return Err(GitError::Unavailable);
        }
    }
    let canonical_root =
        PathBuf::from(runner.text(&["rev-parse", "--show-toplevel"])?).canonicalize()?;
    if !root.starts_with(&canonical_root) {
        return Err(GitError::Unavailable);
    }
    // Git discovery is allowed only inside the explicitly supplied existing workspace.
    // Descendant cwd may resolve to its parent repository, but no sibling clone is grouped.
    let worktree_git_dir = PathBuf::from(runner.text(&["rev-parse", "--absolute-git-dir"])?);
    let common = runner.text(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common_dir = PathBuf::from(common).canonicalize()?;
    let branch = match runner.text(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(value) => Some(value),
        Err(GitError::Unavailable) => None,
        Err(e) => return Err(e),
    };
    let head = match runner.text(&["rev-parse", "--verify", "HEAD"]) {
        Ok(value) if oid(&value) => Some(value),
        Ok(_) => return Err(GitError::Format),
        Err(GitError::Unavailable) => None,
        Err(e) => return Err(e),
    };
    runner.root = canonical_root.clone();
    let mut result = GitInspection {
        canonical_root: canonical_root.clone(),
        common_dir,
        worktree_git_dir,
        detached: branch.is_none() && head.is_some(),
        branch,
        head: head.clone(),
        observed_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        changes: Vec::new(),
        inspected_commits: 0,
        inspected_tracked_paths: 0,
        worktree_scan_truncated: false,
        worktree_scan_complete: false,
        skipped_paths: 0,
        worktree_comparison: concat!(
            "raw_bytes_against_index; attributes and clean filters are not applied; ",
            "untracked files are not enumerated"
        )
        .into(),
    };
    if let Some(head) = head {
        if limits.max_commits > 0 {
            let count = limits.max_commits.to_string();
            let commits = runner.text(&["rev-list", "--max-count", &count, &head, "--"])?;
            for commit in commits.lines() {
                if !oid(commit) {
                    return Err(GitError::Format);
                }
                let raw = runner.run(
                    &[
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--ignore-submodules=all",
                        "-r",
                        "-M",
                        "--raw",
                        "-z",
                        commit,
                        "--",
                    ],
                    None,
                )?;
                parse_changes(
                    &raw,
                    Some(commit),
                    limits.max_paths as usize,
                    &mut result.changes,
                )?;
                result.inspected_commits += 1;
            }
        }
        // Cached diff only examines stored index/object data, never working-tree clean filters.
        let cached = runner.run(
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
                "--raw",
                "-z",
                "-M",
                &head,
                "--",
            ],
            None,
        )?;
        parse_changes(
            &cached,
            None,
            limits.max_paths as usize,
            &mut result.changes,
        )?;
    }
    let files = runner.run(&["ls-files", "--stage", "-z"], None)?;
    for record in split_nul(&files) {
        if result.inspected_tracked_paths >= limits.max_paths {
            result.worktree_scan_truncated = true;
            break;
        }
        if Instant::now() >= runner.deadline {
            return Err(GitError::Timeout);
        }
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or(GitError::Format)?;
        let header = std::str::from_utf8(&record[..tab]).map_err(|_| GitError::Format)?;
        let mut parts = header.split(' ');
        let mode = parts.next().ok_or(GitError::Format)?;
        let blob = parts.next().ok_or(GitError::Format)?;
        let stage = parts.next().ok_or(GitError::Format)?;
        let name = &record[tab + 1..];
        if !oid(blob) {
            return Err(GitError::Format);
        }
        result.inspected_tracked_paths += 1;
        // Submodules/symlinks/conflicted entries are intentionally not followed.
        if !matches!(mode, "100644" | "100755") || stage != "0" {
            result.skipped_paths += 1;
            continue;
        }
        let file = open_project_file(&canonical_root, &bytes_path(name));
        let status = match file {
            Err(ProjectError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Some("missing"),
            Err(_) => {
                result.skipped_paths += 1;
                None
            }
            Ok(mut file) => {
                let size = file.metadata()?.len();
                if size > runner.remaining as u64 {
                    result.skipped_paths += 1;
                    continue;
                }
                let stored = runner.run(&["cat-file", "blob", blob], None)?;
                let mut actual = Vec::new();
                std::io::Read::by_ref(&mut file)
                    .take((runner.remaining.min(HARD_OUTPUT) + 1) as u64)
                    .read_to_end(&mut actual)?;
                if actual.len() > runner.remaining {
                    return Err(GitError::OutputLimit);
                }
                runner.remaining -= actual.len();
                if actual != stored {
                    Some("raw_bytes_differ")
                } else {
                    None
                }
            }
        };
        if let Some(status) = status {
            if result.changes.len() >= limits.max_paths as usize {
                return Err(GitError::OutputLimit);
            }
            result.changes.push(GitChange {
                category: EvidenceCategory::ObservedWorktreeChange,
                status: status.into(),
                path: GitPath::new(name),
                previous_path: None,
                commit_sha: None,
            });
        }
    }
    result.worktree_scan_complete = !result.worktree_scan_truncated && result.skipped_paths == 0;
    if serde_json::to_vec(&result)
        .map_err(|_| GitError::Format)?
        .len()
        > limits.max_output_bytes
    {
        return Err(GitError::OutputLimit);
    }
    Ok(result)
}
fn parse_changes(
    raw: &[u8],
    commit: Option<&str>,
    cap: usize,
    out: &mut Vec<GitChange>,
) -> Result<(), GitError> {
    let mut fields = split_nul(raw);
    while let Some(header) = fields.next() {
        let header = std::str::from_utf8(header).map_err(|_| GitError::Format)?;
        if !header.starts_with(':') {
            return Err(GitError::Format);
        }
        let status = header.split(' ').next_back().ok_or(GitError::Format)?;
        let first = fields.next().ok_or(GitError::Format)?;
        let (previous, path) = if status.starts_with(['R', 'C']) {
            (
                Some(GitPath::new(first)),
                fields.next().ok_or(GitError::Format)?,
            )
        } else {
            (None, first)
        };
        if out.len() >= cap {
            return Err(GitError::OutputLimit);
        }
        out.push(GitChange {
            category: if commit.is_some() {
                EvidenceCategory::VerifiedCommitChange
            } else {
                EvidenceCategory::ObservedWorktreeChange
            },
            status: if commit.is_none() {
                format!("staged_{status}")
            } else {
                status.into()
            },
            path: GitPath::new(path),
            previous_path: previous,
            commit_sha: commit.map(str::to_owned),
        });
    }
    Ok(())
}
