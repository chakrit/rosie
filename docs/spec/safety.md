# Safety

Status: accepted. Target behavior; not implemented.

## FS interaction layer

All filesystem, command, sudo, and Trash interaction goes through one layer. Everything
is forced through it; there are no exceptions.

- The layer is a `Backend` trait: read_dir, metadata (allocated size, dev and inode,
  dataless flag), file reads for Lua, canonicalize, remove, Trash, commands, and sudo.
- `RealBackend` implements it for the machine; tests inject an in-memory fake
  ([testing.md](testing.md)).
- The roots gate is one concrete wrapper, `Gate<B: Backend>`. Code only ever receives
  the gate, never a bare backend.
- Static generics, no `dyn`: `Gate<B>`, `Scanner<B>`, `Runner<B>`, `App<B>`.
- The trait carries no `Send` or `Sync` bound. Only parallel functions add
  `where B: Sync`, using scoped threads; no `Send`, no `'static`.
- Roots (home, Trash, `/Library`, scan root) are injected, never read from `$HOME`.
- Every mutation is refused unless its canonical path lies within the plan's allowed
  roots.

## Roots

The `roots` allowlist is the sole mutation authority.

- Rosie flatly refuses any folder not inside an allowlisted root.
- Every mode (`project`, `tree`, `caches`, `app`, `orphans`) is gated by `roots`, with no
  exceptions.
- Rules and packs cannot add roots.
- Out-of-roots plan items show as blocked with a `rosie roots add <path>` hint.
- A fresh `config.toml` is seeded with the app-leftover and cache locations as ordinary,
  editable `roots` entries, plus `~/.Trash`. No development roots are seeded.

## Walk skips

By default the walk does not enter bundles (`*.app`, `*.photoslibrary`,
`*.xcarchive`), does not cross volumes, and does not open dataless cloud placeholders.
Configurable flags enable each riskier behavior; all default off:

| `config.toml` `[walk]` | CLI                    |
|------------------------|------------------------|
| `enter_bundles`        | `--enter-bundles`      |
| `enter_mounts`         | `--enter-mounts`       |
| `enter_placeholders`   | `--enter-placeholders` |

Skipped bundles, mounts, and placeholders are logged so the user knows.

## Running processes

Every plan, in every mode, refuses items that any process is executing from. Detection
parses `ps -axo pid=,ppid=,uid=,comm=`.

## Deletion and Trash

- Rosie deletes by default.
- Trash is opt-in: `trash = false` in `config.toml`, or `--trash` per run. Trash mode
  uses the plain macOS Trash; the user restores with Finder's own Put Back. It is a rare
  escape hatch; not freeing space immediately does not matter.
- A one-time first-run notice about the `trash` option appears beside the first auto
  pull.
- Under sudo, items go to the invoking user's `~/.Trash`.
- The safety layer checks both the source and the Trash destination against `roots`.
- There is no run log, no stash directory, and no `rosie restore`.

## Elevation

Rosie elevates only root items, once per run.

- Scan marks an item `sudo` when the user cannot delete it.
- Run does the user's items first, then pipes the remaining items to a rosie child
  launched through `sudo`, via stdin; there is no temp plan file.
- The elevated process loads the invoking user's config and roots via `SUDO_USER`,
  never `$HOME`, and runs through the same safety layer.
- If sudo is refused, those items are reported as skipped.

### Internal elevated entry

The root entry point is a hidden `__elevated` subcommand, gated four ways:

1. It refuses unless running as root.
2. Its parent must be `sudo`, whose parent is the same rosie binary.
3. Stdin must be a pipe.
4. Stdin starts with a versioned header and a nonce matching an argument.

Every other root invocation is refused.

### Refusing `sudo rosie`

A user running `sudo rosie …` directly is refused; it must not "just work". The refusal
rotates through these lines, chosen per run without stored state (for example by
process ID):

- `Jane! Stop this crazy thing!`
- `I swear on my mother's rechargeable batteries!`
- `rosie destroys things. It should never be run under sudo. Trust me, you don't know
  what you are doing.`

It is followed by a plain `run it again without sudo: rosie clean …` line.
