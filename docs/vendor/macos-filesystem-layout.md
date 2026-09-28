<!-- derived from: Apple Developer docs (File System Programming Guide, App Sandbox design),
     community references cited inline @ 2026-09-28 -->

# macOS filesystem layout for cleanup

## `~/Library` and `/Library` subfolders

| Path                                     | Holds                                                          | Removal safety                                                              |
| ----------------------------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `~/Library/Caches`                        | Regenerable per-app cache data                                 | Safe once the owning app is quit; app rebuilds on next launch             |
| `~/Library/Application Support`           | App-specific data: settings, plugins, saved state, local DBs   | Only safe for apps that are fully uninstalled; may hold irreplaceable data |
| `~/Library/Preferences`                   | `.plist` files (user defaults, `cfprefsd`-managed)              | Deleting an in-use app's plist resets its settings; safe post-uninstall   |
| `~/Library/Containers`                    | Sandboxed (mostly App Store) apps' data, caches, prefs, support | Safe once app is uninstalled and orphaned; deletes any in-container docs  |
| `~/Library/Group Containers`              | Shared data between an app's extensions/helpers (App Groups)   | Same caution as Containers; shared across a group ID, not just one app    |
| `~/Library/Saved Application State`       | Window/tab restore state per app (`.savedState` bundles)        | Safe to delete; app just loses "reopen windows" state                     |
| `~/Library/HTTPStorages`                  | Per-app `NSURLSession`/`CFNetwork` HTTP cache and cookies        | Safe; regenerated, but clears cookies/sessions for that app               |
| `~/Library/WebKit`                        | WKWebView-based app storage (IndexedDB, localStorage, etc.)     | Safe post-uninstall; deletes in-app web data for apps still installed     |
| `~/Library/Logs`                          | App and system log files                                        | Safe to delete; may hinder debugging                                      |
| `~/Library/LaunchAgents`                  | Per-user background job `.plist` definitions                    | Must `launchctl unload`/`bootout` before removing, or job stays loaded    |
| `/Library/LaunchDaemons`                  | System-wide background job `.plist` definitions (root)          | Requires root; unload via `launchctl` first; wrong removal can break daemons |
| `/Library/PrivilegedHelperTools`          | Helper binaries installed via `SMJobBless`/`SMAppService`, root-owned | Requires root; tied to a LaunchDaemon plist; remove both together    |
| `/private/var/db/receipts` (`pkgutil`)    | `.pkg` install receipts (bom + plist) used by `pkgutil --forget` | Not disk-reclaiming; removing loses uninstall/upgrade tracking for that pkg |

Sources: [How-To Geek — What Is the macOS Library Folder?](https://www.howtogeek.com/what-is-the-macos-library-folder/),
[iBoysoft — Containers & Group Containers](https://iboysoft.com/wiki/mac-containers-folder.html),
read 2026-09-28. Apple's own canonical descriptions live in the (now largely archived)
*File System Programming Guide* and *App Sandbox Design Guide*; treat the above as
community-corroborated, not Apple-authoritative text.

**Rosie-relevant note:** none of these are SIP-protected by default (SIP protects system
paths like `/System`, `/usr` (except `/usr/local`), `/bin`, `/sbin`, and most of
`/Library/Apple*`), but `/Library/LaunchDaemons` and `/Library/PrivilegedHelperTools`
require root and interact with a running `launchd` — rosie's plan step for these should
unload the job before deleting, not just `rm`.

## TCC / Full Disk Access implications

- Terminal-launched CLIs inherit the **calling terminal app's** TCC grants, not their own.
  A process spawned from Terminal.app or iTerm2 is a child process; macOS TCC checks walk
  up to the responsible app (the terminal), so if the terminal has Full Disk Access, rosie
  running inside it does too. (Source:
  [lapcatsoftware.com — Every unsandboxed app has Full Disk Access if Terminal does](https://lapcatsoftware.com/articles/FullDiskAccess.html),
  read 2026-09-28.)
- Without FDA, reads of `~/Library/Application Support/com.apple.TCC`,
  `~/Library/Safari`, Mail, Messages, and other protected app data are silently denied
  (`EPERM`) even for the owning user. Plain `~/Library/Caches` and most
  `Application Support` subfolders are user-accessible without FDA.
- **Rosie design implication:** scan failures on individual paths should be reported as
  skipped/denied rather than aborting the whole scan, since FDA state varies by which
  terminal emulator launched rosie.
- Re-exec via `sudo` changes the effective UID but does **not** grant TCC access — TCC
  grants are per-app (by code-signature/bundle ID), not per-UID, so root re-exec of an
  unsigned or differently-identified binary can still be denied for protected paths.

## APFS specifics affecting size reporting

| Mechanism                     | Effect on `du`/size reporting                                                             |
| ------------------------------ | ------------------------------------------------------------------------------------------- |
| Clones (`cp -c`, `APFS clonefile`) | Share blocks; `du`/`stat` report full logical size per clone even though blocks are shared. Deleting one clone frees little/no space until all clones are gone. |
| Sparse files                   | Logical size can far exceed allocated blocks; `du` (block-count based) undercounts vs. `ls -l` (apparent size) overcounts. Use `du` for real disk impact. |
| Purgeable space                | Space held by cached/evictable data (e.g. iCloud-optimized content) counted as "available" by Finder but not always reclaimable by user deletion. |
| Local Time Machine snapshots   | `tmutil localsnapshot` snapshots retain deleted files' blocks. Deleting a file does not free space until the referencing snapshot(s) are thinned/expired — `du` walks live files only and is blind to snapshot-retained space, so actual freed space after rosie deletes something can be less than reported until snapshots roll off. |
| Firmlinks                      | Bind `/System/Volumes/Data` into the visible root; transparent to `du`/file APIs, no special handling needed for cleanup logic. |
| Hard links                     | `du` correctly discounts multiply-linked files it has already counted in the same run, but a naive per-path scanner that sums each target's `st_blocks` independently can double-count space that would not actually be freed by deleting only one target's rule. |

Sources: [Michael Tsai — Purgeable Disk Space](https://mjtsai.com/blog/2025/03/10/purgeable-disk-space/),
[eclecticlight.co — Aren't snapshots purgeable?](https://eclecticlight.co/2026/08/24/arent-snapshots-purgeable/),
read 2026-09-28.

**Rosie-relevant note:** rosie's plan-mode size estimates should be presented as
*estimated reclaimable, best case*, since APFS clones and local snapshots mean actual
freed space after `rosie run` can be smaller than the scanned total. Sizing logic should
sum `st_blocks` (not apparent size) and de-duplicate by inode within a single plan to
avoid double-counting hard-linked paths.
