# Rules

Status: accepted. Target behavior; not implemented.

Cleanup targets are declarative TOML rules. Future developers can add their own rules for
new or unsupported tools and publish them independently of rosie.

## Rule tables

Rules are TOML tables named `[rules.<name>]`. Each rule has one `strategy`, and each
strategy has its own fields:

| `strategy` | Fields                                   | Does                                  |
|------------|------------------------------------------|---------------------------------------|
| `name`     | `target`                                 | cleans folders matched by name alone  |
| `marker`   | `target`, `marker` and/or `inside`       | cleans folders whose markers match    |
| `lua`      | `target`, exactly one of `expr`/`script` | cleans folders the Lua code accepts   |
| `path`     | `paths`                                  | cleans fixed paths                    |
| `tool`     | `cmd`, `cmd_aggressive`, `shell`, `env`  | runs the tool's own cleanup command   |

- `target` is one exact folder name.
- `paths` lists fixed paths, used by caches rules.

### Folder strategies

- `marker = [...]` lists sibling files; `inside = [...]` lists files within the folder.
  Both are any-of lists; globs are allowed. When both are set, both must match.
- A `lua` rule has exactly one of `expr` or `script`. `expr` is wrapped as
  `return (...)`; `script` is a full chunk. Both or neither is a load error naming the
  rule.

### Paths

- A fixed path is `/…` or `~/…`. It is made absolute by `fs::Paths::resolve`
  ([safety.md](safety.md#symlinks)); `roots` decides the rest.
- A fixed path that passes through a symlink (for example `/var` → `/private/var`) hits
  the symlink ban and fails like any refused path, unless `enter_links` allows it
  ([safety.md](safety.md#symlinks)). Pack authors write the non-symlink path, e.g.
  `/private/var/...`.

### Tool strategy

A tool rule invokes the tool's own cleanup command instead of reimplementing its
internals:

```toml
[rules.docker]
strategy = "tool"
cmd = "docker system prune --force"
cmd_aggressive = "docker system prune --all --volumes --force"

[rules.homebrew]
strategy = "tool"
shell = "/bin/zsh"
cmd = "brew cleanup"
env = { HOMEBREW_NO_ENV_HINTS = "1" }
```

- `cmd` and `cmd_aggressive` are strings. Rosie runs each as `<shell> -c <cmd>`, with
  `env` added to its environment.
- Rosie never parses or splits `cmd`.
- `shell` is an absolute path; it defaults to `/bin/bash`.
- `env` is a table of variables. Names are letters, digits, and `_`, not starting with
  a digit.
- `shell` and `env` apply to both `cmd` and `cmd_aggressive`.
- A missing tool fails normally and is reported.

### Aggressive twins

Every "what to clean" field has an `_aggressive` twin: `target_aggressive`,
`paths_aggressive`, `cmd_aggressive`. Without `--aggressive`, those items are listed
unticked; with it, they are ticked, and `cmd_aggressive` replaces `cmd`.

## Modes

Which modes use a rule is inferred from its shape:

| Rule shape             | Used by                                                   |
|------------------------|-----------------------------------------------------------|
| `target`               | `tree`                                                    |
| `paths`, `tool`        | `caches`; `tree` for fixed paths under the tree root      |

## Lua

Lua 5.4 ships in v1, through `mlua` with `vendored`, sandboxed with a read-only API:

| Function    | Purpose                    |
|-------------|----------------------------|
| `exists`    | test whether a path exists |
| `glob`      | match paths                |
| `read_json` | read a JSON file           |
| `read_toml` | read a TOML file           |
| `parent`    | parent directory of a path |

- `expr` and `script` see the candidate folder as the global `path`, and return a truthy
  value to match.
- A Lua error is reported against the rule, and that candidate does not match.
- There is no instruction limit.

## Names

- Rule identity is `pack/rule`. The default pack is `rosie`.
- Pack and rule names are ASCII: the first character is a letter, followed by letters,
  digits, `_`, or `-`; at most 64 characters.

## Packs

- No rules are compiled into the binary.
- A pack is a folder of `*.toml` rule files. The pack name is the repository name.
- The `rosie` pack lives in the public repository github.com/chakrit/rosie and is the
  CLI's default source.
- The `rosie` pack is `rules/*.toml` at the root of github.com/chakrit/rosie; the
  built-in rules live there.
- In the `rosie` pack, JetBrains caches are aggressive-only.
- A default config file, `config.toml`, at the root of github.com/chakrit/rosie is
  downloaded and used to seed a fresh user config.
- `rosie rules pull [<source>]` takes a GitHub `owner/repo` and downloads the pack as a
  tarball over HTTPS; git is not needed. Unpacking follows
  [safety.md](safety.md#rosies-own-data).
- Pack sources and names compare ignoring ASCII case.
- Pull is HTTPS only, with a 30 s total timeout and caps on download and unpacked size.
  Files are written create-new, and the install is crash-safe.
- Pull refuses a pack whose rules fail the loader the rule layers use.
- A pulled pack is replaced wholesale in `~/.local/share/rosie/packs/<owner>/<repo>/`.
- A stray folder under the packs folder stops loading.
- On first run with no pulled pack, rosie auto-runs `rules pull` from the default
  source, then proceeds normally. If that first pull fails offline, rosie stops with the
  reason.
- There is no auto-update. Pack age is the time since its pull; after 6 months rosie
  warns: "Your rules are x months old, consider rosie rules pull to update".
- `rosie rules remove <pack>` removes any pack, including `rosie`. Removing the last
  pack means the next run auto-pulls `rosie` again.

## Layers and overrides

Layers, later overriding earlier by rule name:

1. Pulled packs: `~/.local/share/rosie/packs/<owner>/<repo>/`
2. User rules: `~/.config/rosie/rules/`, the pack `user`

A later layer overrides a rule by its qualified name, e.g. `[rules."rosie/docker"]`. An
override replaces the whole rule. Rules cannot be disabled individually: a pack is used
whole or removed.

Rules and packs cannot add `roots` ([safety.md](safety.md#roots)).
