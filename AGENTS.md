# AGENTS.md

Grok Build (`grok`) is a terminal-based AI coding agent: a Rust workspace producing a
full-screen TUI plus its agent runtime, tool implementations, and workspace host.

This tree is synced periodically from the SpaceXAI monorepo (`SOURCE_REV` records the
upstream commit) with a small local fork on top. Prefer changes that survive the next sync.

## Build and test

Cargo workspace, 92 members, edition 2024, toolchain pinned to 1.94.0 by `rust-toolchain.toml`
(rustup installs it automatically). No MSRV is declared beyond that pin.

**Always scope commands with `-p <crate>`.** Full-workspace builds take minutes and produce
multi-gigabyte `target/` dirs.

```sh
cargo check -p <crate>              # fast validation loop
cargo test -p <crate>               # per-crate tests
cargo clippy -p <crate>             # config: clippy.toml at the repo root
cargo fmt --all                     # config: rustfmt.toml at the repo root
```

`cargo test -p xai-grok-config` (212 tests, sub-second, no network or TTY) is the cheapest
end-to-end sanity gate. Run it plus `cargo fmt --all --check` and a `cargo clippy -p <crate>`
on whatever you touched before finishing.

CI only builds a Windows release binary — it does not run tests, clippy, or fmt, and there
is no Linux or macOS job. Nothing downstream catches a non-Windows regression, so local
validation is the only gate.

`cargo check --all-targets --workspace` is reserved for toolchain bumps (see the comment in
`rust-toolchain.toml`), not the normal loop.

## Validating changes

The TUI cannot be exercised headlessly: `cargo run -p xai-grok-pager-bin` needs a real TTY
and opens a browser to authenticate on first launch. Validate with `cargo check` and
`cargo test` instead of trying to start the app.

Debug builds are self-contained. Release builds are not: the build scripts download ripgrep
from GitHub releases, so `--release` and `--profile release-dist` can fail offline where
`cargo check` succeeds. Point `GROK_SHELL_BUNDLE_RG_PATH` / `GROK_TOOLS_BUNDLE_RG_PATH` at a
local binary for offline release builds. bfs and ugrep are never downloaded — they bundle only
from `GROK_TOOLS_BUNDLE_{BFS,UGREP}_PATH` and are silently skipped when unset.

Proto codegen (only `xai-grok-tools-api` compiles protos) resolves `protoc` via `$PROTOC`,
then `bin/protoc` through DotSlash, then `PATH`. `dotslash` must be installed and on `PATH`.
Outside GitHub Actions a missing protoc fails soft with a warning, which surfaces later as a
confusing downstream error rather than a clean one.

## Things that will bite you

**The root `Cargo.toml` is generated — treat it as read-only.** It owns all workspace members,
~250 dependency versions, lints, and profiles. Add dependencies in the per-crate manifest
referencing an existing `[workspace.dependencies]` entry. Root edits get clobbered by the
next monorepo sync.

**`panic = "abort"` in both the `dev` and `release` profiles.** No unwinding in tests or dev
builds, so `catch_unwind` and `#[should_panic]` do not behave like a default cargo project,
and a failed thread spawn aborts the process.

**Two families of APIs are clippy-banned workspace-wide** (see `clippy.toml` for the rationale):

- `std::fs::canonicalize`, `Path::canonicalize`, `tokio::fs::canonicalize` — use
  `dunce::canonicalize`. Raw canonicalize returns `\\?\` verbatim paths on Windows that break
  external tools, leak into prompts, and poison path-equality keys.
- `Command::spawn` (std and tokio) and `portable_pty::SlavePty::spawn_command` — enroll the
  child with `xai_tty_utils::ProcessScope` so it dies with its session instead of outliving it.

Escape hatches use `#[allow(clippy::disallowed_methods)]` with a comment saying why it is safe
(the caller reaps it, the test kills it, it is deliberately detached). Follow that convention.

**Tests are often not in the file you are editing.** Alongside inline `#[cfg(test)] mod tests`,
this workspace uses sibling test files wired by an explicit path attribute:

```rust
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
```

So `foo.rs` may keep its tests in `foo_tests.rs` or `foo/tests.rs`. Grep both before assuming
a change is untested. `#[tokio::test]` is the async standard; `serial_test` guards env-var and
global-state tests. Snapshot tests (`insta`) exist only in the pager crate.

**Some source files exceed 200 KB** and `THIRD-PARTY-NOTICES` is over 700 KB. Use targeted grep
and offset reads rather than whole-file reads.

**`third_party/` is verbatim vendored** (the Mermaid stack). Do not reformat or lint-fix it; each
manifest opens with vendoring notes listing local patches to re-apply on upgrade, and the Apache
crates use British `LICENCE` spelling. See `third_party/README.md`.

**`.gitignore` only covers `/target`, `**/*.rs.bk`, and `.DS_Store`.** Scratch files, logs, and
editor directories show up in `git status` and are easy to commit by accident.

`clippy.toml` and several manifests reference a monorepo root config, `just lint-rs`, and Bazel
lint aspects. None of those exist in this tree — don't chase them.

## Conventions

Crates are named with an `xai-` prefix and live under `crates/codegen/` (the CLI closure),
`crates/common/` (shared leaf crates), `crates/build/`, and `prod/mc/`. See the layout table
in `README.md`.

`anyhow` with `.context()` for application and top-level flows; `thiserror` enums for library
error types. `tokio` for async, `clap` derive for CLI parsing, `ratatui` + `crossterm` for the
TUI, `serde` derive for wire types, `tracing` for logging. Most crate manifests opt into
`[lints] workspace = true` — keep new crates consistent.

Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`) with lowercase imperative
subjects. Never hand-author a `Synced from monorepo` commit; those belong to the sync bot.
`CONTRIBUTING.md` describes upstream SpaceXAI policy (no external PRs) and is inherited from
the monorepo — it does not describe this fork's own workflow.

## Where to look next

- `README.md` — install, build-from-source prerequisites, repository layout table
- `crates/codegen/xai-grok-pager/README.md` — TUI architecture and the Elm-style
  input → Action → Effect → state model
- `crates/codegen/xai-grok-shell/README.md` — agent runtime and product feature map
- `crates/codegen/xai-grok-pager/docs/user-guide/` — end-user behavior: configuration, MCP,
  hooks, skills, sandboxing, headless mode, project rules
- `third_party/README.md` — vendoring and upgrade procedure
- `SECURITY.md` — report vulnerabilities via HackerOne, never a public issue
