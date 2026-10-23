# CLI

Status: accepted. Target behavior; not implemented.

## Modes

Each mode is its own subcommand. A separate mode is a subcommand, never a flag.

| Mode            | Cleans                                                    |
|-----------------|-----------------------------------------------------------|
| `tree [<dir>]`  | everything the rules match under `<dir>`                  |
| `caches`        | tool-wide caches                                          |
| `app [<X.app>]` | an application and its leftovers ([app.md](app.md))       |
| `orphans`       | leftovers of apps no longer installed ([app.md](app.md))  |

- **tree** walks `<dir>` applying the folder rules, includes `caches` targets whose fixed
  path lies under `<dir>`, and never dispatches `app` mode.
- **caches** covers tool-wide caches: cargo registry, DerivedData, Docker, Homebrew,
  `~/Library/Caches`.
- `<dir>` defaults to the current directory.
- A folder given in place of a mode runs `tree` on it: `rosie scan ../hdiff` is
  `rosie scan tree ../hdiff`.
- A command or mode name always means the command or mode; `./app` names the folder.
- An `.app` given in place of a mode runs `tree` on it, like any folder.
- A word in place of a command or mode that names no command, mode, or folder is a usage
  error. Its hints list the close names, the folder form, and the other modes:

```text
$ rosie scna
✗ no command or folder named scna
  did you mean:
    rosie scan
  or to clean a folder:
    rosie <dir>
  or another mode:
    rosie clean caches
    rosie clean app
    rosie clean orphans
```

- A name is close when its Jaro similarity to the word is above 0.7, as clap suggests
  subcommands. *(agent-inferred)*
