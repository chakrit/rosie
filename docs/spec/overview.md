# Rosie overview

Status: accepted. Nothing here is implemented yet; every section is target behavior.

## What rosie is

- "Rosie" is inspired by the Jetsons' robot maid.
- A macOS cleanup CLI geared for developers.
- Supports Apple Silicon Macs on macOS 11 (Big Sur), the first Apple Silicon release, and
  later. That release is the starting point for the corpus of folders and tools rosie
  checks.
- Written in Rust, following the PRODIGY9 school skills and conventions.
- Everything should be fast, as per Rust standard. See
  [performance.md](performance.md).
- Rosie shows interactive progress where possible.
- Rosie is just a cleanup tool. It does not reason about how code is edited, activity,
  or git. `.git` may be used as an ordinary rule marker. Plan selection is decided by
  rules and the aggressive level only.

## Shape

- Cleanup runs in two phases: scan, which produces an inspectable plan (like terraform),
  and run, which executes it. The basic invocation scans, presents the plan, asks the
  user to confirm, and executes. See [plan.md](plan.md).
- Cleanup targets are data, not code: rules in TOML, downloaded as packs, editable and
  publishable independently of rosie. Built-in coverage spans standard coding-tool
  artifacts (`node_modules`, cargo `target/`, and similar), detected smartly rather than
  by name alone (check `Cargo.toml` before treating a folder named `target` for cleanup).
  See [rules.md](rules.md).
- Cleanup can be targeted: a specific directory, or specific cleanup groups (all
  `node_modules`, Docker, cargo targets). See [cli.md](cli.md).
- A special mode cleans a macOS application and its leftovers, in the manner of
  AppCleaner.app. See [app.md](app.md).
- Every filesystem mutation goes through one safety layer gated by a `roots` allowlist.
  Rosie elevates through the system `sudo` only for items that need root. See
  [safety.md](safety.md).

## Subject specs

| File                             | Covers                                                  |
|----------------------------------|---------------------------------------------------------|
| [cli.md](cli.md)                 | commands, modes, flags, config keys, exit codes         |
| [plan.md](plan.md)               | plan file, stats, confirmation, run, `.sh` export       |
| [rules.md](rules.md)             | rule format, detection strategies, packs, layering, Lua |
| [safety.md](safety.md)           | FS layer, roots, symlinks, walk skips, deletion, sudo   |
| [app.md](app.md)                 | `clean app` and `clean orphans`                         |
| [performance.md](performance.md) | parallel walk, sizing, delete                           |
| [stack.md](stack.md)             | toolchain, crates, FFI policy                           |
| [testing.md](testing.md)         | test tiers, fixtures, fakes, benchmark budgets          |
