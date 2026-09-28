# CLI

Status: accepted. Target behavior; not implemented.

## Modes

Each mode is its own subcommand. A separate mode is a subcommand, never a flag.

| Command                     | Cleans                                                     |
|-----------------------------|------------------------------------------------------------|
| `rosie clean project <dir>` | artifacts of one project                                   |
| `rosie clean tree <dir>`    | meta-scan over the fs-based modes under `<dir>`            |
| `rosie clean caches`        | tool-wide caches not tied to a project                     |
| `rosie clean app <X.app>`   | an application and its leftovers ([app.md](app.md))        |
| `rosie clean orphans`       | leftovers of apps no longer installed ([app.md](app.md))   |

- **caches** covers tool-wide caches not tied to a project: cargo registry, DerivedData,
  Docker, Homebrew, `~/Library/Caches`.
- **tree** applies `project` to every project root found, includes `caches` targets
  whose fixed path lies under the tree root, and never dispatches `app` mode.

## Bare `rosie`

Bare `rosie` auto-detects the mode:

- A `.git` is found → `project` clean of that project.
- Invoked in the home folder → `caches` plus `tree` over each allowlisted root
  ([safety.md](safety.md#roots)).

## Other commands

| Command                          | Does                                                      |
|----------------------------------|-----------------------------------------------------------|
| `rosie run <plan>`               | executes a plan file ([plan.md](plan.md))                 |
| `rosie run -`                    | executes a plan read from stdin; a public command         |
| `rosie rules pull [<source>]`    | installs a rule pack ([rules.md](rules.md#packs))         |
| `rosie roots add <path>`         | adds a path to the `roots` allowlist                      |
| `rosie config get <key>`         | prints a config value                                     |
| `rosie config set <key> <value>` | sets a config value, e.g. `rosie config set trash true`   |

The hidden `__elevated` subcommand is internal and refuses every invocation that does
not come from rosie itself ([safety.md](safety.md#elevation)).

## Flags

| Flag                   | Effect                                                          |
|------------------------|-----------------------------------------------------------------|
| `--aggressive`         | ticks aggressive items; `cmd_aggressive` replaces `cmd`         |
| `--trash`              | moves items to the macOS Trash for this run instead of deleting |
| `--enter-bundles`      | walk into bundles (overrides `[walk] enter_bundles`)            |
| `--enter-mounts`       | walk across volumes (overrides `[walk] enter_mounts`)           |
| `--enter-placeholders` | open dataless cloud placeholders (`[walk] enter_placeholders`)  |

## Configuration

User configuration is TOML at `~/.config/rosie/config.toml`. Keys defined so far:

| Key                         | Default     | Spec                                      |
|-----------------------------|-------------|-------------------------------------------|
| `roots`                     | seeded list | [safety.md](safety.md#roots)              |
| `trash`                     | `false`     | [safety.md](safety.md#deletion-and-trash) |
| `[walk] enter_bundles`      | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_mounts`       | `false`     | [safety.md](safety.md#walk-skips)         |
| `[walk] enter_placeholders` | `false`     | [safety.md](safety.md#walk-skips)         |

Rule overrides also live in configuration ([rules.md](rules.md#layers-and-overrides)).
