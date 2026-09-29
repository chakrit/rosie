# Plan and run

Status: accepted. Target behavior; not implemented.

Cleanup runs in two phases that can also be invoked independently: scan produces a
plan, run executes it, like `terraform plan -out` followed by `terraform apply`.
`rosie scan` (alias `rosie plan`) prints the TOML plan to stdout; `rosie clean` scans,
confirms, and runs; `rosie run <plan>` or `rosie run -` executes a saved or piped plan
([cli.md](cli.md#scan-and-clean)).

## Plan file

The plan file is `version = 1` followed by one table per entry, grouped by action;
deletes are in path order. Each entry except `[[report]]` is preceded by a comment with
its size (`size unknown` for `[[tool]]` and `[[forget]]`) and, when blocked, the
`rosie roots add <path>` hint; report entries have no comment. A launch job's bootout
sits inside the delete that holds its plist:

```toml
# rosie plan. Only entries with status = "ticked" run.
# Delete any entry you do not want, then: rosie run <this file>
version = 1

# 4.1 KB; blocked, outside roots. To allow it: rosie roots add /Library/LaunchDaemons/com.x.plist
[[delete]]
path = "/Library/LaunchDaemons/com.x.plist"
type = "file"
size = 4096
rules = ["app/com.x"]
status = "blocked"
sudo = true

[[delete.bootout]]
plist = "/Library/LaunchDaemons/com.x.plist"
domain = "system"

# 1.2 GB
[[delete]]
path = "/Users/me/src/app/node_modules"
type = "folder"
size = 1234567890
rules = ["rosie/node"]
status = "ticked"

# size unknown
[[tool]]
rules = ["rosie/docker", "user/docker"]
cmd = ["docker", "system", "prune", "--force"]
status = "ticked"

# size unknown
[[forget]]
package = "com.x.pkg"
status = "ticked"

[[report]]
subject = "login item X"
steps = ["Open System Settings > General > Login Items", "Remove X"]
```

| Table                 | Fields                                                             |
|-----------------------|--------------------------------------------------------------------|
| top level             | `version`: the format version; any other version is refused        |
| `[[delete]]`          | `path`, `type` (`file`/`folder`), `size` (allocated bytes),        |
|                       | `rules`, `status`, `sudo` (present and `true` when not owned by    |
|                       | the user)                                                          |
| `[[delete.bootout]]`  | `plist` (the delete's `path` or inside it), `domain` (`system` or  |
|                       | `gui/<uid>`); runs when its delete does, before any delete         |
| `[[tool]]`            | `rules`, `cmd` (argv), `status`                                    |
| `[[forget]]`          | `package` for `pkgutil --forget`, `status`                         |
| `[[report]]`          | `subject`, `steps`; report-only, never run                         |

- `status` is `ticked`, `unticked`, or `blocked`; `[[tool]]` and `[[forget]]` are never
  `blocked`.
- Unknown keys are refused.

- A plan carries a format version.
- A plan is a TOML file with one entry per item: target, path, size, action.
- Path entries are keyed by path and list every rule that matched that path. Tool
  entries are keyed by rule.
- Nested matches collapse into the outer one.
- Tool rules with an identical trimmed `cmd` dedupe into one entry.
- Aggressive items appear unticked unless `--aggressive` is given
  ([rules.md](rules.md#aggressive-twins)).
- Items outside `roots` appear as blocked, with a `rosie roots add <path>` hint
  ([safety.md](safety.md#roots)).
- Unticked and blocked entries are kept in the plan but not executed.
- A plan never carries roots; run checks against the `roots` in `config.toml`.
- Scan marks an item `sudo` when it is not owned by the user
  ([safety.md](safety.md#elevation)).
- Walk skips are reported: the user is told what was skipped
  ([safety.md](safety.md#walk-skips)).
- When things are being skipped, the log message briefly explains why.

## Stats

Plans and clean runs report saved space. Simple stats suffice. Stats are printed to
stderr, never written into the plan TOML.

- Scan and confirmation show the item count and total size of ticked items, unticked
  aggressive items, and blocked items.
- The end of a run shows the total freed and the items done, plus skipped and failed
  items (stale, running process, sudo refused, errors) with their sizes, and walk-skip
  counts per reason.
- Sizes are allocated bytes; hardlinks are counted once.
- Tool items show "size unknown" in the plan and are excluded from totals.

## Editing a plan

Users may delete entries from a plan before `rosie run`. Run never adds anything.

## Interactive confirmation

The interactive prompt is `[y/N/e]`. `e` opens a pre-ticked checklist; after the user
edits it, rosie re-shows the trimmed plan and re-prompts.

## Run

- Run re-validates each entry; a stale entry is skipped and reported. An entry is stale
  when its path is missing or its `lstat` type changed.
- Run refuses items that any process is executing from
  ([safety.md](safety.md#running-processes)).
- A failed item is reported and the run continues.
- Run does the user's items first, then elevates once for the remaining `sudo` items
  ([safety.md](safety.md#elevation)).
- While a tool command runs, rosie shows a spinner with the command and a live rolling
  window of the last 3 lines of its output. When it finishes, those 3 lines stay as the
  item's result. When stderr is not a terminal, only the final 3 lines are printed.

## Shell-script export

`rosie scan … --sh` prints the plan as a shell script that the user inspects and runs
with sudo themselves. An `.sh` plan is export-only, for power users; rosie cannot run it.
