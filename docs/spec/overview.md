# Rosie overview

Status: accepted. Nothing here is implemented yet; every section is target behavior.

## What rosie is

- "Rosie" is inspired by the Jetsons' robot maid.
- A macOS cleanup CLI geared for developers.
- Written in Rust, following the PRODIGY9 school skills and conventions.
- Everything should be fast, as per Rust standard.
- Rosie is just a cleanup tool. It does not reason about how projects are edited,
  project activity, or git. Plan selection is decided by rules and the aggressive level
  only.

## Shape

- Cleanup runs in two phases: scan, which produces an inspectable plan (like terraform),
  and run, which executes it. The basic invocation scans, presents the plan, asks the
  user to confirm, and executes. See [plan.md](plan.md).
- Cleanup targets are data, not code: rules in TOML, downloaded as packs, editable and
  publishable independently of rosie. Built-in coverage spans standard coding-tool
  artifacts (`node_modules`, cargo `target/`, and similar), detected smartly rather than
  by name alone (check `Cargo.toml` before treating a folder named `target` for cleanup).
  See [rules.md](rules.md).
- Cleanup can be targeted: a specific project directory, or specific cleanup groups
  (all `node_modules`, Docker, cargo targets). See [cli.md](cli.md).
- A special mode cleans a macOS application and its leftovers, in the manner of
  AppCleaner.app. See [app.md](app.md).
- Every mutation goes through one safety layer gated by a `roots` allowlist. Rosie
  elevates through the system `sudo` only for items that need root. See
  [safety.md](safety.md).

## Subject specs

| File                       | Covers                                                |
|----------------------------|-------------------------------------------------------|
| [cli.md](cli.md)           | commands, modes, flags, config commands               |
| [plan.md](plan.md)         | plan file, confirmation prompt, run, `.sh` export     |
| [rules.md](rules.md)       | rule format, detection strategies, packs, pull        |
| [safety.md](safety.md)     | FS interaction layer, roots, walk skips, Trash, sudo  |
| [app.md](app.md)           | `clean app` and `clean orphans`                       |
| [stack.md](stack.md)       | toolchain, crates, FFI policy                         |
| [testing.md](testing.md)   | test tiers, record-and-replay fixtures, fakes         |
