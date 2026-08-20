//! Git access for skill installs.
//!
//! Skill installs need a throwaway checkout, not the shared marketplace source
//! cache: they read a repository once and copy the files out. This module keeps
//! that distinction explicit while reusing the crate's hardened git runner
//! (URL and ref validation, bounded timeouts, process-scope enrollment).

use std::path::Path;

/// Clone `url` into `dest` and return the checked-out commit sha when known.
///
/// `git_ref` accepts a branch, tag, or full commit sha.
pub fn clone_source(
    url: &str,
    git_ref: Option<&str>,
    dest: &Path,
) -> Result<Option<String>, String> {
    crate::git::clone_for_read(url, git_ref, dest)
}

/// Check whether a remote is reachable before attempting a full clone.
pub fn probe(url: &str) -> Result<(), String> {
    crate::git::probe_git_remote(url)
}
