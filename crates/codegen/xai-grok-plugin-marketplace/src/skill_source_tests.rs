use std::path::{Path, PathBuf};

use super::{GitHost, SkillSource, SkillSourceError, parse_skill_source};

fn cwd() -> PathBuf {
    PathBuf::from("/work/project")
}

fn git(input: &str) -> SkillSource {
    parse_skill_source(input, &cwd(), false).expect("source should parse")
}

#[test]
fn parses_github_shorthand() {
    assert_eq!(
        git("vercel-labs/agent-skills"),
        SkillSource::Git {
            url: "https://github.com/vercel-labs/agent-skills.git".into(),
            git_ref: None,
            subpath: None,
            skill_filter: None,
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn strips_dot_git_suffix_from_shorthand() {
    let SkillSource::Git { url, .. } = git("owner/repo.git") else {
        panic!("expected git source");
    };
    assert_eq!(url, "https://github.com/owner/repo.git");
}

#[test]
fn parses_shorthand_with_subdirectory() {
    assert_eq!(
        git("owner/repo/packages/skills"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: None,
            subpath: Some("packages/skills".into()),
            skill_filter: None,
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn parses_shorthand_with_skill_filter() {
    assert_eq!(
        git("owner/repo@commit-helper"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: None,
            subpath: None,
            skill_filter: Some("commit-helper".into()),
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn parses_fragment_ref_and_skill_filter() {
    assert_eq!(
        git("owner/repo#v2.1@deploy"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: Some("v2.1".into()),
            subpath: None,
            skill_filter: Some("deploy".into()),
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn fragment_skill_filter_overrides_at_suffix() {
    let SkillSource::Git { skill_filter, .. } = git("owner/repo@ignored#main@wanted") else {
        panic!("expected git source");
    };
    assert_eq!(skill_filter.as_deref(), Some("wanted"));
}

#[test]
fn parses_github_tree_url_with_ref_and_subdir() {
    assert_eq!(
        git("https://github.com/owner/repo/tree/main/skills/deploy"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: Some("main".into()),
            subpath: Some("skills/deploy".into()),
            skill_filter: None,
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn parses_github_tree_url_without_subdir() {
    assert_eq!(
        git("https://github.com/owner/repo/tree/release"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: Some("release".into()),
            subpath: None,
            skill_filter: None,
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn normalizes_www_and_uppercase_hosts() {
    let SkillSource::Git { url, host, .. } = git("https://WWW.GitHub.com/Owner/Repo") else {
        panic!("expected git source");
    };
    assert_eq!(url, "https://github.com/Owner/Repo.git");
    assert_eq!(host, GitHost::GitHub);
}

#[test]
fn parses_gitlab_tree_url_with_nested_groups() {
    assert_eq!(
        git("https://gitlab.com/group/sub/project/-/tree/main/skills"),
        SkillSource::Git {
            url: "https://gitlab.com/group/sub/project.git".into(),
            git_ref: Some("main".into()),
            subpath: Some("skills".into()),
            skill_filter: None,
            host: GitHost::GitLab,
        }
    );
}

#[test]
fn parses_gitlab_prefix_shorthand() {
    let SkillSource::Git { url, host, .. } = git("gitlab:group/project") else {
        panic!("expected git source");
    };
    assert_eq!(url, "https://gitlab.com/group/project.git");
    assert_eq!(host, GitHost::GitLab);
}

#[test]
fn parses_github_prefix_shorthand() {
    let SkillSource::Git { url, .. } = git("github:owner/repo") else {
        panic!("expected git source");
    };
    assert_eq!(url, "https://github.com/owner/repo.git");
}

#[test]
fn parses_scp_style_ssh_source() {
    assert_eq!(
        git("git@github.com:owner/repo.git"),
        SkillSource::Git {
            url: "https://github.com/owner/repo.git".into(),
            git_ref: None,
            subpath: None,
            skill_filter: None,
            host: GitHost::GitHub,
        }
    );
}

#[test]
fn keeps_ssh_scheme_for_ssh_urls() {
    let SkillSource::Git { url, .. } = git("ssh://git@github.com/owner/repo.git") else {
        panic!("expected git source");
    };
    assert_eq!(url, "ssh://github.com/owner/repo.git");
}

#[test]
fn parses_self_hosted_git_url_as_other_host() {
    let SkillSource::Git { url, host, .. } = git("https://git.example.com/team/skills") else {
        panic!("expected git source");
    };
    assert_eq!(url, "https://git.example.com/team/skills.git");
    assert_eq!(host, GitHost::Other);
}

#[test]
fn parses_registry_id_when_registry_is_preferred() {
    assert_eq!(
        parse_skill_source("vercel-labs/skills/find-skills", &cwd(), true).unwrap(),
        SkillSource::Registry {
            source: "vercel-labs/skills".into(),
            slug: "find-skills".into(),
        }
    );
}

#[test]
fn treats_three_segments_as_subdir_when_registry_not_preferred() {
    let SkillSource::Git { subpath, .. } =
        parse_skill_source("vercel-labs/skills/find-skills", &cwd(), false).unwrap()
    else {
        panic!("expected git source");
    };
    assert_eq!(subpath.as_deref(), Some("find-skills"));
}

#[test]
fn registry_preference_does_not_apply_with_a_ref() {
    let source = parse_skill_source("owner/repo/dir#main", &cwd(), true).unwrap();
    assert!(matches!(source, SkillSource::Git { .. }));
}

#[test]
fn parses_relative_and_absolute_local_paths() {
    assert_eq!(
        parse_skill_source("./skills", &cwd(), false).unwrap(),
        SkillSource::Local {
            path: cwd().join("./skills")
        }
    );
    assert_eq!(
        parse_skill_source("/opt/skills", &cwd(), false).unwrap(),
        SkillSource::Local {
            path: PathBuf::from("/opt/skills")
        }
    );
    assert_eq!(
        parse_skill_source(".", &cwd(), false).unwrap(),
        SkillSource::Local {
            path: cwd().join(".")
        }
    );
}

#[test]
fn rejects_empty_and_unrecognized_sources() {
    assert_eq!(
        parse_skill_source("   ", &cwd(), false).unwrap_err(),
        SkillSourceError::Empty
    );
    assert!(matches!(
        parse_skill_source("just-a-word", &cwd(), false).unwrap_err(),
        SkillSourceError::Unrecognized(_)
    ));
}

#[test]
fn rejects_parent_traversal_in_subpath() {
    assert!(matches!(
        parse_skill_source("owner/repo/../../etc", &cwd(), false).unwrap_err(),
        SkillSourceError::UnsafeSubpath(_)
    ));
    assert!(matches!(
        parse_skill_source("https://github.com/o/r/tree/main/../etc", &cwd(), false).unwrap_err(),
        SkillSourceError::UnsafeSubpath(_)
    ));
}

#[test]
fn rejects_control_characters() {
    assert!(matches!(
        parse_skill_source("owner/repo\nrm -rf", &cwd(), false).unwrap_err(),
        SkillSourceError::ControlCharacter(_)
    ));
}

#[test]
fn identity_is_stable_and_includes_ref_and_subpath() {
    assert_eq!(
        git("owner/repo/dir#v1").identity(),
        "https://github.com/owner/repo.git#v1/dir"
    );
    assert_eq!(
        parse_skill_source("owner/repo/slug", &cwd(), true)
            .unwrap()
            .identity(),
        "owner/repo/slug"
    );
    assert_eq!(
        SkillSource::Local {
            path: Path::new("/tmp/x").to_path_buf()
        }
        .identity(),
        "local:/tmp/x"
    );
}

#[test]
fn kind_labels_match_source_family() {
    assert_eq!(git("owner/repo").kind(), "github");
    assert_eq!(git("gitlab:group/project").kind(), "gitlab");
    assert_eq!(git("https://git.example.com/a/b").kind(), "git");
    assert_eq!(
        parse_skill_source("./x", &cwd(), false).unwrap().kind(),
        "local"
    );
    assert_eq!(
        parse_skill_source("o/r/s", &cwd(), true).unwrap().kind(),
        "registry"
    );
}
