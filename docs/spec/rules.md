# Rules

Status: accepted. Target behavior; not implemented.

Cleanup targets are declarative TOML rules. Future developers can add their own rules for
new or unsupported tools and publish them independently of rosie.

## Rule tables

Rules are TOML tables named `[rules.<name>]`.

### Folder rules

A folder rule has a `target` (the folder name) and a `strategy` discriminator:

| `strategy` | Matches when                                                        |
|------------|---------------------------------------------------------------------|
| `name`     | the folder name alone matches                                       |
| `marker`   | `marker = [...]` sibling files and/or `inside = [...]` files match  |
| `lua`      | the Lua `expr` or `script` returns true                             |

- `marker` and `inside` are any-of lists; globs are allowed. When both are set, both
  must match.
- A `lua` rule has exactly one of `expr` or `script`. `expr` is wrapped as
  `return (...)`; `script` is a full chunk. Both or neither is a load error naming the
  rule.

### Tool rules

A tool rule invokes the tool's own cleanup command instead of reimplementing its
internals:

```toml
[rules.docker]
kind = "tool"
cmd = "docker system prune --force"
cmd_aggressive = "docker system prune --all --volumes --force"
```

### Aggressive twins

Any "what to clean" field may have an `_aggressive` twin (`cmd_aggressive`,
`paths_aggressive`, …). Without `--aggressive`, those items are listed unticked; with
it, they are ticked, and `cmd_aggressive` replaces `cmd`.

## Lua

Lua ships in v1, sandboxed with a read-only API:

| Function    | Purpose                    |
|-------------|----------------------------|
| `exists`    | test whether a path exists |
| `glob`      | match paths                |
| `read_json` | read a JSON file           |
| `read_toml` | read a TOML file           |
| `parent`    | parent directory of a path |

## Names

- Rule identity is `pack/rule`. The default pack is `rosie`.
- Pack and rule names: the first character is a letter or digit (UAX #31 `XID_Start` or
  a digit), followed by `XID_Continue`, `_`, or `-`; at most 64 characters. No
  whitespace, `/`, `.`, punctuation, symbols, or emoji. Checked with the `unicode-ident`
  crate.

## Packs

- No rules are compiled into the binary.
- The `rosie` pack lives in the public repository github.com/chakrit/rosie and is the
  CLI's default source.
- `rosie rules pull` downloads a pack as a tarball over HTTPS; git is not needed.
- A pulled pack is replaced wholesale in `~/.local/share/rosie/packs/<owner>-<repo>/`.
- On first run with no pulled pack, rosie auto-runs `rules pull` from the default
  source, then proceeds normally. If that first pull fails offline, rosie stops with the
  reason.
- There is no auto-update. When rules are old, rosie warns: "Your rules are x months
  old, consider rosie rules pull to update".
- Pull warns about rules from different packs that share the same target and detection.
- Rule packs may add per-bundle-ID leftover paths for app cleanup ([app.md](app.md)).

## Layers and overrides

Layers, later overriding earlier by rule name:

1. Pulled packs: `~/.local/share/rosie/packs/<owner>-<repo>/`
2. User rules: `~/.config/rosie/rules/`
3. Project rules: `.rosie.toml`

Override or disable a rule by its qualified name:

```toml
[rules."rosie/docker"]
enabled = false
```

Rules and packs cannot add `roots` ([safety.md](safety.md#roots)).
