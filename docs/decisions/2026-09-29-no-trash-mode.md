# No Trash mode

- **Date:** 2026-09-29
- **PR:** `manual`
- **Status:** accepted

## Decision

Rosie deletes. There is no Trash mode: no `trash` config key, no `--trash` flag, and no
Trash operation in the FS layer.

## Rationale

The spec used to offer opt-in Trash, restorable with Finder's Put Back. It could not be
built consistently.

**Losing position 1: `/usr/bin/trash`.** It records Put Back and handles other volumes.
But it first shipped in macOS 15, while rosie supports macOS 11 and later. Run as root by
the elevated child, it sends items to `/var/root/.Trash`. The items that need sudo are
still the user's files, which only need root to get past permissions. chakrit: "it does
*not* belong to root and thus on \"put back\" it should definitely not be put back as root
user". The NSFileManager API that could trash as the user needs Objective-C FFI, which the
stack bans.

**Losing position 2: rename into `~/.Trash`.** It is fast and works under sudo, but it
records no original location, so Put Back cannot restore anything. It also fails across
volumes, which keep their own `.Trashes/<uid>`. Recovering by hand is not workable.
chakrit: "how would htat evne work for the user? how would i know which \"node_modules\" to
put back where". The spec also has no run log to look origins up in.

**Losing position 3: Trash for user items, and skip or delete sudo items in a Trash run.**
Skipping leaves a hole in the escape hatch. Deleting quietly during a Trash run defeats
its purpose. With no mechanism giving every item a working Put Back, chakrit dropped the
feature: "Rather, remove the trash feature entirely please." The plan and its
confirmation remain the safety net before anything is deleted.
