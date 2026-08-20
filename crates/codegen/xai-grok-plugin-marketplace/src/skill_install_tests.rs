use std::path::{Path, PathBuf};

use super::{InstallAction, InstallOptions, SkillInstallError, install, remove, update};
use crate::skill_lock::{SkillLock, SkillOrigin};
use crate::skill_registry::RegistryClient;
use crate::skill_scope::{SkillScopeKind, SkillScopeTarget};
use crate::skill_source::SkillSource;

/// A registry client pointed at a closed port, so any accidental network call
/// fails immediately instead of hanging or hitting the real registry.
fn offline_registry() -> RegistryClient {
    RegistryClient::with_base_url("http://127.0.0.1:1")
        .with_timeout(std::time::Duration::from_millis(50))
}

fn scope(dir: &Path) -> SkillScopeTarget {
    SkillScopeTarget::new(SkillScopeKind::User, dir.join(".grok"))
}

fn write_skill(root: &Path, name: &str, description: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .unwrap();
    dir
}

fn local(path: &Path) -> SkillSource {
    SkillSource::Local {
        path: path.to_path_buf(),
    }
}

#[tokio::test]
async fn installs_a_local_skill_into_the_scope() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship the app");
    let target = scope(home.path());

    let installed = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "deploy");
    assert_eq!(installed[0].description, "Ship the app");
    assert_eq!(installed[0].action, InstallAction::Installed);
    assert!(
        target
            .skills_dir()
            .join("deploy")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn install_records_provenance_in_the_lockfile() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship the app");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let lock = SkillLock::load(target.root());
    let entry = lock.get("deploy").expect("entry should be recorded");
    assert_eq!(entry.origin, SkillOrigin::Local);
    assert_eq!(entry.source, "./deploy");
    assert!(!entry.content_hash.is_empty());
}

#[tokio::test]
async fn installs_every_skill_in_a_multi_skill_source() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(src.path(), "alpha", "First");
    write_skill(src.path(), "beta", "Second");
    let target = scope(home.path());

    let installed = install(
        &local(src.path()),
        "./skills",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn only_option_narrows_a_multi_skill_source() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(src.path(), "alpha", "First");
    write_skill(src.path(), "beta", "Second");
    let target = scope(home.path());

    let installed = install(
        &local(src.path()),
        "./skills",
        &target,
        &InstallOptions {
            only: Some("beta".into()),
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "beta");
    assert!(!target.skills_dir().join("alpha").exists());
}

#[tokio::test]
async fn only_option_reports_a_missing_skill() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(src.path(), "alpha", "First");

    let error = install(
        &local(src.path()),
        "./skills",
        &scope(home.path()),
        &InstallOptions {
            only: Some("nope".into()),
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, SkillInstallError::SkillNotInSource { .. }));
}

#[tokio::test]
async fn nested_skill_directories_are_discovered() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(&src.path().join("team").join("infra"), "deploy", "Ship it");
    let target = scope(home.path());

    let installed = install(
        &local(src.path()),
        "./repo",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "deploy");
}

#[tokio::test]
async fn subdirectories_of_a_skill_are_copied_not_treated_as_skills() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    std::fs::create_dir_all(skill.join("rules")).unwrap();
    std::fs::write(skill.join("rules").join("a.md"), "rule body").unwrap();
    let target = scope(home.path());

    let installed = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].file_count, 2);
    let copied = target
        .skills_dir()
        .join("deploy")
        .join("rules")
        .join("a.md");
    assert_eq!(std::fs::read_to_string(copied).unwrap(), "rule body");
}

#[tokio::test]
async fn scan_skips_vendor_and_build_directories() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(&src.path().join("node_modules"), "vendored", "Nope");
    write_skill(src.path(), "real", "Yes");

    let installed = install(
        &local(src.path()),
        "./repo",
        &scope(home.path()),
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["real"]);
}

