use hession_core::{
    git::{self, GitLimits},
    projects::EvidenceCategory,
};
use std::{fs, path::Path, process::Command};
fn git(root: &Path, args: &[&str]) {
    let result = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "Synthetic Test"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    fs::write(dir.path().join("old name\n.rs"), "before").unwrap();
    git(dir.path(), &["add", "--", "old name\n.rs"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    dir
}
#[test]
fn recorded_commit_changes_dirty_deleted_and_unusual_filenames() {
    let dir = repo();
    git(dir.path(), &["mv", "--", "old name\n.rs", "new name\t.rs"]);
    git(dir.path(), &["commit", "-qm", "rename"]);
    fs::write(dir.path().join("new name\t.rs"), "dirty").unwrap();
    let evidence = git::inspect(dir.path(), GitLimits::default()).unwrap();
    assert!(evidence
        .changes
        .iter()
        .any(|c| c.category == EvidenceCategory::VerifiedCommitChange
            && c.status.starts_with('R')
            && c.previous_path.as_ref().unwrap().display == "old name\n.rs"));
    assert!(evidence
        .changes
        .iter()
        .any(|c| c.category == EvidenceCategory::ObservedWorktreeChange
            && c.status == "raw_bytes_differ"));
    fs::remove_file(dir.path().join("new name\t.rs")).unwrap();
    let evidence = git::inspect(dir.path(), GitLimits::default()).unwrap();
    assert!(evidence.changes.iter().any(|c| c.status == "missing"));
}
#[test]
fn detached_worktrees_have_distinct_identity_and_shared_common_dir() {
    let dir = repo();
    let detached = tempfile::tempdir().unwrap();
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "--detach",
            detached.path().to_str().unwrap(),
            "HEAD",
        ],
    );
    let first = git::inspect(dir.path(), GitLimits::default()).unwrap();
    let second = git::inspect(detached.path(), GitLimits::default()).unwrap();
    assert_eq!(first.common_dir, second.common_dir);
    assert_ne!(first.canonical_root, second.canonical_root);
    assert_ne!(first.worktree_git_dir, second.worktree_git_dir);
    assert!(second.detached);
    assert!(second.branch.is_none());
}
#[test]
fn malicious_filters_fsmonitor_and_external_diff_are_never_executed() {
    let dir = repo();
    let marker = dir.path().join("executed");
    let command = format!("touch {}", marker.display());
    git(dir.path(), &["config", "core.fsmonitor", &command]);
    git(dir.path(), &["config", "diff.external", &command]);
    git(dir.path(), &["config", "filter.evil.clean", &command]);
    git(dir.path(), &["config", "filter.evil.smudge", &command]);
    git(dir.path(), &["config", "diff.evil.textconv", &command]);
    fs::write(
        dir.path().join(".gitattributes"),
        "* filter=evil diff=evil\n",
    )
    .unwrap();
    fs::write(dir.path().join("old name\n.rs"), "changed").unwrap();
    let evidence = git::inspect(dir.path(), GitLimits::default()).unwrap();
    assert!(evidence
        .changes
        .iter()
        .any(|c| c.status == "raw_bytes_differ"));
    assert!(!marker.exists());
}
#[test]
fn path_output_and_commit_caps_and_missing_checkout_fail_explicitly() {
    let dir = repo();
    assert!(git::inspect(
        dir.path(),
        GitLimits {
            max_commits: 51,
            ..Default::default()
        }
    )
    .is_err());
    assert!(git::inspect(
        dir.path(),
        GitLimits {
            max_output_bytes: 1,
            ..Default::default()
        }
    )
    .is_err());
    assert!(git::inspect(&dir.path().join("deleted"), Default::default()).is_err());
    let evidence = git::inspect(
        dir.path(),
        GitLimits {
            max_commits: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(evidence.inspected_commits, 0);
    assert!(!evidence
        .changes
        .iter()
        .any(|c| c.category == EvidenceCategory::VerifiedCommitChange));
}
#[test]
fn symlink_files_are_never_followed_and_non_utf8_names_keep_identity() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let dir = repo();
    let name = std::ffi::OsString::from_vec(vec![b'n', 0xff, b'x']);
    // Git trees permit raw bytes even when the host filesystem (APFS) does not.
    let blob = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "HEAD:old name\n.rs"])
        .output()
        .unwrap();
    assert!(blob.status.success());
    let blob = String::from_utf8(blob.stdout).unwrap();
    let staged = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(dir.path())
        .args([
            "update-index",
            "--add",
            "--cacheinfo",
            "100644",
            blob.trim(),
        ])
        .arg(&name)
        .output()
        .unwrap();
    assert!(
        staged.status.success(),
        "{}",
        String::from_utf8_lossy(&staged.stderr)
    );
    git(dir.path(), &["commit", "-qm", "bytes"]);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("private"), "private").unwrap();
    fs::remove_file(dir.path().join("old name\n.rs")).unwrap();
    symlink(
        outside.path().join("private"),
        dir.path().join("old name\n.rs"),
    )
    .unwrap();
    let evidence = git::inspect(dir.path(), Default::default()).unwrap();
    assert!(evidence.skipped_paths >= 1);
    assert!(!evidence.worktree_scan_complete);
    assert!(evidence
        .changes
        .iter()
        .any(|c| c.path.bytes_hex == "6eff78"));
}

