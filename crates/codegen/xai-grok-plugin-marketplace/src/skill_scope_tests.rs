use std::path::Path;

use super::{SkillScopeKind, SkillScopeTarget};

#[test]
fn parses_scope_aliases() {
    assert_eq!(SkillScopeKind::parse("user"), Some(SkillScopeKind::User));
    assert_eq!(SkillScopeKind::parse("GLOBAL"), Some(SkillScopeKind::User));
    assert_eq!(
        SkillScopeKind::parse("project"),
        Some(SkillScopeKind::Project)
    );
    assert_eq!(
        SkillScopeKind::parse(" repo "),
        Some(SkillScopeKind::Project)
    );
    assert_eq!(
        SkillScopeKind::parse("local"),
        Some(SkillScopeKind::Project)
    );
    assert_eq!(SkillScopeKind::parse("nonsense"), None);
}

#[test]
fn scope_labels_are_stable() {
    assert_eq!(SkillScopeKind::User.as_str(), "user");
    assert_eq!(SkillScopeKind::Project.as_str(), "project");
    assert_eq!(SkillScopeKind::default(), SkillScopeKind::User);
}

#[test]
fn user_scope_lives_under_grok_home() {
    let target = SkillScopeTarget::user(Path::new("/home/u/.grok"));
    assert_eq!(target.kind(), SkillScopeKind::User);
    assert_eq!(target.root(), Path::new("/home/u/.grok"));
    assert_eq!(target.skills_dir(), Path::new("/home/u/.grok/skills"));
}

#[test]
fn project_scope_prefers_the_repository_root() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    let nested = repo.path().join("crates").join("thing");
    std::fs::create_dir_all(&nested).unwrap();

    let target = SkillScopeTarget::project(&nested);
    assert_eq!(target.kind(), SkillScopeKind::Project);
    assert_eq!(target.root(), repo.path().join(".grok"));
}

#[test]
fn project_scope_accepts_a_git_file_for_worktrees() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".git"), "gitdir: /elsewhere").unwrap();

    let target = SkillScopeTarget::project(repo.path());
    assert_eq!(target.root(), repo.path().join(".grok"));
}

#[test]
fn project_scope_falls_back_to_cwd_outside_a_repository() {
    let plain = tempfile::tempdir().unwrap();
    let target = SkillScopeTarget::project(plain.path());
    assert_eq!(target.root(), plain.path().join(".grok"));
}

#[test]
fn skill_dir_normalizes_names() {
    let target = SkillScopeTarget::user(Path::new("/home/u/.grok"));
    assert_eq!(
        target.skill_dir("Deploy To Prod").unwrap(),
        Path::new("/home/u/.grok/skills/deploy-to-prod")
    );
}

#[test]
fn skill_dir_rejects_traversal_attempts() {
    let target = SkillScopeTarget::user(Path::new("/home/u/.grok"));
    // Normalization turns separators into hyphens, so a traversal attempt can
    // never produce a path outside the skills directory.
    let resolved = target.skill_dir("../../etc/passwd").unwrap();
    assert!(resolved.starts_with("/home/u/.grok/skills"));
    assert_eq!(target.skill_dir(""), None);
    assert_eq!(target.skill_dir("!!!"), None);
}

#[test]
fn installed_names_lists_only_directories_with_skill_md() {
    let home = tempfile::tempdir().unwrap();
    let target = SkillScopeTarget::user(home.path());
    let skills = target.skills_dir();

    std::fs::create_dir_all(skills.join("deploy")).unwrap();
    std::fs::write(skills.join("deploy").join("SKILL.md"), "---\n---\n").unwrap();
    std::fs::create_dir_all(skills.join("alpha")).unwrap();
    std::fs::write(skills.join("alpha").join("SKILL.md"), "---\n---\n").unwrap();
    std::fs::create_dir_all(skills.join("not-a-skill")).unwrap();
    std::fs::write(skills.join("loose.md"), "x").unwrap();

    assert_eq!(target.installed_names(), vec!["alpha", "deploy"]);
}

#[test]
fn installed_names_is_empty_when_the_directory_is_missing() {
    let home = tempfile::tempdir().unwrap();
    let target = SkillScopeTarget::user(home.path());
    assert!(target.installed_names().is_empty());
}
