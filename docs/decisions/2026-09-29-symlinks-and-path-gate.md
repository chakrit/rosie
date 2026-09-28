# Symlinks and the path gate

- **Date:** 2026-09-29
- **PR:** `manual`
- **Status:** accepted

## Decision

Rosie never follows, resolves, or accepts a symlink. Every path that enters rosie from
outside the walk is refused if it passes through one. The `roots` gate compares paths as
written, component by component, after resolving `.` and `..` by text.

## Rationale

**Losing position 1: canonicalize every path before the roots check.** The earlier spec
checked each item's "canonical path" against `roots`. That fails both ways:
- A symlink inside a root that points outside it is refused, though deleting the link is
  harmless.
- Canonicalizing whatever comes in is the wrong fix for a symlink outside a root that
  points inside it.

Once the walk never descends into a link, walk-produced paths are already real, and
canonicalizing adds nothing for them. The remaining source of symlinked paths was a
hand-written plan, and chakrit ruled that out of scope: "\"hand-written plan fed to rosie
run\" is a user performing harakiri on his own fs".

**Losing position 2: resolve roots entries (at `roots add` or at every startup).** Two
entry points still admit links: the scan folder itself (`rosie tree ~/code/escape`, where
`escape` points to `/`) and a `roots` entry. Resolving at startup lets the approved area
move silently when a link is repointed. Resolving once at `roots add` pins it. chakrit
rejected resolution altogether: "Perhaps, rather, is it not better to simply refuse to add
or process any symlinks in roots? This feels like both a ux mishap and a security
confusion." Refusing at every entry point (roots, scan folder, app argument) means
nothing ever needs resolving, and each refusal names the real path.

**Losing position 3: an `enter_links` walk flag,** modelled on `enter_bundles`. It would
reintroduce paths through links, needing cycle detection and a resolved-location gate.
It also invites deleting through `npm link` / `pnpm link` into source checkouts elsewhere.
chakrit: "i'd consider rosie deleting node_modules deleting it in pnpm cache a failure of
the safety boundary tho, perhaps no." (pnpm's store itself is shared through hardlinks,
which a delete leaves intact; the danger is linked source checkouts.)

**Losing position 4: validate symlinked rule paths at pack load, or skip them with their
own report.** A fixed rule path such as `/var/...` passes through macOS's `/var` →
`/private/var` link. Whether a path crosses a link is a fact about each machine, not the
pack, so a load-time check is wrong. A dedicated skip is special handling. chakrit: "It
should just hit the regular symlink ban normally and fails and pack author should be aware
that the correct fix is to use the non-symlink path".

**Losing position 5: case-insensitive comparison, or accepting case mismatches as an
annoyance.** macOS's default disk format is case-insensitive, but APFS volumes can be
case-sensitive, and developers use such volumes for code. There, folding case would let
`~/Code` pass for a root of `~/code`. chakrit rejected living with the mismatch: "it's
definitely an \"annoyance\" and we target macos specifically (vs all *nix) for a reason
no?" Only typed paths can be misspelled, because walk output and the current directory
come from the disk. So the symlink check compares each typed component with its parent's
listing and refuses a mismatch, naming the correct spelling.

**Path traversal (`..`) is a separate hole from symlinks.** A pack path such as
`~/Library/Caches/../../Documents` passes an as-written check. With links refused,
resolving `.` and `..` by text at the gate is exact, and it covers every path source in
one place. A string prefix check would let `~/code2` pass for `~/code`, so the comparison
is component by component.