#[tokio::test]
async fn invalid_skill_md_files_are_skipped_rather_than_failing_the_install() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(src.path(), "good", "Valid");
    let broken = src.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("SKILL.md"), "no frontmatter here").unwrap();

    let installed = install(
        &local(src.path()),
        "./repo",
        &scope(home.path()),
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["good"]);
}

#[tokio::test]
async fn reports_when_a_source_contains_no_skills() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("README.md"), "nothing here").unwrap();

    let error = install(
        &local(src.path()),
        "./repo",
        &scope(home.path()),
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, SkillInstallError::NoSkillsFound(_)));
}

#[tokio::test]
async fn reinstalling_identical_content_is_unchanged() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();
    let second = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(second[0].action, InstallAction::Unchanged);
}

#[tokio::test]
async fn installing_over_an_untracked_skill_requires_force() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    // A hand-written skill with no lockfile entry.
    let existing = target.skills_dir().join("deploy");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("SKILL.md"), "---\nname: deploy\n---\nmine\n").unwrap();

    let error = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::AlreadyInstalled(_)));

    let forced = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions {
            force: true,
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap();
    assert_eq!(forced[0].action, InstallAction::Updated);
}

#[tokio::test]
async fn locally_edited_skills_are_protected_from_being_overwritten() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    // The user edits the installed copy, then upstream also changes.
    std::fs::write(
        target.skills_dir().join("deploy").join("SKILL.md"),
        "---\nname: deploy\ndescription: My edit\n---\nlocal changes\n",
    )
    .unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: deploy\ndescription: Upstream change\n---\nnew upstream\n",
    )
    .unwrap();

    let error = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::LocallyModified(_)));

    let forced = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions {
            force: true,
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap();
    assert_eq!(forced[0].action, InstallAction::Updated);
    assert!(
        std::fs::read_to_string(target.skills_dir().join("deploy").join("SKILL.md"))
            .unwrap()
            .contains("new upstream")
    );
}

#[tokio::test]
async fn dry_run_writes_nothing_and_records_nothing() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    let planned = install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(planned[0].action, InstallAction::Installed);
    assert!(!target.skills_dir().join("deploy").exists());
    assert!(SkillLock::load(target.root()).skills.is_empty());
}

#[tokio::test]
async fn update_reinstalls_from_the_recorded_source() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        skill.to_str().unwrap(),
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: deploy\ndescription: Ship it faster\n---\nv2\n",
    )
    .unwrap();

    let updated = update(
        "deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
        src.path(),
    )
    .await
    .unwrap();

    assert_eq!(updated.action, InstallAction::Updated);
    assert_eq!(updated.description, "Ship it faster");
}

#[tokio::test]
async fn update_is_unchanged_when_the_source_did_not_move() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        skill.to_str().unwrap(),
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let updated = update(
        "deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
        src.path(),
    )
    .await
    .unwrap();
    assert_eq!(updated.action, InstallAction::Unchanged);
}

#[tokio::test]
async fn update_rejects_a_skill_that_was_not_installed_from_the_marketplace() {
    let home = tempfile::tempdir().unwrap();
    let target = scope(home.path());
    let error = update(
        "handwritten",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
        home.path(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::NotTracked(_)));
}

#[tokio::test]
async fn remove_deletes_the_directory_and_the_record() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let removed = remove("deploy", &target, false).unwrap();
    assert_eq!(removed, target.skills_dir().join("deploy"));
    assert!(!removed.exists());
    assert!(SkillLock::load(target.root()).skills.is_empty());
}

#[tokio::test]
async fn remove_can_keep_files_while_dropping_tracking() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    remove("deploy", &target, true).unwrap();
    assert!(target.skills_dir().join("deploy").exists());
    assert!(SkillLock::load(target.root()).skills.is_empty());
}

