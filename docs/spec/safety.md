# Safety

Status: accepted. Target behavior; not implemented.

## FS interaction layer

All filesystem, command, and sudo interaction goes through one layer. Everything is
forced through it; there are no exceptions.

- The layer is a `Backend` trait: read_dir, metadata via `lstat` (allocated size, dev and
  inode, file type, owner, `st_flags`), file reads for Lua, remove, commands, sudo, and
  the metadata methods below.
- `RealBackend` implements it for the machine; tests inject an in-memory fake
  ([testing.md](testing.md)).
- The roots gate is one concrete wrapper, `Gate<B: Backend>`. Code only ever receives
  the gate, never a bare backend.
- Static generics, no `dyn`: `Gate<B>`, `Scanner<B>`, `Runner<B>`, `App<B>`.
- The trait carries no `Send` or `Sync` bound. Only parallel functions add
  `where B: Sync`, using scoped threads; no `Send`, no `'static`.
- Roots (home, `/Library`, scan root, rosie's own folders) are injected, never read from
  `$HOME`.

### Cleanup targets

- Every filesystem mutation of a cleanup target is refused unless its path lies within
  the `roots` in `config.toml`.
- The gate resolves `.` and `..` lexically before every check, then compares paths
  component by component (`Path::starts_with`), never by string prefix.
- The gate covers filesystem mutations only. Tool commands are outside it; the plan lists
  each for confirmation. Tools are safe by construction: rosie does not work their
  internals.

### Rosie's own data

- Separate metadata methods act on rosie's own data only: config read and write, and
  pack install, replace, and remove.
- They are confined to the injected `~/.config/rosie` and `~/.local/share/rosie`, not
  checked against `roots`.
- Pack tarballs are unpacked with the `tar` and `flate2` crates. Only regular files and
  folders are accepted; absolute paths, `..`, symlinks, and hardlinks are rejected.
  Entries are written through the metadata methods.
- Decision record:
  [FS layer metadata and pack unpacking](../decisions/2026-09-29-fs-layer-metadata-and-pack-unpacking.md).

## Roots

The `roots` allowlist in `config.toml` is the sole mutation authority. A plan never
carries or widens roots.

- Rosie flatly refuses any folder not inside an allowlisted root.
- Every mode (`tree`, `caches`, `app`, `orphans`) is gated by `roots`, with no
  exceptions.
- Rules and packs cannot add roots.
- Out-of-roots plan items show as blocked with a `rosie roots add <path>` hint.
- A fresh `config.toml` is seeded with ordinary, editable `roots` entries: `/Applications`,
  `~/Applications`, the app-leftover folders under `~/Library` and `/Library` listed in
  [macos-filesystem-layout.md](../vendor/macos-filesystem-layout.md), and the tool cache
  folders named by the built-in pack. All are written as real, non-symlink paths (for
  example `/private/var/...`). No development roots are seeded.

## Symlinks

Rosie never follows, resolves, or accepts a symlink. Nothing is canonicalized.

- `rosie roots add` refuses a path with a symlink component. A `roots` entry with one is
  a load error naming it.
- A scan start folder or app argument with a symlink component is refused. Bare `rosie`
  uses the current directory, which is always the physical path.
- Refusals name the real path to use instead.
- User-typed paths (`roots add`, scan folder, app argument) are checked component by
  component against the parent's directory listing, in the same pass as the symlink
  check. A case or Unicode-form mismatch is refused, naming the correctly spelled path.
  There is no case folding.
- The walk never descends into a symlink. Symlinks never match rules.
- Fixed rule paths that pass through a symlink hit this ban and fail like any refused
  path ([rules.md](rules.md#paths)).
- Deleting a matched folder removes the symlinks inside it as links; their targets are
  untouched.
- A hand-written plan fed to `rosie run` is the user's own doing; the gate compares its
  paths as written.
- Decision record:
  [symlinks and the path gate](../decisions/2026-09-29-symlinks-and-path-gate.md).

## Walk skips

By default the walk does not enter bundles, does not cross volumes, and does not open
dataless cloud placeholders. Configurable flags enable each riskier behavior; all default
off:

| `config.toml` `[walk]` | CLI                    |
|------------------------|------------------------|
| `enter_bundles`        | `--enter-bundles`      |
| `enter_mounts`         | `--enter-mounts`       |
| `enter_placeholders`   | `--enter-placeholders` |

- Bundles are folders with these extensions: `.app`, `.framework`, `.bundle`, `.plugin`,
  `.xcarchive`, `.photoslibrary`, `.sparsebundle`, `.dSYM`.
- The walk `lstat`s an entry before any `readdir`. A dataless placeholder is detected by
  `SF_DATALESS` in `st_flags`, read via std `MetadataExt`.
- `EPERM` during the walk is logged as a walk skip.
- Skipped bundles, mounts, placeholders, and denied paths are logged so the user knows;
  the summary shows skip counts per reason.

## Running processes

Every plan, in every mode, refuses items that any process is executing from. Detection
parses `ps -axo pid=,ppid=,uid=,comm=`. The `comm` path is matched as written against
item paths, component by component. Bare names and symlinked launches are not matched.

## Deletion

- Rosie deletes. There is no Trash mode, no run log, no stash directory, and no
  `rosie restore`.
- Rosie's delete never follows symlinks and makes user-owned read-only folders writable
  as it goes.
- Decision record: [no Trash mode](../decisions/2026-09-29-no-trash-mode.md).

## Elevation

Rosie elevates only root items, once per run.

- Scan marks an item `sudo` when it is not owned by the user.
- Run does the user's items first, then pipes the remaining items to a rosie child
  launched through plain `sudo` with no special flags, via stdin; there is no temp plan
  file.
- The elevated process gets the invoking user's home from the stdin header, loads that
  user's config and roots, never `$HOME`, and runs through the same safety layer.
- If sudo is refused or fails, those items are reported as skipped.

### Internal elevated entry

The root entry point is a hidden `__elevated` subcommand, gated four ways:

1. It refuses unless running as root, checked with `id -u`.
2. An ancestor process must be `sudo`.
3. Stdin must be a pipe.
4. Stdin starts with a versioned header and a nonce matching an argument. The header
   carries the invoking user's home.

Every other root invocation is refused.

Decision record: [elevation without FFI](../decisions/2026-09-29-elevation-without-ffi.md).

### Refusing `sudo rosie`

A user running `sudo rosie …` directly is refused; it must not "just work". The refusal
rotates through these lines, chosen per run without stored state (for example by
process ID):

- `Jane! Stop this crazy thing!`
- `I swear on my mother's rechargeable batteries!`
- `rosie destroys things. It should never be run under sudo. Trust me, you don't know
  what you are doing.`

It is followed by a plain `run it again without sudo: rosie clean …` line.
