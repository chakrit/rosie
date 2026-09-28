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

## Git checkpoints

Commit coherent, completed slices autonomously after completing the checks required for
that work. Do not ask for permission to make a local commit. A local commit does not
authorize pushing, publishing, merging, releasing, deploying, or any other change to
shared or external state; each requires separate user authorization.

## Durable artifacts

`docs/` holds this project's durable record.

**`docs/spec/` is authoritative — read the relevant spec before working, and comply.**
It owes no justification: a rule that departs from mainstream practice, from what you
would have picked, or from what you expected is not grounds to escalate, annotate, or
re-open it. If a spec is wrong, raise it with the user and amend the spec.

**Everything settled is a `docs/spec/` amendment** — an instruction you were given, an
approach that was agreed, a library that was picked, a convention or preference that
was fixed. Write it there as it was given: the rule, at the length it was given, with
no reason supplied and no note of what it was chosen over. A one-sentence rule — "use
RESTful routes" — is a complete entry. There is no decisions log.

File new material by the gate, first match wins: third-party lookup → `vendor/`; our
own design or surface → `spec/`; unsettled exploration → `scratch/` (last resort,
opened with a "not spec because ___" line). Nothing defaults to `scratch/`.

**Before writing under `docs/`, read `docs/README.md`, then the destination folder's
`README.md`.** The root file defines the routing gate. The folder file defines its
filing test, filename format, and lifecycle rules. Both are binding.

**`docs/spec/README.md` indexes every spec file — keep it current.** Read the index
before adding a spec file, so you amend the existing doc on a subject instead of
writing a second one. Adding, renaming, or retiring a file updates its row in the same
change.