#[test]
fn promisor_repositories_never_start_remote_helpers() {
    let dir = repo();
    let marker = dir.path().join("remote-helper-executed");
    let helper = dir.path().join("helper");
    fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.url",
            &format!("ext::{}", helper.display()),
        ],
    );
    git(dir.path(), &["config", "remote.origin.promisor", "true"]);
    // Remove a real loose blob so any unsafe attempt to read it would require lazy fetching.
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "HEAD:old name\n.rs"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let blob = String::from_utf8(output.stdout).unwrap();
    let blob = blob.trim();
    fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    git(dir.path(), &["config", "protocol.ext.allow", "always"]);
    assert!(matches!(
        git::inspect(dir.path(), Default::default()),
        Err(git::GitError::Unavailable)
    ));
    assert!(!marker.exists());
}

#[test]
fn tracked_path_cap_marks_scan_incomplete_and_never_recurses_untracked_directories() {
    let dir = repo();
    for name in ["a", "b"] {
        fs::write(dir.path().join(name), "unchanged").unwrap();
    }
    git(dir.path(), &["add", "--all"]);
    git(dir.path(), &["commit", "-qm", "more tracked files"]);
    fs::create_dir(dir.path().join("untracked")).unwrap();
    fs::write(dir.path().join("untracked/private"), "not collected").unwrap();
    let evidence = git::inspect(
        dir.path(),
        GitLimits {
            max_commits: 0,
            max_paths: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(evidence.inspected_tracked_paths, 1);
    assert!(evidence.worktree_scan_truncated);
    assert!(!evidence.worktree_scan_complete);
    assert!(evidence
        .changes
        .iter()
        .all(|c| !c.path.display.starts_with("untracked")));
}

#[test]
fn staged_changes_are_observed_and_do_not_get_commit_or_agent_attribution() {
    let dir = repo();
    fs::write(dir.path().join("old name\n.rs"), "staged").unwrap();
    git(dir.path(), &["add", "--all"]);
    let evidence = git::inspect(
        dir.path(),
        GitLimits {
            max_commits: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(evidence.changes.len(), 1);
    let change = &evidence.changes[0];
    assert_eq!(change.category, EvidenceCategory::ObservedWorktreeChange);
    assert_eq!(change.status, "staged_M");
    assert!(change.commit_sha.is_none());
}

#[test]
fn inspection_deadline_is_shared_across_all_git_subprocesses() {
    let dir = repo();
    let start = std::time::Instant::now();
    assert!(matches!(
        git::inspect(
            dir.path(),
            GitLimits {
                timeout_ms: 1,
                ..Default::default()
            }
        ),
        Err(git::GitError::Timeout)
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}
