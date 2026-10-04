# Running-process check in the FS layer; app mode validates its bundle

- **Date:** 2026-10-04
- **PR:** `manual`
- **Status:** accepted

## Decision

"A running process executes from inside this path" is one of the FS layer's delete
checks, fed by one `lsof -nP -d txt -F pn` snapshot per scan or run, and app mode
validates its operation by running the bundle's own delete checks
(`Cleanup::delete(&bundle).verify(&ctx)`) before it lists any leftovers.

## Rationale

**The problem.** `rosie clean app /System/Applications/Notes.app` blocked the bundle
(outside roots, SIP) but still ticked Notes' `~/Library` data. chakrit: "if app can't be
deleted, it's related files aren't touched".

**Losing position 1: handle it as an app-mode case.** Two options were on the table:
(a) refuse the whole plan when the bundle is blocked, as a new app-mode check next to the
existing "app is running" refusal; (b) show the plan with every leftover blocked and
unticked. Both make the gate's verdict on one item change the treatment of other items,
which mixes two operations: producing the list (which files belong to this app) and
validating that the operation can run at all (can this app be cleaned). chakrit: "i think
we're muddying up the operation quite a bit here", and "do we have any validation step
for the modes right now or arey ou planning to just add yet another exception case to
account for this?"

Every mode already has that validation step, unnamed: the user's typed path goes
through the FS layer's `check_typed_path`, and its refusal fails the whole operation.
chakrit: "doesn't that bubble back up to the layer we're talking about?" So app mode's
validation is the FS layer's verdict on the operation's subject, the bundle, and needs no
new mechanism. Leftovers are then gated one by one like any mode's items.

**Losing position 2: keep the running check where it was.** The same rule had three
owners: scan's per-item `Refusals` (`src/scan/refusal.rs`), app mode's `refuse_running`
(`src/apps/app.rs`), and run's `running_process` (`src/run/mod.rs`). Left there,
`verify` on the bundle would not cover "app is running", and app mode would keep its own
exception. Moving the check into the FS layer makes it one verdict among roots, mounts,
and symlinks, and deletes all three owners.

**Losing position 3: file locks.** macOS does not lock a running app's files; a running
binary can be unlinked. A process holds a lock only when it chooses to, so locks cannot
detect "running".

**Losing position 4: keep `ps -axo comm`.** chakrit asked: "how can we be sure that ps is
reliable on all systems and we won't have to compat-chase?" It is not a compatibility
issue; `comm` is the wrong source. It shows `argv[0]` or a self-set process title, not
the kernel's executable path. On chakrit's machine on 2026-10-04, 74 of 683 processes
showed a bare name or a title (`tmux`, `nvim`, `go`, `postgres: chakrit bc_crm …`), so
deleting `/opt/homebrew/Cellar/tmux/3.7c` while tmux ran from it would have gone through.
`lsof -d txt` reports the kernel's paths (`/opt/homebrew/Cellar/tmux/3.7c/bin/tmux`),
including mapped libraries, and `-F` is its output mode meant for parsing. It ships with
every supported macOS and took 0.3 s for the whole system.

**Losing position 5: libproc `proc_pidpath`.** Apple's proper API for executable paths,
but it is FFI, which `stack.md` bans along with the `libc` crate.

**Losing position 6: a fresh `lsof` inside every delete.** Proposed after chakrit: "Fast
isn't a priority above safety tho." At 0.3 s per call it adds minutes to a large run and
complexity to every delete. chakrit: "You are overengineering this. simplify please". One
snapshot per scan or run was accepted ("Ok fine with me").

**Known limits, accepted.** Run as the user, `lsof` sees only that user's processes; the
elevated child takes its own snapshot as root. A process that starts after the snapshot
is not seen.