- A `tree` folder that does not exist reads `<dir> does not exist`. *(agent-inferred)*
- A `<dir>` or `<X.app>` argument whose path contains a symlink (unless `enter_links` is
  `roots` or `all`), or whose spelling does not match the disk, is refused
  ([safety.md](safety.md#symlinks)).
- Which rules each mode uses is inferred from the rule's shape
  ([rules.md](rules.md#modes)).

## Scan and clean

| Command                    | Does                                                        |
|----------------------------|-------------------------------------------------------------|
| `rosie scan <mode> …`      | prints the plan to stdout ([plan.md](plan.md))              |
| `rosie plan <mode> …`      | alias of `rosie scan`                                       |
| `rosie clean <mode> …`     | scan, confirm, run                                          |
| `rosie run <plan>`         | executes a plan file                                        |
| `rosie run -`              | executes a plan read from stdin; a public command           |

Save a plan with `rosie scan tree ~ > plan.toml`, or pipe it:
`rosie scan tree ~ | rosie run -`.

- `rosie scan` prints the plan as a listing when stdout is a terminal and as TOML
  otherwise; a format flag overrides that ([Flags](#flags)).
- `rosie run` does not ask for confirmation; the plan is the confirmation. It asks only
  about conflicts ([plan.md](plan.md#rule-conflicts)).
- `rosie clean` with no terminal fails with a usage error.

### Bare `rosie`

Bare `rosie [<dir>]` is `rosie clean tree [<dir>]`, flags included.

## Check

| Command               | Does                                           |
|-----------------------|------------------------------------------------|
| `rosie check <path>…` | says, per path, what a plan would do with it   |

- Per path: its status and the path, the path last, then its reason and hint on the
  lines below, under the path.
- Plan words for would-be entries: `clean`, `skip`, `conflict`. Scan
  report words otherwise: `in use`, `skipped`, `no rule`.
- Same flags as `scan`.
- Walks each given folder's subtree so its verdict equals the plan's.
- A folder containing targets gets a one-line summary pointing to `rosie scan`.
- A summary line's status is the most consequential inside: conflict, clean, skip.
- Exit 0 if any path would be cleaned, else 1.

Approved example:

```text
$ rosie check ~/src/app/node_modules ~/src/app/node_modules/react \
    ~/Library/Developer/Xcode/DerivedData ~/src/old/.venv \
    ~/Library/Caches/com.foo.Bar ~/src/api/target ~/src/api/target/debug ~/src/web \
    ~/Applications/Slack.app ~/.cargo/registry /Volumes/Backup/build \
    ~/src/app/src ~/Downloads/missing
clean     ~/src/app/node_modules
          rosie/node-modules (normal)
clean     ~/src/app/node_modules/react
          inside ~/src/app/node_modules (rosie/node-modules)
clean     ~/Library/Developer/Xcode/DerivedData
          rosie/xcode-derived-data (normal); needs sudo
skip      ~/src/old/.venv
          rosie/python-venv (aggressive); to clean it: --aggressive
skip      ~/Library/Caches/com.foo.Bar
          outside the roots; to allow just it:
            rosie roots add ~/Library/Caches/com.foo.Bar
          or to allow its folder:
            rosie roots add ~/Library/Caches
conflict  ~/src/api/target
          rosie/cargo-target (normal) vs alice/rust (aggressive, on target/debug)
conflict  ~/src/api/target/debug
          rosie/cargo-target (normal, on ~/src/api/target) vs alice/rust (aggressive)
clean     ~/src/web
          contains 3 items: 2 clean, 1.4 GB; 1 skip (aggressive), 210 MB; see rosie scan tree ~/src/web
in use    ~/Applications/Slack.app
          running: Slack (pid 4123)
skipped   ~/.cargo/registry
          through symlink ~/.cargo → ~/dotfiles/cargo; to allow it: rosie config set walk.enter_links roots
skipped   /Volumes/Backup/build
          another volume; to cross it: --enter-mounts
no rule   ~/src/app/src
          no rule matches it or anything inside
no rule   ~/Downloads/missing
          does not exist
```

- `check` takes no format flag. *(agent-inferred)*
- Lines go to stdout; scan problems go to stderr as `scan` prints them.
  *(agent-inferred)*
- Paths under home print as `~`. *(agent-inferred)*
- Paths and reasons start two spaces past the widest status. *(agent-inferred)*
- Exit 0 when any printed line's status is `clean`. *(agent-inferred)*
- The walk starts at home for a path under it and at `/` otherwise, judges each folder
  on the way down as a `tree` walk would, and walks the given path's subtree whole.
  *(agent-inferred)*
- A line's verdict, first match wins: an item a process runs from at or above the path,
  a plan entry at or above it, a walk skip at or above it, a folder at or above it that
  cannot be read, plan entries inside it, items refused or skipped inside it, then `no
  rule`. *(agent-inferred)*
- A folder on the way down that cannot be read makes the path `skipped` with the
  error's text. *(agent-inferred)*
- A path inside a plan entry takes the entry's status: `inside <entry> (<rules>)`.
  *(agent-inferred)*
- A `skip` line carries its hint inside an entry too.
  *(agent-inferred)*
- `needs sudo` follows the description on every line about an entry not owned by the
  user, inside lines included, before any hint. *(agent-inferred)*
- A line outside the roots lists the ways to allow it, one of which is enough: the entry
  itself (`to allow just it`) or the folder holding it (`or to allow its folder`). When
  that folder is `/`, only the entry is listed, as `to allow it`. The plan file, scan
  skip lines, and errors about a path outside the roots list the same.
- A conflict found by a `tree` or `caches` walk is at one path, so neither side names an
  `on <path>`, and a conflict nested in another, as in the example's `~/src/api/target`
  and `~/src/api/target/debug` lines, does not arise. *(agent-inferred)*
- A summary's counts list `conflict`, `clean`, `skip (aggressive)`, `skip (outside the
  roots)`, and `skip`, in that order, leaving out any that are zero.
  *(agent-inferred)*
- A folder with no plan entry inside but items refused or skipped inside is `in use`
  when any item runs, else `skipped`: `contains N in use, M skipped; see rosie scan tree
  <path>`, leaving out a count that is zero. It is never `no rule`. A path inside it that cannot be read counts as
  skipped. *(agent-inferred)*
- A link out of the roots that the walk skips sits where the link is, not where it
  leads. *(agent-inferred)*
- A path inside an item a process runs from reads `inside <item> (<rules>); running:
  <program> (pid <pid>)`. *(agent-inferred)*
- A skip names every flag that would enter it: `to enter it: --a --b`.
  *(agent-inferred)*
- A refused typed path is `skipped` with the refusal's own text; one through a symlink
  names the link and where it leads. *(agent-inferred)*
- A resolved path prints at its real location; a refused one prints as typed.
  *(agent-inferred)*
- An `.app` path is judged as `rosie scan app` would plan it, when nothing at or above
  it is claimed or skipped other than as a closed bundle; only the bundle's own line is
  printed. `orphans` is not consulted. An `.app` refused other than for a running
  process, or unreadable, reads `skipped` with the error's text; one refused for lying
  outside the roots adds the roots hints. *(agent-inferred)*

## Management commands

| Command                          | Does                                                      |
|----------------------------------|-----------------------------------------------------------|
| `rosie rules`                    | shows the rules, with each rule's pack                    |
| `rosie rules pull [<source>]`    | installs a rule pack ([rules.md](rules.md#packs))         |
| `rosie rules remove [<pack>]`    | removes a pulled pack                                     |
| `rosie roots`                    | shows the `roots` allowlist                               |
| `rosie roots add <path>`         | adds a path to the `roots` allowlist                      |
| `rosie roots remove [<path>]`    | removes a path from the `roots` allowlist                 |
| `rosie config`                   | shows every config key and its current value              |
| `rosie config get [<key>]`       | prints a config value                                     |
| `rosie config set [<key> <val>]` | sets a config value                                       |
| `rosie config unset [<key>]`     | deletes the key from `config.toml`; its default applies   |

- Example: `rosie config set walk.enter_bundles true`.
- The first run downloads the default archive once.
- `rosie rules remove` removes any pack, including `rosie`. Removing the last pack means
  the next run auto-pulls `rosie` again ([rules.md](rules.md#packs)).
- `rosie roots add` refuses a path that contains a symlink (unless `enter_links` is
  `roots` or `all`) or does not match the disk's spelling, naming the real path
  ([safety.md](safety.md#symlinks)).
- `rosie roots remove` matches an entry as written or at its real location, without a
  disk check. *(agent-inferred)*
- `rosie config set` refusals list the key's allowed values. *(agent-inferred)*

The hidden `__elevated` subcommand is internal and refuses every invocation that does
not come from rosie itself ([safety.md](safety.md#elevation)).

## Pickers

When a command that asks for an item is run without it, rosie shows a picker:

| Command                          | Picker shows                                          |
|----------------------------------|-------------------------------------------------------|
| `rosie roots remove`             | current roots                                         |
| `rosie rules remove`             | pulled packs                                          |
| `rosie config get` / `unset`     | config keys                                           |
| `rosie config set`               | config keys, then the key's allowed values            |
| `rosie clean app`, `scan app`    | installed apps (`/Applications`, `~/Applications`)    |

When stdin or stdout is not a terminal, rosie prints the usage error instead.

## Flags

Flags for `scan`, `plan`, `clean`, bare `rosie`, and `check`:

| Flag                     | Effect                                                         |
|--------------------------|----------------------------------------------------------------|
| `--only <pattern>`       | limits the run to the rules `<pattern>` matches; repeatable    |
| `--aggressive`           | aggressive items are `clean`; `cmd_aggressive` replaces `cmd`  |
| `--enter-bundles`        | walk into bundles (overrides `[walk] enter_bundles`)           |
| `--enter-mounts`         | walk across volumes (overrides `[walk] enter_mounts`)          |
| `--enter-placeholders`   | open dataless cloud placeholders (`[walk] enter_placeholders`) |
| `--enter-links=<level>`  | `none`, `roots`, or `all` (overrides `[walk] enter_links`)     |
| `--format=<format>`      | `scan` / `plan` only: `list`, `toml`, or `sh`                  |
| `--toml`                 | `scan` / `plan` only: shortcut for `--format=toml`             |
| `--sh`                   | `scan` / `plan` only: shortcut for `--format=sh`               |
| `--conflicts=<answer>`   | `scan`, `clean`, `run` only: answers every conflict `clean` or |
|                          | `skip`; no default                                             |

- `list` is the plan listing, `toml` the plan file, and `sh` the shell-script export
  ([plan.md](plan.md)).
- At most one of `--format`, `--toml`, and `--sh` is given. *(agent-inferred)*
- `--only`, `--enter-bundles`, and `--enter-placeholders` exist only on `tree`, `caches`,
  and `check`.
- Rule names for `--only` are listed by `rosie rules`.
- `--only` accepts wildcards.
- An `--only` pattern is `rule`, matched in any pack, or `pack/rule`.
- `--only` wildcards are `*` and `?`, as in rule markers.
- The pack side of an `--only` pattern ignores ASCII case, as pack names do.
- An `--only` pattern that matches no rule is an error.
- A mode's flags follow its name or folder: `rosie scan caches --aggressive`, not
  `rosie scan --aggressive caches`. *(agent-inferred)*
- Skip lines name the path and a short reason, with a flag hint only when that flag could
  change the outcome.

Global flags are `--help` and `--version`. There is no `--verbose`.

## Exit status

| Code | Meaning                                    |
|------|--------------------------------------------|
| `0`  | ok                                         |
| `1`  | an item failed or the command errored      |
| `2`  | usage error                                |

`rosie check` exits `0` when any path would be cleaned and `1` otherwise
([Check](#check)).

## Output

Every line rosie writes to stderr starts with a marker in the first column:

| Marker  | Line                        |
|---------|-----------------------------|
| `·`     | note                        |
| `✓`     | done                        |
| `✗`     | error                       |
| `!`     | warning                     |
| `-`     | skipped                     |
| spinner | work in progress            |
| `?`     | prompt                      |

- One module owns the markers and the palette; `inquire`'s render config takes the same
  palette.
- Color only when stderr is a terminal and `NO_COLOR` is unset. Markers always.
- The plan listing is colored only when stdout is a terminal and `NO_COLOR` is unset.
- Other stdout data (plan TOML, the `--sh` script, lists, `check` columns) stays plain.

## Errors

- An error line states the problem only. Its causes follow.
- Each hint follows on its own lines: a dim label, then the command in cyan, indented.
- A hint is a rosie command, a flag, or a manual step. Commands are shell-quoted.
- Every failure the user can fix by changing their setup, their config, or how they run
  rosie carries at least one hint.
- Failures the user cannot act on (the elevation protocol, plan files a scan never
  writes, pack archive tampering, Lua runtime errors, unexpected `launchctl` output)
  carry the problem line only.
- `rosie check` lines and plan TOML comments show one hint on the line. Several follow on
  the lines below in the stderr form, each line of a plan comment commented out.

## Configuration

User configuration is TOML at `~/.config/rosie/config.toml`. Keys defined so far:

| Key                         | Default     | Spec                                      |
|-----------------------------|-------------|-------------------------------------------|
| `roots`                     | seeded list | [safety.md](safety.md#roots)              |
| `allow_outside_roots`       | `false`     | [safety.md](safety.md#roots)              |
| `[walk] enter_bundles`      | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_mounts`       | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_placeholders` | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_links`        | `"none"`    | [safety.md](safety.md#symlinks)           |
