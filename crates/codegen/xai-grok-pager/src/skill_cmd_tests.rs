use clap::Parser as _;

use super::{SkillCommand, format_installs, preview, scope_label, truncate};
use xai_grok_plugin_marketplace::skill_scope::SkillScopeKind;

/// Parse a `grok skill …` invocation through the real top-level parser, so
/// these tests exercise the same clap wiring the binary uses.
fn parse(args: &[&str]) -> SkillCommand {
    let mut argv = vec!["grok", "skill"];
    argv.extend_from_slice(args);
    let parsed = crate::app::cli::PagerArgs::try_parse_from(argv).expect("args should parse");
    match parsed.command {
        Some(crate::app::cli::Command::Skill(skill)) => skill.command,
        other => panic!("expected the skill subcommand, got {other:?}"),
    }
}

fn try_parse(args: &[&str]) -> Result<SkillCommand, clap::Error> {
    let mut argv = vec!["grok", "skill"];
    argv.extend_from_slice(args);
    let parsed = crate::app::cli::PagerArgs::try_parse_from(argv)?;
    match parsed.command {
        Some(crate::app::cli::Command::Skill(skill)) => Ok(skill.command),
        _ => panic!("expected the skill subcommand"),
    }
}

#[test]
fn search_joins_multi_word_queries() {
    let SkillCommand::Search { query, limit, .. } = parse(&["search", "react", "native"]) else {
        panic!("expected search");
    };
    assert_eq!(query.join(" "), "react native");
    assert_eq!(limit, super::DEFAULT_SEARCH_LIMIT);
}

#[test]
fn search_accepts_limit_owner_and_json() {
    let SkillCommand::Search {
        limit, owner, json, ..
    } = parse(&[
        "search", "deploy", "--limit", "5", "--owner", "vercel", "--json",
    ])
    else {
        panic!("expected search");
    };
    assert_eq!(limit, 5);
    assert_eq!(owner.as_deref(), Some("vercel"));
    assert!(json);
}

#[test]
fn search_has_a_find_alias() {
    assert!(matches!(
        parse(&["find", "deploy"]),
        SkillCommand::Search { .. }
    ));
}

#[test]
fn search_requires_a_query() {
    assert!(try_parse(&["search"]).is_err());
}

#[test]
fn install_defaults_to_the_user_scope() {
    let SkillCommand::Install {
        source,
        scope,
        force,
        dry_run,
        only,
        ..
    } = parse(&["install", "owner/repo"])
    else {
        panic!("expected install");
    };
    assert_eq!(source, "owner/repo");
    assert_eq!(scope, "user");
    assert!(!force);
    assert!(!dry_run);
    assert_eq!(only, None);
}

#[test]
fn install_accepts_project_scope_and_flags() {
    let SkillCommand::Install {
        scope,
        only,
        force,
        dry_run,
        ..
    } = parse(&[
        "install",
        "owner/repo",
        "--scope",
        "project",
        "--only",
        "deploy",
        "--force",
        "--dry-run",
    ])
    else {
        panic!("expected install");
    };
    assert_eq!(scope, "project");
    assert_eq!(only.as_deref(), Some("deploy"));
    assert!(force);
    assert!(dry_run);
}

#[test]
fn install_has_an_add_alias() {
    assert!(matches!(
        parse(&["add", "owner/repo"]),
        SkillCommand::Install { .. }
    ));
}

#[test]
fn list_scope_is_optional() {
    let SkillCommand::List { scope, json } = parse(&["list"]) else {
        panic!("expected list");
    };
    assert_eq!(scope, None);
    assert!(!json);

    let SkillCommand::List { scope, .. } = parse(&["ls", "--scope", "project"]) else {
        panic!("expected list");
    };
    assert_eq!(scope.as_deref(), Some("project"));
}

#[test]
fn update_name_is_optional() {
    let SkillCommand::Update { name, .. } = parse(&["update"]) else {
        panic!("expected update");
    };
    assert_eq!(name, None);

    let SkillCommand::Update { name, force, .. } = parse(&["update", "deploy", "--force"]) else {
        panic!("expected update");
    };
    assert_eq!(name.as_deref(), Some("deploy"));
    assert!(force);
}

#[test]
fn remove_requires_a_name_and_supports_keep_files() {
    assert!(try_parse(&["remove"]).is_err());

    let SkillCommand::Remove {
        name, keep_files, ..
    } = parse(&["remove", "deploy", "--keep-files"])
    else {
        panic!("expected remove");
    };
    assert_eq!(name, "deploy");
    assert!(keep_files);
}

#[test]
fn remove_has_rm_and_uninstall_aliases() {
    assert!(matches!(
        parse(&["rm", "deploy"]),
        SkillCommand::Remove { .. }
    ));
    assert!(matches!(
        parse(&["uninstall", "deploy"]),
        SkillCommand::Remove { .. }
    ));
}

#[test]
fn info_accepts_a_source_and_json() {
    let SkillCommand::Info { source, json } = parse(&["info", "owner/repo/slug", "--json"]) else {
        panic!("expected info");
    };
    assert_eq!(source, "owner/repo/slug");
    assert!(json);

    assert!(matches!(
        parse(&["show", "owner/repo/slug"]),
        SkillCommand::Info { .. }
    ));
}

#[test]
fn resolve_scope_rejects_unknown_values() {
    assert!(super::resolve_scope("nonsense").is_err());
    assert!(super::resolve_scope("user").is_ok());
    assert!(super::resolve_scope("project").is_ok());
}

#[test]
fn target_for_user_scope_points_at_grok_home() {
    let target = super::target_for(SkillScopeKind::User);
    assert_eq!(target.kind(), SkillScopeKind::User);
    assert!(target.skills_dir().ends_with("skills"));
}

#[test]
fn scopes_to_inspect_covers_both_scopes_by_default() {
    let both = super::scopes_to_inspect(None).unwrap();
    assert_eq!(both.len(), 2);
    let one = super::scopes_to_inspect(Some("user")).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].kind(), SkillScopeKind::User);
    assert!(super::scopes_to_inspect(Some("bogus")).is_err());
}

#[test]
fn install_counts_are_abbreviated() {
    assert_eq!(format_installs(0), "—");
    assert_eq!(format_installs(42), "42");
    assert_eq!(format_installs(1_500), "1.5k");
    assert_eq!(format_installs(646_463), "646.5k");
    assert_eq!(format_installs(2_400_000), "2.4m");
}

#[test]
fn truncate_preserves_short_values_and_marks_long_ones() {
    assert_eq!(truncate("short", 10), "short");
    assert_eq!(truncate("abcdefghij", 5), "abcd…");
    // Multi-byte characters are counted, not sliced mid-codepoint.
    assert_eq!(truncate("日本語テキスト", 3), "日本…");
}

#[test]
fn preview_caps_the_number_of_lines() {
    let long: String = (0..50)
        .map(|i| format!("line {i}\n"))
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(preview(&long).lines().count(), 20);
}

#[test]
fn scope_label_describes_the_search_area() {
    assert_eq!(scope_label(None), "any scope");
    assert_eq!(scope_label(Some("user")), "the user scope");
}
