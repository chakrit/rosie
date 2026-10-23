# Safety

Status: accepted. Target behavior; not implemented.

## FS interaction layer

All filesystem, command, and sudo interaction goes through one layer. Everything is
forced through it; there are no exceptions.

- The security layer is absolute: every protection must pass on its own for an item to
  go through. The only exceptions are sanctioned escape hatches, such as rosie's own
  metadata methods and `allow_outside_roots`.
- The layer is a `Backend` trait: read_dir (each entry's name and the kind the listing
  reports), metadata via `lstat` (allocated size, dev and inode, file type, owner,
  `st_flags`), file reads for Lua, remove, commands, sudo, and the metadata methods
  below.
- `RealBackend` implements it for the machine; tests inject an in-memory fake
  ([testing.md](testing.md)).
- The layer's one concrete wrapper is `FsContext<B: Backend>`, mostly for security
  gates. Code only ever receives the context, never a bare backend. The context knows
  whether it runs as root.
- Every FS access is a typed, self-performing op, checked by its owner's policy. A sealed
  `FsOp` trait has `perform(self, &ctx)`, and each op returns its own type. Ops live in
  the owner namespaces `Cleanup`, `Rosie`, and `UserInput`, whose constructors are the
  only way to build one (e.g. `UserInput::read(path).perform(&ctx)`). Raw backend helpers
  are private to `fs`.
- Deletes are `Cleanup::delete(&path).perform(&ctx)`, with admission inside `perform`. An
  optional `.verify(&ctx)?` runs the same checks before side work such as a launchd
  bootout; `perform` re-checks.
- A plan file is read with `UserInput::read`, which follows links.
- System tools are a type holding only absolute paths (`/bin/ps`, `/usr/bin/sudo`, …).
- Commands (`run`, `sudo`) are context methods, not typed ops. *(agent-inferred)*
- Static generics, no `dyn`: `FsContext<B>`, `Scanner<B>`, `Runner<B>`, `App<B>`.
- The trait carries no `Send` or `Sync` bound. Only parallel functions add
  `where B: Sync`, using scoped threads; no `Send`, no `'static`.