#[tokio::test]
async fn remove_clears_a_stale_record_when_the_directory_is_already_gone() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();
    std::fs::remove_dir_all(target.skills_dir().join("deploy")).unwrap();

    remove("deploy", &target, false).unwrap();
    assert!(SkillLock::load(target.root()).skills.is_empty());
}

#[tokio::test]
async fn remove_reports_a_skill_that_was_never_installed() {
    let home = tempfile::tempdir().unwrap();
    let error = remove("nope", &scope(home.path()), false).unwrap_err();
    assert!(matches!(error, SkillInstallError::NotInstalled(_)));
}

#[tokio::test]
async fn remove_works_for_a_hand_written_skill() {
    let home = tempfile::tempdir().unwrap();
    let target = scope(home.path());
    let dir = target.skills_dir().join("handwritten");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: handwritten\n---\n").unwrap();

    remove("handwritten", &target, false).unwrap();
    assert!(!dir.exists());
}

#[tokio::test]
async fn install_replaces_files_removed_upstream() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    std::fs::write(skill.join("extra.md"), "temporary").unwrap();
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();
    assert!(target.skills_dir().join("deploy").join("extra.md").exists());

    std::fs::remove_file(skill.join("extra.md")).unwrap();
    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions {
            force: true,
            ..InstallOptions::default()
        },
        &offline_registry(),
    )
    .await
    .unwrap();

    assert!(!target.skills_dir().join("deploy").join("extra.md").exists());
    assert!(target.skills_dir().join("deploy").join("SKILL.md").exists());
}

#[tokio::test]
async fn install_skips_metadata_json_sidecars() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "deploy", "Ship it");
    std::fs::write(skill.join("metadata.json"), "{}").unwrap();
    let target = scope(home.path());

    install(
        &local(&skill),
        "./deploy",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert!(
        !target
            .skills_dir()
            .join("deploy")
            .join("metadata.json")
            .exists()
    );
}

#[tokio::test]
async fn install_reports_a_missing_local_path() {
    let home = tempfile::tempdir().unwrap();
    let error = install(
        &local(Path::new("/definitely/not/here")),
        "/definitely/not/here",
        &scope(home.path()),
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::NoSkillsFound(_)));
}

#[tokio::test]
async fn a_partial_multi_skill_failure_still_records_what_landed() {
    let src = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write_skill(src.path(), "alpha", "First");
    write_skill(src.path(), "beta", "Second");
    let target = scope(home.path());

    // `beta` already exists untracked, so installing the pair fails on it
    // after `alpha` has already been written.
    let existing = target.skills_dir().join("beta");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("SKILL.md"), "---\nname: beta\n---\nmine\n").unwrap();

    let error = install(
        &local(src.path()),
        "./skills",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::AlreadyInstalled(_)));

    // `alpha` landed, so it must be tracked rather than looking hand-written.
    assert!(target.skills_dir().join("alpha").join("SKILL.md").is_file());
    let lock = SkillLock::load(target.root());
    assert!(lock.contains("alpha"));
    assert!(!lock.contains("beta"));
}

#[tokio::test]
async fn install_action_labels_are_stable() {
    assert_eq!(InstallAction::Installed.as_str(), "installed");
    assert_eq!(InstallAction::Updated.as_str(), "updated");
    assert_eq!(InstallAction::Unchanged.as_str(), "unchanged");
}

// ── git-backed installs ─────────────────────────────────────────────
//
// These clone from a local `file://`-style path, so they exercise the real git
// path without needing a network or a remote host.

fn git_available() -> bool {
    let git_bin = std::env::var("GIT_BIN_PATH").unwrap_or_else(|_| "git".to_string());
    std::process::Command::new(git_bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success())
}

