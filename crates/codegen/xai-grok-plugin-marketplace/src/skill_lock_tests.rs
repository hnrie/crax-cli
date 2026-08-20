use std::path::Path;

use super::{
    LOCK_FILE_NAME, LOCK_VERSION, SkillLock, SkillLockEntry, SkillOrigin, hash_files,
    hash_skill_dir, now_timestamp,
};

fn entry(source: &str, hash: &str) -> SkillLockEntry {
    SkillLockEntry {
        source: source.to_string(),
        source_id: source.to_string(),
        origin: SkillOrigin::Registry,
        registry_id: Some(source.to_string()),
        git_ref: None,
        commit: None,
        content_hash: hash.to_string(),
        registry_hash: None,
        installed_at: "2020-01-01T00:00:00Z".to_string(),
        updated_at: "2020-01-01T00:00:00Z".to_string(),
    }
}

#[test]
fn missing_lockfile_loads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let lock = SkillLock::load(dir.path());
    assert_eq!(lock.version, LOCK_VERSION);
    assert!(lock.skills.is_empty());
}

#[test]
fn round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut lock = SkillLock::default();
    lock.upsert("deploy", entry("owner/repo/deploy", "hash-a"));
    lock.save(dir.path()).unwrap();

    assert!(dir.path().join(LOCK_FILE_NAME).exists());
    let reloaded = SkillLock::load(dir.path());
    assert_eq!(reloaded, lock);
    assert_eq!(reloaded.get("deploy").unwrap().content_hash, "hash-a");
}

#[test]
fn upsert_preserves_the_original_install_timestamp() {
    let mut lock = SkillLock::default();
    lock.upsert("deploy", entry("owner/repo/deploy", "hash-a"));

    let mut updated = entry("owner/repo/deploy", "hash-b");
    updated.installed_at = "2030-06-06T00:00:00Z".to_string();
    updated.updated_at = "2030-06-06T00:00:00Z".to_string();
    lock.upsert("deploy", updated);

    let stored = lock.get("deploy").unwrap();
    assert_eq!(stored.installed_at, "2020-01-01T00:00:00Z");
    assert_eq!(stored.updated_at, "2030-06-06T00:00:00Z");
    assert_eq!(stored.content_hash, "hash-b");
}

#[test]
fn remove_reports_whether_an_entry_existed() {
    let mut lock = SkillLock::default();
    lock.upsert("deploy", entry("owner/repo/deploy", "hash-a"));
    assert!(lock.contains("deploy"));
    assert!(lock.remove("deploy"));
    assert!(!lock.remove("deploy"));
    assert!(!lock.contains("deploy"));
}

#[test]
fn saving_an_empty_lock_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut lock = SkillLock::default();
    lock.upsert("deploy", entry("owner/repo/deploy", "hash-a"));
    lock.save(dir.path()).unwrap();
    assert!(dir.path().join(LOCK_FILE_NAME).exists());

    lock.remove("deploy");
    lock.save(dir.path()).unwrap();
    assert!(!dir.path().join(LOCK_FILE_NAME).exists());
}

#[test]
fn corrupt_lockfile_is_treated_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(LOCK_FILE_NAME), "{ not json").unwrap();
    assert!(SkillLock::load(dir.path()).skills.is_empty());
}

#[test]
fn lockfile_from_a_newer_or_older_schema_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(LOCK_FILE_NAME),
        r#"{"version":0,"skills":{"x":{}}}"#,
    )
    .unwrap();
    assert!(SkillLock::load(dir.path()).skills.is_empty());
}

#[test]
fn skills_are_serialized_in_sorted_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut lock = SkillLock::default();
    lock.upsert("zebra", entry("o/r/zebra", "h"));
    lock.upsert("alpha", entry("o/r/alpha", "h"));
    lock.save(dir.path()).unwrap();

    let raw = std::fs::read_to_string(dir.path().join(LOCK_FILE_NAME)).unwrap();
    assert!(raw.find("alpha").unwrap() < raw.find("zebra").unwrap());
}

#[test]
fn file_hash_is_order_independent_but_content_sensitive() {
    let a = hash_files([("SKILL.md", b"one".as_slice()), ("b.md", b"two".as_slice())]);
    let b = hash_files([("b.md", b"two".as_slice()), ("SKILL.md", b"one".as_slice())]);
    assert_eq!(a, b);

    let changed = hash_files([
        ("SKILL.md", b"one!".as_slice()),
        ("b.md", b"two".as_slice()),
    ]);
    assert_ne!(a, changed);
}

#[test]
fn file_hash_distinguishes_content_moved_between_files() {
    let a = hash_files([("a.md", b"xy".as_slice()), ("b.md", b"z".as_slice())]);
    let b = hash_files([("a.md", b"x".as_slice()), ("b.md", b"yz".as_slice())]);
    assert_ne!(a, b);
}

#[test]
fn file_hash_distinguishes_renamed_files() {
    let a = hash_files([("a.md", b"same".as_slice())]);
    let b = hash_files([("b.md", b"same".as_slice())]);
    assert_ne!(a, b);
}

#[test]
fn directory_hash_matches_the_equivalent_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), "body").unwrap();
    std::fs::create_dir_all(dir.path().join("rules")).unwrap();
    std::fs::write(dir.path().join("rules").join("a.md"), "rule").unwrap();

    let expected = hash_files([
        ("SKILL.md", b"body".as_slice()),
        ("rules/a.md", b"rule".as_slice()),
    ]);
    assert_eq!(hash_skill_dir(dir.path()).unwrap(), expected);
}

#[test]
fn directory_hash_changes_when_a_file_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), "body").unwrap();
    let before = hash_skill_dir(dir.path()).unwrap();

    std::fs::write(dir.path().join("SKILL.md"), "body edited").unwrap();
    assert_ne!(hash_skill_dir(dir.path()).unwrap(), before);
}

#[test]
fn directory_hash_fails_for_a_missing_directory() {
    assert!(hash_skill_dir(Path::new("/nonexistent/skill/dir")).is_err());
}

#[test]
fn timestamps_are_rfc3339() {
    let now = now_timestamp();
    assert!(chrono::DateTime::parse_from_rfc3339(&now).is_ok());
}

#[test]
fn origin_labels_are_stable() {
    assert_eq!(SkillOrigin::Registry.as_str(), "registry");
    assert_eq!(SkillOrigin::Git.as_str(), "git");
    assert_eq!(SkillOrigin::Local.as_str(), "local");
}
