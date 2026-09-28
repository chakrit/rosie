# Plan and run

Status: accepted. Target behavior; not implemented.

Cleanup runs in two phases that can also be invoked independently: scan produces a
plan, run executes it, like `terraform plan -out` followed by `terraform apply`.

## Plan file

- A plan is a TOML file with one entry per item: target, path, size, action.
- Entries are keyed by path and list every rule that matched that path.
- Nested matches collapse into the outer one.
- Tool rules with an identical trimmed `cmd` dedupe into one entry.
- Aggressive items appear unticked unless `--aggressive` is given
  ([rules.md](rules.md#aggressive-twins)).
- Items outside `roots` appear as blocked, with a `rosie roots add <path>` hint
  ([safety.md](safety.md#roots)).
- Scan marks an item `sudo` when the user cannot delete it
  ([safety.md](safety.md#elevation)).
- Walk skips are reported: the user is told what was skipped
  ([safety.md](safety.md#walk-skips)).

## Editing a plan

Users may delete entries from a plan before `rosie run`. Run never adds anything.

## Interactive confirmation

The interactive prompt is `[y/N/e]`. `e` opens a pre-ticked checklist; after the user
edits it, rosie re-shows the trimmed plan and re-prompts.

## Run

- Run re-validates each entry; a stale entry is skipped and reported.
- Run refuses items that any process is executing from
  ([safety.md](safety.md#running-processes)).
- Run does the user's items first, then elevates once for the remaining `sudo` items
  ([safety.md](safety.md#elevation)).

## Shell-script export

Rosie can write a plan as a shell script that the user inspects and runs with sudo
themselves. An `.sh` plan is export-only, for power users; rosie cannot run it.
