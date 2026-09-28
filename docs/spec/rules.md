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
| `tool`     | `cmd`, `cmd_aggressive`                  | runs the tool's own cleanup command   |

`target` is the folder name.

### Folder strategies

- `marker = [...]` lists sibling files; `inside = [...]` lists files within the folder.
  Both are any-of lists; globs are allowed. When both are set, both must match.
- A `lua` rule has exactly one of `expr` or `script`. `expr` is wrapped as
  `return (...)`; `script` is a full chunk. Both or neither is a load error naming the
  rule.

### Tool strategy

A tool rule invokes the tool's own cleanup command instead of reimplementing its
internals:

```toml
[rules.docker]
strategy = "tool"
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
- `rosie rules remove <pack>` removes any pack, including `rosie`. Removing the last
  pack means the next run auto-pulls `rosie` again.
- Rule packs may add per-bundle-ID leftover paths for app cleanup ([app.md](app.md)).

## Layers and overrides

Layers, later overriding earlier by rule name:

1. Pulled packs: `~/.local/share/rosie/packs/<owner>-<repo>/`
2. User rules: `~/.config/rosie/rules/`
3. Project rules: `.rosie.toml`

A later layer overrides a rule by its qualified name, e.g. `[rules."rosie/docker"]`.
Rules cannot be disabled individually: a pack is used whole or removed.

Rules and packs cannot add `roots` ([safety.md](safety.md#roots)).