fn run_git(dir: &Path, args: &[&str]) {
    let git_bin = std::env::var("GIT_BIN_PATH").unwrap_or_else(|_| "git".to_string());
    let output = std::process::Command::new(git_bin)
        .current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo_with_skill(path: &Path, skill_subdir: &str, name: &str) {
    run_git(path, &["init", "--initial-branch", "main"]);
    run_git(path, &["config", "user.email", "test@example.com"]);
    run_git(path, &["config", "user.name", "Test User"]);
    write_skill(&path.join(skill_subdir), name, "From git");
    run_git(path, &["add", "-A"]);
    run_git(path, &["commit", "-m", "add skill"]);
}

fn git_source(path: &Path, subpath: Option<&str>, filter: Option<&str>) -> SkillSource {
    SkillSource::Git {
        url: path.to_string_lossy().to_string(),
        git_ref: None,
        subpath: subpath.map(str::to_string),
        skill_filter: filter.map(str::to_string),
        host: crate::skill_source::GitHost::Other,
    }
}

#[tokio::test]
async fn installs_a_skill_from_a_git_repository() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_repo_with_skill(remote.path(), "skills", "deploy");
    let target = scope(home.path());

    let installed = install(
        &git_source(remote.path(), None, None),
        "owner/repo",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "deploy");
    assert!(
        target
            .skills_dir()
            .join("deploy")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn git_install_records_the_resolved_commit() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_repo_with_skill(remote.path(), "skills", "deploy");
    let target = scope(home.path());

    install(
        &git_source(remote.path(), None, None),
        "owner/repo",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let lock = SkillLock::load(target.root());
    let entry = lock.get("deploy").unwrap();
    assert_eq!(entry.origin, SkillOrigin::Git);
    let commit = entry.commit.as_deref().expect("commit should be recorded");
    assert_eq!(commit.len(), 40);
    assert!(commit.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn git_install_does_not_copy_the_dot_git_directory() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // A skill at the repository root would otherwise pull `.git` along with it.
    init_repo_with_skill(remote.path(), ".", "deploy");
    let target = scope(home.path());

    install(
        &git_source(remote.path(), None, None),
        "owner/repo",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert!(!target.skills_dir().join("deploy").join(".git").exists());
}

#[tokio::test]
async fn git_install_honors_a_subdirectory() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    run_git(remote.path(), &["init", "--initial-branch", "main"]);
    run_git(remote.path(), &["config", "user.email", "t@e.com"]);
    run_git(remote.path(), &["config", "user.name", "T"]);
    write_skill(&remote.path().join("packages"), "wanted", "Yes");
    write_skill(&remote.path().join("other"), "unwanted", "No");
    run_git(remote.path(), &["add", "-A"]);
    run_git(remote.path(), &["commit", "-m", "skills"]);
    let target = scope(home.path());

    let installed = install(
        &git_source(remote.path(), Some("packages"), None),
        "owner/repo/packages",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["wanted"]);
}

#[tokio::test]
async fn git_install_honors_a_skill_filter() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    run_git(remote.path(), &["init", "--initial-branch", "main"]);
    run_git(remote.path(), &["config", "user.email", "t@e.com"]);
    run_git(remote.path(), &["config", "user.name", "T"]);
    write_skill(&remote.path().join("skills"), "alpha", "First");
    write_skill(&remote.path().join("skills"), "beta", "Second");
    run_git(remote.path(), &["add", "-A"]);
    run_git(remote.path(), &["commit", "-m", "skills"]);
    let target = scope(home.path());

    let installed = install(
        &git_source(remote.path(), None, Some("beta")),
        "owner/repo@beta",
        &target,
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap();

    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "beta");
}

#[tokio::test]
async fn git_install_rejects_a_missing_subdirectory() {
    if !git_available() {
        eprintln!("skipping git-dependent test: git binary not available");
        return;
    }
    let remote = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_repo_with_skill(remote.path(), "skills", "deploy");

    let error = install(
        &git_source(remote.path(), Some("nope"), None),
        "owner/repo/nope",
        &scope(home.path()),
        &InstallOptions::default(),
        &offline_registry(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SkillInstallError::NoSkillsFound(_)));
}