- Roots (home, `/Library`, scan root, rosie's own folders) are injected, never read from
  `$HOME`.

### System tool versions

Before a process first runs a system tool, rosie reads the tool's version. A version
rosie was not tested with draws a warning naming the tool, the version found, and the
tested version; the command continues. Each tool is checked at most once per process,
and only when that process runs it.

| Tool                | Version read from                | Tested       |
|---------------------|----------------------------------|--------------|
| `/usr/sbin/lsof`    | `lsof -v`                        | 4.91         |
| `/bin/launchctl`    | `launchctl version`              | 7.0.0        |
| `/usr/bin/sudo`     | `sudo -V`                        | 1.9.17p2     |
| `/bin/bash`         | `bash --version`                 | 3.2.57       |
| `/bin/ps`           | macOS, `sw_vers -productVersion` | macOS 26.6.2 |
| `/usr/bin/plutil`   | macOS, `sw_vers -productVersion` | macOS 26.6.2 |
| `/usr/sbin/pkgutil` | macOS, `sw_vers -productVersion` | macOS 26.6.2 |
| `/usr/bin/id`       | macOS, `sw_vers -productVersion` | macOS 26.6.2 |

- A version that cannot be read draws a warning too. *(agent-inferred)*
- A tool rule's `shell` other than `/bin/bash` is not version-checked.
- `/usr/bin/id` is checked by the macOS version. *(agent-inferred)*
- A version matches only when equal to the tested one; newer or older warns.
  *(agent-inferred)*
- Warnings print when the command finishes, in the elevated child too.
  *(agent-inferred)*

### Cleanup targets

- Every filesystem mutation of a cleanup target is refused unless its path lies within
  the `roots` in `config.toml`, or `allow_outside_roots` is `true`.
- The context resolves `.` and `..` lexically before every check, then compares paths
  component by component (`Path::starts_with`), never by string prefix.
- The context refuses to delete a mount point in every mode.
- The roots check covers filesystem mutations only. Tool commands are outside it; the
  plan lists each for confirmation. Tools are safe by construction: rosie does not work
  their internals.
- A tool rule's `cmd` runs through the rule's `shell`; rosie never parses it
  ([rules.md](rules.md#tool-strategy)).

### Rosie's own data

- Separate metadata methods act on rosie's own data only: config read and write, and
  pack install, replace, and remove.
- They are confined to the injected `~/.config/rosie` and `~/.local/share/rosie`, not
  checked against `roots`. These folders get the same symlink ban as every other path.
- Creating an entry in rosie's own data refuses a case clash with an existing entry
  before the write. *(agent-inferred)*
- Pack tarballs are unpacked with the `tar` and `flate2` crates. Only regular files and
  folders are accepted; absolute paths, `..`, symlinks, and hardlinks are rejected.
  Entries are written through the metadata methods.
- Decision record:
  [FS layer metadata and pack unpacking](../decisions/2026-09-29-fs-layer-metadata-and-pack-unpacking.md).

## Roots

The `roots` allowlist in `config.toml` is the sole mutation authority. A plan never
carries or widens roots.

- Rosie flatly refuses any folder not inside an allowlisted root, unless
  `allow_outside_roots` is `true`.
- Every mode (`tree`, `caches`, `app`, `orphans`) is gated by `roots`, with no other
  exceptions.
- Rules and packs cannot add roots.
- A `roots` entry absent on disk is valid.
- `allow_outside_roots` in `config.toml` turns the roots check off. Default `false`.
- With `allow_outside_roots` on, every other protection still applies, and `"all"` follows
  links wherever they lead. *(agent-inferred)*
- Out-of-roots plan items show as `skip` with a `rosie roots add <path>`
  hint.
- A fresh `config.toml` is seeded with ordinary, editable `roots` entries: `/Applications`,
  `~/Applications`, the app-leftover folders under `~/Library` and `/Library` listed in
  [macos-filesystem-layout.md](../vendor/macos-filesystem-layout.md), and the tool cache
  folders named by the built-in pack. All are written as real, non-symlink paths (for
  example `/private/var/...`). No development roots are seeded.
- The first run writes the default `config.toml`, then checks its roots. A seeded root
  that fails on this machine fails the run like any other root, with its hints.
- `rosie roots …` and `rosie config …` need only a well-formed `config.toml`; they never
  check the roots against the disk.

## Symlinks

Rosie never accepts a path that passes through a symlink, and never substitutes a
resolved path for the one it was given. The ban is on what rosie accepts, not on how it
detects links: `canonicalize` and other link-resolving calls are the detector.

- Every written path (`roots` entries, rule fixed paths, typed paths, context checks) is
  made absolute by one call, `fs::Paths::resolve`: expand `~` against the injected home,
  normalize by text (`.`, `..`, `//`), then `canonicalize` through the `Backend`.
- A canonical form that differs from the normalized form means a symlink or a case or
  Unicode-form misspelling; the path is refused, naming the canonical path. There is no
  case folding.
- A path that does not exist canonicalizes its deepest existing ancestor and appends the
  rest.
- `rosie roots add` refuses a path with a symlink component. A `roots` entry with one
  fails every command that uses the roots, naming each such entry with its hints.
- A scan start folder or app argument with a symlink component is refused. Bare `rosie`
  without a folder uses the current directory, which is always the physical path.
- The walk never descends into a symlink. Symlinks never match rules.
- Fixed rule paths that pass through a symlink hit this ban and fail like any refused
  path ([rules.md](rules.md#paths)).
- Deleting a matched folder removes the symlinks inside it as links; their targets are
  untouched.
- A hand-written plan fed to `rosie run` is the user's own doing; the context compares
  its paths as written.
- Decision record:
  [symlinks and the path gate](../decisions/2026-09-29-symlinks-and-path-gate.md).

### `enter_links`

`[walk] enter_links` relaxes the ban in named levels; CLI `--enter-links=<level>`. At
`"roots"` and `"all"`, a path given through a link is worked on at its real location:
the roots check, the plan, and the delete all use the real location. Only `config.toml`
keeps the `roots` entry as written.

| Level              | Effect                                                            |
|--------------------|-------------------------------------------------------------------|
| `"none"` (default) | the ban above applies in full                                     |
| `"roots"`          | symlink components are allowed in `roots` entries, rule fixed     |
|                    | paths, and typed paths (scan folder, app argument, `roots add`);  |
|                    | the walk never follows a link                                     |
| `"all"`            | as `"roots"`, and the walk also follows links                     |

- `roots` entries are checked at the `enter_links` level set in `config.toml`, never at
  the CLI flag's. *(agent-inferred)*
- A path given through a link takes its real spelling; a misspelling elsewhere in it is
  not refused separately. *(agent-inferred)*
- At `"roots"`, a fixed rule path that is itself a link resolves to its target, and the
  target is planned. *(agent-inferred)*

At `"all"`:

- The walk follows a link only when its target is inside the roots. A link whose target
  is outside the roots is not followed, and rosie suggests `rosie roots add <target>`.
- A link onto another volume is followed only when its target is inside the roots and
  `enter_mounts` is set.
- Loops are detected and skipped. Dangling links are skipped.
- Two links reaching one target plan it once.
- Links inside a folder being deleted are still removed as links; their targets are
  untouched.
- A followed link's target is matched by its own real name, never the link's name. A
  link to a file is not followed. *(agent-inferred)*
- Skipped links are counted as links in the summary. *(agent-inferred)*

## Walk skips

By default the walk does not enter bundles, does not cross volumes, and does not open
dataless cloud placeholders. Configurable flags enable each riskier behavior; all default
off:

| `config.toml` `[walk]` | CLI                       |
|------------------------|---------------------------|
| `enter_bundles`        | `--enter-bundles`         |
| `enter_mounts`         | `--enter-mounts`          |
| `enter_placeholders`   | `--enter-placeholders`    |
| `enter_links`          | `--enter-links=<level>`   |

`enter_links` takes a level, not a boolean ([Symlinks](#enter_links)).

- The walk checks each entry in this order: symlink, placeholder, bundle, volume, then
  rules. A matched placeholder is never entered.
- A mount point is never planned, even with `--enter-mounts`.
- A mount point a scan matches is a mount walk skip, never a scan problem.
  *(agent-inferred)*
- Bundles are folders with these extensions: `.app`, `.framework`, `.bundle`, `.plugin`,
  `.xcarchive`, `.photoslibrary`, `.sparsebundle`, `.dSYM`.
- The walk `lstat`s an entry before any `readdir`. A dataless placeholder is detected by
  `SF_DATALESS` in `st_flags`, read via std `MetadataExt`.
- `EPERM` during the walk is logged as a walk skip.
- Skipped bundles, mounts, placeholders, and denied paths are logged so the user knows;
  the summary shows skip counts per reason.

## Running processes

- A delete, in every mode, is refused when a running process executes from inside its
  path. The check is one of the FS layer's delete checks.
- Detection takes one snapshot per scan or run with `lsof -nP -d txt -F pn`. Each
  reported path is matched against item paths, component by component.
- The snapshot is taken once per context. A context is made per scan and per run, so a
  run's user part and its after-elevation part share one snapshot; the elevated child
  takes its own as root. *(agent-inferred)*
- Decision record:
  [running-process check in the FS layer](../decisions/2026-10-04-running-check-in-fs-layer.md).

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
- The elevated child deletes root-owned items anywhere inside the roots. On the root side
  it never follows symlinks, never crosses mounts, and never chmods.
- A bootout that never ran is a skip, never a failure.
- A user folder whose elevated delete the child refused fails the run.
- The child reports each delete as it finishes.

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

It is followed by the hint `run it again without sudo:` with the same command,
`rosie clean …` ([cli.md](cli.md#errors)).
