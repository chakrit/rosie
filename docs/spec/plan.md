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
its size (`size unknown` for `[[tool]]` and `[[forget]]`) and, when `outside-roots`,
the `rosie roots add <path>` hint; report entries have no comment. A launch job's bootout
sits inside the delete that holds its plist:

```toml
# rosie plan. Only entries with status = "clean" run.
# Delete any entry you do not want, then: rosie run <this file>
version = 1

# 4.1 KB; outside roots
# to allow just it:
#   rosie roots add /Library/LaunchDaemons/com.x.plist
# or to allow its folder:
#   rosie roots add /Library/LaunchDaemons
[[delete]]
path = "/Library/LaunchDaemons/com.x.plist"
type = "file"
size = 4096
rules = ["app/com.x"]
status = "outside-roots"
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
status = "clean"

# size unknown
[[tool]]
rule = "rosie/docker"
shell = "/bin/bash"
cmd = "docker system prune --force"
status = "clean"

# size unknown
[[tool]]
rule = "user/homebrew"
shell = "/bin/zsh"
cmd = "brew cleanup"
status = "clean"

[tool.env]
HOMEBREW_NO_ENV_HINTS = "1"

# size unknown
[[forget]]
package = "com.x.pkg"
status = "clean"

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
| `[[tool]]`            | `rule`, `shell`, `cmd`, `status`, `env` (present when the rule     |
|                       | sets one); runs `<shell> -c <cmd>` with `env` added                |
| `[[forget]]`          | `package` for `pkgutil --forget`, `status`                         |
| `[[report]]`          | `subject`, `steps`; report-only, never run                         |

- Plan status words are user-facing: `clean`, `keep`, `outside-roots`, `conflict`.
- `status` is `clean`, `keep`, `outside-roots`, or `conflict`; `[[tool]]` and
  `[[forget]]` are never `outside-roots` or `conflict`.
- Unknown keys are refused.

- A plan carries a format version.
- A plan is a TOML file with one entry per item: target, path, size, action.
- Path entries are keyed by path and list every rule that matched that path.
- Each `[[tool]]` entry is one rule's action, keyed by that rule, carrying `shell`,
  `cmd`, and `env` as the rule gave them.
- Rosie never compares, dedupes, merges, or conflict-checks tool commands.
- A plan always writes a `[[tool]]` entry's `shell`; a plan read back without it is
  refused. *(agent-inferred)*
- A `[[tool]]` entry's `rule` is not validated when a plan is read. *(agent-inferred)*
- Every found item goes through all delete checks: an item outside `roots` is
  `outside-roots`, a running process leaves it out of the plan (sized and reported), a
  mount point is a walk skip, and any other refusal is a scan problem. *(agent-inferred)*
- Nested matches collapse into the outer one. The outer entry is `sudo` if any inner one
  was.
- Aggressive items appear as `keep` unless `--aggressive` is given
  ([rules.md](rules.md#aggressive-twins)).
- Items outside `roots` appear as `outside-roots`, with a `rosie roots add <path>` hint
  ([safety.md](safety.md#roots)).
- `keep`, `outside-roots`, and `conflict` entries are kept in the plan but not executed.
- A plan never carries roots; run checks against the `roots` in `config.toml`.
- Scan marks an item `sudo` when it is not owned by the user
  ([safety.md](safety.md#elevation)).
- Walk skips are reported: the user is told what was skipped
  ([safety.md](safety.md#walk-skips)).
- When things are being skipped, the log message briefly explains why.

### Rule conflicts

- An entry is `conflict` when two or more rules' scan matches overlap (the same path, or
  one nested in another) and the rules disagree on tier. No rule's tier wins.
- The entry's comment names the rules and their tiers.
- `rosie run` and the interactive `y` refuse while any `conflict` entry remains.
- The user resolves each conflict to `clean` or `keep`, by editing the plan or
  through `e`.
- `rosie scan … --sh` refuses while any `conflict` entry remains.
- A rule never conflicts with itself: a rule whose normal and aggressive values match the
  same path counts as normal, and a rule matching inside a path it already matched keeps
  the outer match's tier. *(agent-inferred)*
- `--aggressive` does not hide a conflict. *(agent-inferred)*
- A conflict entry's `rules` lists only the outer path's own rules; rules from nested
  paths appear only in its comment. *(agent-inferred)*

## Stats

Plans and clean runs report saved space. Simple stats suffice. Stats are printed to
stderr, never written into the plan TOML.

- Scan and confirmation show the item count and total size of `clean` items, `keep`
  aggressive items, `outside-roots` items, and conflicts.
- The end of a run shows the total freed and the items done, plus skipped and failed
  items (stale, running process, sudo refused, errors) with their sizes, and walk-skip
  counts per reason.
- Sizes are allocated bytes; hardlinks are counted once.
- Tool items show "size unknown" in the plan and are excluded from totals.

## Editing a plan

Users may delete entries from a plan before `rosie run`. Run never adds anything.

- A plan with nested, repeated, or duplicate-bootout deletes is refused.
- Deletes are sorted by path on parse.

## Interactive confirmation

The interactive prompt is `[y/N/e]`. `e` first asks, for each `conflict` entry, whether
to clean or keep it; the user must choose one. It then opens a checklist with every
entry checked; after the user edits it, rosie re-shows the trimmed plan and re-prompts.

- A conflict pick has no default; cancelling any pick cancels the whole edit.
  *(agent-inferred)*
- The checklist shows a tool entry's `cmd` only. *(agent-inferred)*

## Run

- Run re-validates each entry; a stale entry is skipped and reported. An entry is stale
  when its path is missing or its `lstat` type changed.
- Run refuses items that any process is executing from
  ([safety.md](safety.md#running-processes)).
- Run checks staleness before the running-process check. *(agent-inferred)*
- Run output shows a tool's full command: `NAME=value` pairs, then `<shell> -c <cmd>`.
  *(agent-inferred)*
- A failed item is reported and the run continues.
- Run does the user's items first, then elevates once for the remaining `sudo` items
  ([safety.md](safety.md#elevation)).
- While a tool command runs, rosie shows a spinner with the command and a live rolling
  window of the last 3 lines of its output. When it finishes, those 3 lines stay as the
  item's result. When stderr is not a terminal, only the final 3 lines are printed.

## Shell-script export

`rosie scan … --sh` prints the plan as a shell script that the user inspects and runs
with sudo themselves. An `.sh` plan is export-only, for power users; rosie cannot run it.

- A `[[tool]]` entry is exported as `<shell> -c <cmd>`, preceded by
  `/usr/bin/env NAME=value …` when it has an `env`.
