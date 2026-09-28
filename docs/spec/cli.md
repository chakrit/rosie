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
- A `<dir>` or `<X.app>` argument whose path contains a symlink, or whose spelling does
  not match the disk, is refused ([safety.md](safety.md#symlinks)).
- Which rules each mode uses is inferred from the rule's shape
  ([rules.md](rules.md#modes)).

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

- `rosie run` does not ask for confirmation; the plan is the confirmation.
- `rosie clean` with no terminal fails with a usage error.

### Bare `rosie`

Bare `rosie` is `rosie clean tree .`.

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
- `rosie rules remove` removes any pack, including `rosie`. Removing the last pack means
  the next run auto-pulls `rosie` again ([rules.md](rules.md#packs)).
- `rosie roots add` refuses a path that contains a symlink or does not match the disk's
  spelling, naming the real path ([safety.md](safety.md#symlinks)).

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
| `--enter-bundles`      | walk into bundles (overrides `[walk] enter_bundles`)            |
| `--enter-mounts`       | walk across volumes (overrides `[walk] enter_mounts`)           |
| `--enter-placeholders` | open dataless cloud placeholders (`[walk] enter_placeholders`)  |
| `--sh`                 | `scan` / `plan` only: prints the plan as a shell script         |

Rule names for `--only` are listed by `rosie rules`.

Global flags are `--help` and `--version`. There is no `--verbose`.

## Exit status

Rosie exits non-zero when any item failed or the command errored.

## Configuration

User configuration is TOML at `~/.config/rosie/config.toml`. Keys defined so far:

| Key                         | Default     | Spec                                      |
|-----------------------------|-------------|-------------------------------------------|
| `roots`                     | seeded list | [safety.md](safety.md#roots)              |
| `[walk] enter_bundles`      | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_mounts`       | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_placeholders` | `false`     | [safety.md](safety.md#walk-skips)         |
