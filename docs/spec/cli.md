# CLI

Status: accepted. Target behavior; not implemented.

## Modes

Each mode is its own subcommand. A separate mode is a subcommand, never a flag.

| Mode              | Cleans                                                    |
|-------------------|-----------------------------------------------------------|
| `project [<dir>]` | artifacts of one project                                  |
| `tree [<dir>]`    | meta-scan over the fs-based modes under `<dir>`           |
| `caches`          | tool-wide caches not tied to a project                    |
| `app [<X.app>]`   | an application and its leftovers ([app.md](app.md))       |
| `orphans`         | leftovers of apps no longer installed ([app.md](app.md))  |

- **caches** covers tool-wide caches not tied to a project: cargo registry, DerivedData,
  Docker, Homebrew, `~/Library/Caches`.
- **tree** applies `project` to every project root found, includes `caches` targets
  whose fixed path lies under the tree root, and never dispatches `app` mode.
- `<dir>` defaults to the current directory.

## Scan and clean

| Command                    | Does                                                        |
|----------------------------|-------------------------------------------------------------|
| `rosie scan <mode> …`      | prints the TOML plan to stdout ([plan.md](plan.md))         |
| `rosie plan <mode> …`      | alias of `rosie scan`                                       |
| `rosie clean <mode> …`     | scan, confirm, run                                          |
| `rosie run <plan>`         | executes a plan file                                        |
| `rosie run -`              | executes a plan read from stdin; a public command           |

Save a plan with `rosie scan tree ~ > plan.toml`, or pipe it:
`rosie scan tree ~ | rosie run -`.

### Bare `rosie`

Bare `rosie` auto-detects the mode:

- A `.git` is found → `project` clean of that project.
- Invoked in the home folder → `caches` plus `tree` over each allowlisted root
  ([safety.md](safety.md#roots)).

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
| `rosie config set [<key> <val>]` | sets a config value, e.g. `rosie config set trash true`   |
| `rosie config unset [<key>]`     | deletes the key from `config.toml`; its default applies   |

- `rosie rules remove` removes any pack, including `rosie`. Removing the last pack means
  the next run auto-pulls `rosie` again ([rules.md](rules.md#packs)).

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

Flags for `scan`, `plan`, and `clean`:

| Flag                   | Effect                                                          |
|------------------------|-----------------------------------------------------------------|
| `--only <rule>`        | limits the run to that rule name in any pack; repeatable        |
| `--aggressive`         | ticks aggressive items; `cmd_aggressive` replaces `cmd`         |
| `--trash`              | moves items to the macOS Trash for this run instead of deleting |
| `--enter-bundles`      | walk into bundles (overrides `[walk] enter_bundles`)            |
| `--enter-mounts`       | walk across volumes (overrides `[walk] enter_mounts`)           |
| `--enter-placeholders` | open dataless cloud placeholders (`[walk] enter_placeholders`)  |
| `--sh`                 | `scan` / `plan` only: prints the plan as a shell script         |

Rule names for `--only` are listed by `rosie rules`.

## Configuration

User configuration is TOML at `~/.config/rosie/config.toml`. Keys defined so far:

| Key                         | Default     | Spec                                      |
|-----------------------------|-------------|-------------------------------------------|
| `roots`                     | seeded list | [safety.md](safety.md#roots)              |
| `trash`                     | `false`     | [safety.md](safety.md#deletion-and-trash) |
| `[walk] enter_bundles`      | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_mounts`       | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_placeholders` | `false`     | [safety.md](safety.md#walk-skips)         |
