# FS layer metadata methods and pack unpacking

- **Date:** 2026-09-29
- **PR:** `manual`
- **Status:** accepted

## Decision

All filesystem interaction still goes through the one FS layer. Rosie's own data (config,
packs) uses separate metadata methods confined to `~/.config/rosie` and
`~/.local/share/rosie`. Pack tarballs are unpacked with the `tar` and `flate2` crates,
with rosie's own entry checks, and written through those methods.

## Rationale

**The problem.** A pack tarball can carry entry names such as `/Users/you/.zshrc`,
`../../.zshrc`, or a symlink followed by a file written through it. A naive extractor
writes those outside the pack folder. Pulled packs come from remote repositories, so
unpacking needs guarding. Std has no tar or gzip support.

**Losing position 1: unpack with the system `tar`.** macOS's `bsdtar` strips leading
`/` and refuses `..` and writes through symlinks by default, as long as `-P` is not
passed. That fits "prefer system CLIs". But its writes would bypass the FS layer, which
the spec says every filesystem interaction goes through, with no exceptions. The
in-memory fake would never see them. chakrit asked instead: "can we re-implement basic
tar rules?" The `tar` and `flate2` crates are pure Rust with no FFI. Reading entries
itself lets rosie accept only regular files and folders, and reject absolute paths, `..`,
symlinks, and hardlinks.

**Losing position 2: take rosie's own files out of the FS layer.** The gated methods
cannot write the pack. `~/.local/share/rosie` is not a root, and the `roots` gate exists
for cleanup targets, not rosie's own data. chakrit: "it can't write through the backend
tho as we're talking about rosie's metadata here not the actual file contents to be
deleted." Exempting config and pack writes from the layer would break its "no exceptions"
rule and hide rosie's own state from the fake. chakrit kept the rule and extended the
layer: "actually no, we allow that absolute fs layer constraint still but extend the fs
layer to have a special methods for rosie's own metadata?" Confining those methods to
rosie's two injected folders also backs up the tar entry checks.
