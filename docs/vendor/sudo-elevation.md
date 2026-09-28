<!-- derived from: sudo.ws sudo(8) manual @ https://www.sudo.ws/docs/man/sudo.man/, read 2026-09-28 -->

# sudo self-elevation patterns for CLIs

## Re-exec via `sudo` preserving args/env

- A CLI that needs root re-invokes itself as `sudo <argv0> <original-args...>`. This
  hands the password prompt to the real `sudo` binary, matching rosie's stated design
  ("let user type into regular sudo prompt so we don't have to worry ourselves about
  any security stuff").
- By default `sudoers` applies `env_reset`: the child process gets a **minimal**
  environment (`TERM`, `PATH`, `HOME`, `MAIL`, `SHELL`, `LOGNAME`, `USER`, `USERNAME`,
  plus `SUDO_*` and anything explicitly allow-listed via `env_keep`). Any environment
  variable rosie itself relies on (config file location overrides, `NO_COLOR`, etc.)
  will **not** survive re-exec unless passed with `sudo -E` (or `--preserve-env=VAR`) or
  allow-listed in `/etc/sudoers`.
- Rosie should not assume `-E`/`--preserve-env` is available or permitted — a locked-down
  `sudoers` policy can reject it outright. Prefer passing rosie's own config path
  explicitly as a CLI flag on re-exec rather than depending on an inherited env var.

## `SUDO_USER` / `HOME` handling

| Variable    | Meaning                                                             |
| ------------ | ---------------------------------------------------------------------- |
| `SUDO_USER`  | Login name of the user who invoked `sudo` (the real human, not root)  |
| `SUDO_UID`   | UID of that invoking user                                              |
| `SUDO_GID`   | GID of that invoking user                                              |

- Under default `env_reset`, `HOME` is set to the **target** user's (root's) home
  directory unless `set_home`/`always_set_home` is configured, or `-H` is passed
  explicitly (`-H` forces `HOME` to the target user's home — same as default behavior for
  root escalation, so effectively `HOME=/var/root` while elevated).
- **Rosie-relevant note:** while running elevated, `$HOME` refers to root's home, not the
  invoking user's. Any rosie logic that resolves `~/.Trash`, `~/Library`, or a rosie
  config file under the user's home must use `SUDO_USER`/`SUDO_UID` to look up the real
  user's home directory (e.g. via `getpwnam`), never bare `$HOME`, while running as root.
  This directly matters for the Trash design (see `macos-trash.md`) — trashing on behalf
  of the invoking user requires resolving their actual `~/.Trash`, not root's.

## `sudo -v` credential caching

- `sudo -v` validates and refreshes the cached credential/timestamp without running a
  command — useful for rosie to pre-warm the sudo prompt (e.g. right after the user
  confirms a plan containing root-required items) before doing scan/dry-run work, so the
  password prompt appears predictably rather than mid-execution.
- The credential cache is keyed **per terminal** (or per parent PID when there is no
  controlling terminal) and expires after a timeout (5 minutes by default, configurable
  via `timestamp_timeout` in `sudoers`). A rosie plan that takes longer than the cached
  window may prompt again mid-run unless rosie periodically calls `sudo -v` in the
  background during long executions, or executes the elevated portion of the plan as one
  contiguous `sudo` invocation up front.

## Pitfalls for rosie's design

- **Do not** silently swallow `sudo`'s exit code — a cancelled password prompt or wrong
  password must abort the plan run cleanly, not proceed as if elevation succeeded.
- **Do not** re-exec the *entire* rosie process under `sudo` for a plan that is mostly
  non-root work — this makes the whole run run as root unnecessarily (files it creates,
  logs it writes, etc. all become root-owned). Prefer isolating only the root-requiring
  plan steps (e.g. `/Library/LaunchDaemons`, `/Library/PrivilegedHelperTools`) into a
  privileged sub-invocation, keeping the interactive scan/plan/confirm UI running
  unprivileged.
- **Argv0 caveat:** re-exec must use an absolute path to rosie's own binary (resolve via
  `/proc/self/exe`-equivalent, i.e. `std::env::current_exe()` on macOS), not a bare
  program name, since `sudo`'s `PATH` under `env_reset` may differ from the invoking
  shell's `PATH` and could resolve to a different or missing binary.

Source: [sudo.ws — sudo(8) manual](https://www.sudo.ws/docs/man/sudo.man/), read 2026-09-28.
