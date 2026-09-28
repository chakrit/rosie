# Elevation without FFI

- **Date:** 2026-09-29
- **PR:** `manual`
- **Status:** accepted

## Decision

The hidden `__elevated` entry is gated without FFI:
- `id -u` for the root check;
- an ancestor process must be `sudo`;
- a piped stdin whose versioned header carries a nonce and the invoking user's home.

Rosie runs plain `sudo` with no special flags; if sudo fails, those items are reported as
skipped.

## Rationale

**Losing position 1: allow the `libc` crate.** The original gates needed facts std does
not expose:
- `geteuid` for "running as root";
- `getpwnam` to find the home folder from `SUDO_USER`;
- a real binary path (`proc_pidpath`) to prove the parent chain is "sudo, whose parent is
  the same rosie binary".

`libc` would provide all three, but the stack bans new FFI, and chakrit: "libc is a
problematic dependency. stay 1) please."

Without it, `ps comm` cannot prove "the same rosie binary": it prints bare names and
unresolved symlink paths. sudo 1.9.14 and later also run commands under a pseudo-terminal
by default (`use_pty`). That likely puts a second sudo process between rosie and the
child, so the strict parent check would refuse legitimate runs. The check therefore
loosens to "an ancestor is `sudo`". The nonce on a piped stdin is what proves rosie
started the child. The parent sends its own home in that header, so the child never
needs `getpwnam`.

**Losing position 2: `sudo -n` when no terminal is available.** It would turn a missing
password prompt into an explicit refusal. chakrit: "no trying to tingle with sudo flags,
just run regular sudo and if that fails, just fail normally." A sudo failure takes the
existing refused-sudo path: those items are reported as skipped.
