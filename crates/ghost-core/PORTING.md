# Porting a `ghost_core` module to Rust

The Python daemon is the specification. A module ports with its tests, name for name, and the
rows, files and messages it produces stay the same, so the operator's database, folder and
vault open as they are and the words Ghost says do not change.

## Read first

1. `CLAUDE.md` at the repo root: the rules. They hold in Rust exactly as in Python.
2. The ADRs the module's docstring names, in `docs/adr/`.
3. The Rust already here, for the conventions: `models.rs` (the closed write model),
   `codec.rs` (the stored form), `store.rs` (SQLite, the same schema), `redact.rs`,
   `time.rs`, `source.rs` (the tool-source contract), `gate.rs`, `permissions.rs`,
   `policy.rs`.
4. The Python module in `packages/core/src/ghost_core/` and its tests in
   `packages/core/tests/` of the main checkout at `/home/detroitpro/projects/active/ghost`.

## Conventions

- Edition 2024. One module per Python module, same name. Public types and functions keep
  the Python names in Rust casing. Doc comments carry the Python docstrings' content.
- Errors: a `thiserror` enum per module; messages the operator reads stay word for word.
- Serde: the same JSON and TOML shapes as Python (field names, defaults, formats).
  Timestamps through `crate::time` (`iso` for rows, `time::json` for JSON, `parse_iso`
  for both, `utcnow` at microsecond precision).
- Anything stored, logged or handed on goes through `crate::redact` first, as the Python
  did. No value from the vault ever reaches a log line, an error, the audit or a test
  fixture committed to git.
- Logging with `tracing` (`warn!`, `info!`, `debug!`), the same wording as the loguru lines.
- Async with tokio; a trait that must be a trait object uses `async_trait`. HTTP with
  `reqwest` (already in the workspace deps: rustls, json, stream); test HTTP with `wiremock`.
  Files and folders in tests with `tempfile`.
- A dependency on a module not yet ported: define the small trait your module needs
  (what it reads, what it calls), take it as a parameter, and say so in your report. Do not
  port the other module.
- Dependencies: declare in the root `Cargo.toml` under `[workspace.dependencies]` and use
  `name.workspace = true` in `crates/ghost-core/Cargo.toml`. Prefer crates already there.
  `cargo deny check` must stay clean (licenses in `deny.toml`).
- Keep edits to the shared files (`Cargo.toml`, `lib.rs`) to the lines your module needs,
  so branches merge.

## Tests

Port every test of the module, by name, with the same cases. Then:

```bash
cargo fmt --all
cargo nextest run -p ghost-core
cargo clippy -p ghost-core --all-targets -- -D warnings
cargo deny check
```

All four clean before a commit.

## Branch protocol

Work in your own worktree on a branch from `rust-daemon`:

```bash
git -C /home/detroitpro/projects/active/ghost worktree add \
  /home/detroitpro/projects/active/ghost-w2-<module> -b w2/<module> rust-daemon
```

Commit on that branch with a conventional message (`feat(core): port <module>`) ending in
`Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`. Do not push, do not merge, do not
touch the Python packages or anything outside `crates/` and the two shared files. Report
the branch, the commit, the test count, and anything not ported with why.

## Never

Add an undo path. Log or store a secret. Read a webhook body before its signature passes.
Narrow or widen the permissions per agent. Let a model's text change a permission, a rule or
a prompt. Make Ghost ask by default.
