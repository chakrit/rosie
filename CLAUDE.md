# PRODIGY9 Coding School

This project's AI coding environment is managed by
[ACE](https://github.com/ace-rs/ace). Run `ace` to start a coding session.
Run `ace setup` if not yet configured.

Skills and conventions are provided by the **PRODIGY9 Coding School** school and
are symlinked into `.claude/skills/`. Skill edits go through
symlinks into the school clone — propose changes back to the school repo
when ready. Run `ace config` or `ace paths` to debug configuration issues.

## Project

Rosie (named after the Jetsons' robot maid) is a macOS cleanup CLI for developers. It
scans for reclaimable developer artifacts, presents a plan, and executes it after
confirmation. Cleanup targets are data-driven rules so new tools can be supported without
rebuilding rosie. See `docs/spec/` for the design.

## Commands

- `cargo build` — build.
- `cargo test` — tests.
- `cargo clippy --all-targets --all-features` — must be clean before work is done.

## Conventions

- Rust stable, pinned in `rust-toolchain.toml`. Follow the `code`, `architect`, and
  `rust-coding` skills for all coding work.
- User-facing configuration is TOML.
