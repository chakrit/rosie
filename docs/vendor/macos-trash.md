<!-- derived from: Apple NSFileManager documentation, trash-rs GitHub issue tracker,
     community sources cited inline @ 2026-09-28 -->

# Trash on macOS

## Trash locations

| Case                              | Location                              | Notes                                                                 |
| ----------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------ |
| User trashing on boot volume       | `~/.Trash`                              | Standard Finder-visible per-user trash on the startup disk              |
| User trashing on another volume    | `/Volumes/<name>/.Trashes/<uid>`        | Per-volume hidden trash, subfoldered by UID so multiple users share a volume safely and it survives disconnect/reconnect |
| Root/admin trashing via Finder     | `~/.Trash` of the **logged-in** user     | Finder prompts for the admin password but still places the item in the GUI user's own `~/.Trash` |
| Trashing as root via `osascript`/API (no logged-in GUI session context) | `/private/var/root/.Trash` | Lands in root's own trash, not any human user's — effectively invisible to the person who ran the command |

Source: [Apple Community — Where is trash bin of root on MacOS?](https://discussions.apple.com/thread/250458142),
[sindresorhus/macos-trash issue #11 — trash moves files to wrong user when run with elevated privileges](https://github.com/sindresorhus/macos-trash/issues/11),
read 2026-09-28.

**Rosie-relevant note:** if rosie self-elevates via `sudo` and then trashes a root-owned
file, the file will end up in root's `~/.Trash` (`/private/var/root/.Trash`), not the
invoking user's Trash — the user will not see it in Finder's Trash. Rosie should either
(a) trash root-owned items using `SUDO_UID`/`SUDO_USER` to target that user's own
`~/.Trash` explicitly via `mv` + ownership fixup, or (b) clearly report that root-owned
trashed items went to root's trash, not the user's.

## `NSFileManager.trashItem(at:resultingItemURL:)`

- Moves an item to the appropriate per-volume Trash (`~/.Trash` or `/Volumes/.../.Trashes/<uid>`)
  and returns the resulting URL of the trashed item via the `resultingItemURL` out
  parameter.
- Records Finder's "Put Back" metadata (original path) as part of the move, same as a
  Finder drag-to-trash — but only when performed as the file's owning user through the
  standard API path; API calls from a different privilege context may not populate this
  metadata correctly (see below).
- Requires the target volume to support a Trash directory; network/foreign filesystems
  without one will error and callers should fall back to `unlink`/permanent delete after
  warning the user.

## Rust crates for trashing

### `trash` crate (github.com/Byron/trash-rs)

- Cross-platform (`Windows`/`macOS`/`Linux` via `FreeDesktop.org` Trash spec).
- On macOS historically supported two delete strategies, tracked in
  [trash-rs issue #24 — Implement deletion using trashItemAtURL](https://github.com/Byron/trash-rs/issues/24):
  - **AppleScript/Finder** (`osascript` telling Finder to delete): triggers a macOS
    "would like to control Finder" permission prompt the first time, which is disruptive
    for a CLI and unsuitable for unattended/scripted runs.
  - **`NSFileManager.trashItem` (native)**: no Finder-automation permission prompt
    required and cannot be blocked by declining Automation access, but "Put Back" support
    is less reliable than the genuine Finder-driven move.
- Speed: native `NSFileManager` path is a single filesystem move (fast, same cost as
  `rename(2)` within a volume); the AppleScript/Finder path pays Apple Event IPC overhead
  per item and is materially slower for bulk operations — relevant since rosie's plans
  can include thousands of `node_modules` files.

### `ai-trash` (crates.io)

- A CLI-focused trash utility; found in search but not independently verified for its
  macOS backend choice in this pass — worth a closer look only if the `trash` crate's
  native path proves insufficient.

**Rosie-relevant note:** given rosie's "fast fast fast" requirement and default
non-interactive plan execution, prefer the `trash` crate configured for the native
`NSFileManager` backend, not the AppleScript/Finder backend — it avoids both the
Automation permission prompt and the per-item Apple Event overhead. Document in rosie's
own spec that "Put Back" is not guaranteed for items rosie trashes.
