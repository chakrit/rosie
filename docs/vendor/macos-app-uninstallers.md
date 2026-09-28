<!-- derived from: freemacsoft.net/appcleaner (site + release notes), github.com/alienator88/Pearcleaner @ commit 7724df7111bff82ae243301cf701992ef05ecf19 (main), nektony.com/help/app-cleaner, macpaw.com/support/cleanmymac-x, various community uninstaller repos, and Apple man pages (launchctl, pkgutil, systemextensionsctl) via ss64.com mirrors — all read 2026-09-28 -->

# macOS app uninstaller family: how leftovers are found and removed

Rosie's `rosie clean SomeApp.app` mode plays in the same space as AppCleaner and its
peers. This is a survey of how that family works, to inform rosie's own rule design.
Confidence varies a lot by source — Pearcleaner is read from its own code (high
confidence); AppCleaner and CleanMyMac are closed source, so their entries are
community-sourced and marked as such.

## AppCleaner (FreeMacSoft) — closed source, low confidence

No official "files we search" document was found; everything below is inferred from
release notes (official, higher confidence) and third-party write-ups (lower
confidence, marked).

- Primary matching signal (third-party source, mole.fit blog): the app's reverse-DNS
  **bundle identifier**, matched against filenames the way macOS itself names things
  (`~/Library/Preferences/com.vendor.app.plist`, container folder names, Saved
  Application State, WebKit/HTTPStorages entries). Cited weakness: this misses
  vendor-named folders that don't embed the bundle ID, e.g.
  `~/Library/Application Support/VendorName/`.
- Release notes (official, freemacsoft.net/appcleaner/releasenotes.html) confirm an
  evolving "search engine" with no published algorithm, plus:
  - v3.6.8 (2023): "Allow searching for related files of system apps."
  - v3.1 (2015): "Stock Apple apps are now always protected on El Capitan due to SIP" —
    i.e. AppCleaner partly relies on System Integrity Protection itself as a backstop
    against destructive matches on system apps, rather than only its own exclusion list.
  - v3.7: "Improved search for related files in 'Application Scripts'."
- Has a standalone orphan-scan mode for apps no longer installed, and marketing copy
  mentions "smart search" and developer-name matching, but no technical description of
  either was found.
- No information available on: fuzzy/Levenshtein matching, an explicit shared-framework
  exclusion list, launch agent/daemon unloading, privileged helper tools, or pkg
  receipts.

## Pearcleaner (alienator88, Swift, open source) — read from source, high confidence

Read from `alienator88/Pearcleaner` @ `7724df7111bff82ae243301cf701992ef05ecf19`
(`main`). This is the most reusable design reference rosie has, since it's the only
tool in the family whose matching logic can be inspected directly.

### Search locations — installed-app mode (`Locations.apps.paths`)

User-domain:

| Path                                                              | Depth |
|--------------------------------------------------------------------|-------|
| `~/Library`                                                         | 2     |
| `~/Library/Application Scripts`                                     | 1     |
| `~/Library/Application Support` (+ every dynamically discovered subfolder) | 1 |
| `~/Library/Application Support/CrashReporter`                       | 1     |
| `~/Library/Containers`                                               | 1     |
| `~/Library/Group Containers`                                         | 1     |
| `~/Library/Caches` (+ known SDK subfolders: Sparkle, Crashlytics, Google Keystone/SoftwareUpdate, Segment, SentryCrash, Rollbar, Amplitude, Realm, Parse) | 1 |
| `~/Library/HTTPStorages`                                             | 1     |
| `~/Library/Internet Plug-Ins`                                        | 1     |
| `~/Library/LaunchAgents`                                             | 1     |
| `~/Library/Logs`, `~/Library/Logs/DiagnosticReports`                 | 1     |
| `~/Library/Preferences`, `~/Library/Preferences/ByHost`               | 1     |
| `~/Library/PreferencePanes`                                          | 1     |
| `~/Library/Saved Application State`                                  | 1     |
| `~/Library/Services`                                                 | 1     |
| `~/Library/WebKit`                                                   | 1     |
| `~`, `~/.config`, `~/Documents`, `~/Desktop`, `~/Applications`        | 1     |
| `~/Library/Application Support/Steam/steamapps(/common)` (Steam special case) | 1 |

System-domain (mirrors the user list where applicable):

```
/Applications, /Users/Shared, /Users/Library,
/Users/Shared/Library/Application Support,
/Library (depth 2), /Library/Application Support,
/Library/Application Support/CrashReporter, /Library/Caches,
/Library/Extensions, /Library/Internet Plug-Ins,
/Library/LaunchAgents, /Library/LaunchDaemons, /Library/Logs,
/Library/Logs/DiagnosticReports, /Library/Preferences,
/Library/PrivilegedHelperTools, /private/var/db/receipts, /private/tmp,
/usr/local/{bin,etc,opt,sbin,share,var}
```

`~/Library` and `/Library` are walked two levels deep; everything else is one level.
Depth 2 is how it reaches vendor subfolders without a bundle-ID filename, e.g.
`/Library/Objective-See/LuLu`. No `.kext` scanning is implemented. Login items are
handled only through the `ServiceManagement` API for Pearcleaner's own helper — the
tool does not enumerate other apps' login items via filesystem scan.

### Search locations — orphan mode (`Locations.reverse.paths`)

Used when scanning for leftovers of an app that is already uninstalled (no `.app`
bundle to read identity from). Non-recursive, one level per directory:

```
~/Library/Application Scripts, ~/Library/Application Support,
~/Library/Application Support/Caches,
~/Library/Application Support/com.apple.sharedfilelist/...ApplicationRecentDocuments,
~/Library/Containers, ~/Library/Caches, ~/Library/HTTPStorages,
~/Library/Internet Plug-Ins, ~/Library/LaunchAgents, ~/Library/Logs,
~/Library/Preferences, ~/Library/PreferencePanes, ~/Library/Preferences/ByHost,
~/Library/Saved Application State, ~/Library/WebKit,
/Users/Shared/Library/Application Support,
/Library/Application Support, /Library/Application Support/CrashReporter,
/Library/Internet Plug-Ins, /Library/LaunchAgents, /Library/LaunchDaemons,
/Library/PrivilegedHelperTools
```

Every candidate found here is discarded if it matches any **currently installed** app
(by bundle ID, name, entitlements, or container-UUID mapping) — that cross-check is the
actual orphan-detection safeguard, not the path list itself.

### Matching rules, in ascending "sensitivity" (Strict / Enhanced / Deep, user-selectable)

| Signal                                    | Strict | Enhanced | Deep |
|--------------------------------------------|:------:|:--------:|:----:|
| Bundle ID substring match                  | yes    | yes      | yes  |
| App name exact match                       | yes    | —        | —    |
| App name substring match                   | —      | yes      | yes  |
| `.app` filename (minus extension) match     | yes    | yes      | yes  |
| Letters-only app name (strips digits)      | yes    | yes      | yes  |
| Bundle ID last-two-components (`company``app`) | —  | yes      | yes  |
| Developer/company name (middle of 3-part bundle ID) | — | —  | yes  |
| Code-signing team ID                       | —      | —        | yes  |
| Base bundle ID (strips helper/agent/daemon/xpc/updater/installer/uninstaller/login/extension/plugin suffixes) | yes | yes | yes |
| Version-stripped app name (regex strips trailing digits, e.g. "Bartender 6" → "Bartender") | — | yes | yes |
| Code-signing entitlement strings           | exact  | substring| substring |

A bundle ID is only used as a match key if it has multiple dot-separated components,
or a single component of at least 5 characters — this guards against short generic
tokens matching too much. Company/team-ID/entitlement matching only activates at Deep,
trading false-positive risk for recall.

Special cases: web-apps match bundle-ID-only (skips name heuristics); Steam games
match via `appmanifest_<id>.acf` and the launcher's `run.sh`. A supplemental Spotlight
query (`NSMetadataQuery`, 5s timeout, 500-result cap, scoped to the user's home) runs
after the manual walk to catch anything the directory list missed.

### False-positive safeguards

- **Per-app override table** (~25 entries keyed by bundle ID): explicit `include`/
  `exclude` keyword lists and forced extra paths, mainly to cross-exclude apps whose
  names collide (Xcode vs. third-party Xcode-cleaner tools, Chrome vs. iTerm, VS Code
  vs. VS Code Insiders, Firefox vs. Thunderbird).
- **Global skip list**: never touches `~/.Trash`, `/Library/SystemExtensions`, or
  Apple/Chrome native-messaging password-manager helper paths; skips
  `mobiledocuments`/`reminders`/`.DS_Store`/password-manager prefixes.
- **Deep-search denylist** (~90 entries): standard `~/Library` and `/Library`
  subdirectory names (Mail, Messages, Photos, Keychains, Safari, Contacts, `com.apple.*`
  service bundles) that the depth-2 walk never descends into.
- **Orphan-scan denylist** (~150 keywords, e.g. `apple`, `icloud`, `homebrew`,
  `sparkle`, `chromium`, `python`): applied only in orphan mode, on top of the
  cross-check against installed apps.
- **UUID-named item skip**: bare 32-hex-digit container folder names are skipped unless
  otherwise resolved.
- **Path collapsing**: once a parent directory is matched, its already-covered children
  are removed from the result so nothing is listed twice.
- User-configurable permanent exclusion list, applied in both modes.

### Launch agents/daemons

Handled as a **separate manual feature**, not auto-invoked while deleting a regular
app's leftovers. Status comes from running `launchctl list` (user) and
`sudo launchctl list` (system) and cross-referencing on-disk plists. Actions map to
literal `launchctl` subcommands — notably the **older** verbs, not `bootstrap`/
`bootout`:

| Action     | Command                                                                 |
|------------|--------------------------------------------------------------------------|
| load       | `launchctl load '<path>'` (or `launchctl enable <domain>/<label>`)       |
| unload     | `launchctl unload '<path>'` (or `launchctl disable <domain>/<label>`)    |
| kickstart  | `launchctl kickstart -k <domain>/<label>`                                |
| remove     | `launchctl remove <label>`                                               |

System-domain items are prefixed with `sudo` and use domain `system`; user-domain items
use `gui/<uid>`. A confirmation prompt is shown before unloading.

### Privileged helper

Pearcleaner ships its own helper (`com.alienator88.Pearcleaner.PearcleanerHelper`)
installed/removed via the **modern** `ServiceManagement.SMAppService.daemon(plistName:)`
API (`register()`/`unregister()`), not the deprecated `SMJobBless`. It talks to the
helper over `NSXPCConnection` for privileged file operations. This only manages
Pearcleaner's own helper — it does not enumerate or manage other apps' helpers or login
items as part of leftover search.

### Package receipts

Uses Apple's private PackageKit framework (`PKReceipt.receiptsOnVolume`) rather than
shelling out to `pkgutil`, falling back to reading
`/var/db/receipts/<packageId>.plist` directly if needed. This feeds a separate
"installed packages" view, not the app-leftover deletion flow.

## Nektony "App Cleaner & Uninstaller" — closed source, category-level only

Support docs (nektony.com/help/app-cleaner) describe categories removed per app, no
exhaustive path list: Preferences, Caches, Logs, Application Support, Containers,
Cookies, Crash Reports, Settings Panes (System Settings plugins), Login Items (agents/
daemons/startup items), Dock icon, Plugins. Explicitly excludes user documents/photos/
downloads. Default search roots: `/Applications`, `~/Applications`,
`/System/Applications`, plus user-added custom locations (their UI has a "Locations"
tab for apps that store files outside the standard folders).

Nektony also publishes `github.com/Nektony/macos-uninstall-scripts` — one-off shell
scripts (currently just Python) targeting `.bom`/`.plist` receipts under
`/private/var/db/receipts`, `/Library/Frameworks`, `/private/var/folders` caches, and
`~/Library/Python`. Not a general library; no reusable SDK ("LSKit" or similar) was
found published by Nektony.

## CleanMyMac (MacPaw) uninstaller — closed source, no public technical detail

Support page (macpaw.com/support/cleanmymac-x/knowledgebase/uninstaller) is
marketing-level only: "find and safely remove applications with all their components,
including the ones that aren't in the Applications folder." No path list, no matching
algorithm published. Not useful as a design source beyond confirming the same general
shape (name/bundle-based discovery of outside-the-bundle files).

## Other open-source leftover finders (lower depth, listed for awareness)

| Project | Language | Notable detail | Source |
|---|---|---|---|
| bigbearman/macos-app-uninstaller | shell | `find -iname "*name*" -o -iname "*bundleid*"`, one level deep per known dir. Notes Group Containers sometimes use an identifier different from the app's bundle ID. | github.com/bigbearman/macos-app-uninstaller |
| wpexpertinbd/BHUninstaller | Rust + Node | Every removal goes to Trash behind a review sheet explaining why each file matched — same restorable-by-default posture rosie's spec wants. | github.com/wpexpertinbd/BHUninstaller |
| gostonx/uninstally | Swift | "Smart bundle-identifier detection," Finder extension. | github.com/gostonx/uninstally |
| SyntaxFear/scrub-app | native/shell | Ranks matches by confidence, highest = exact bundle-ID match. | github.com/SyntaxFear/scrub-app |

`mas` (Mac App Store CLI) is unrelated to uninstall/cleanup — it only manages App
Store install/update/search.

## System mechanisms an app-clean plan needs to use correctly

### launchctl — unload before delete

Modern (10.11+) subcommands, domain-target syntax `system/[label]`,
`gui/<uid>/[label]`, `user/<uid>/[label]`, `login/<asid>/[label]`:

- `launchctl bootout <domain-target> [<plist-path> ...]` — unloads and removes the job;
  the recommended replacement for legacy `unload`. Removing a user LaunchAgent before
  deleting its plist: `launchctl bootout gui/$(id -u)/<label>` or
  `launchctl bootout gui/$(id -u) <path-to-plist>`.
- `launchctl load`/`unload [-wF]` — legacy pair, still used by some tools (Pearcleaner
  uses these, not bootstrap/bootout); `-w` toggles the persisted `Disabled` key.
- System-domain jobs (LaunchDaemons) require root to bootout.

Source: ss64.com/mac/launchctl.html (third-party man-page mirror — spot-check against
`man launchctl` on the dev machine, since Apple's own text is sparse).

### Privileged helper tools

`SMJobBless()`-installed helpers live at `/Library/PrivilegedHelperTools/<id>`, paired
with a generated LaunchDaemon plist at `/Library/LaunchDaemons/<id>.plist` (the plist's
`SMAuthorizedClients` key lists which apps may talk to it). A correct app-scoped clean
checks both locations for a label matching the app's bundle-ID prefix, `bootout`s the
daemon (root required, `system` domain), then removes the executable and the plist.
Modern equivalent is `ServiceManagement.SMAppService.daemon`, register/unregister —
Pearcleaner uses this for its own helper (see above).

### Login items / Background Task Management (macOS 13+)

Registrations live in `/private/var/db/com.apple.backgroundtaskmanagement/
BackgroundItems-v*.btm` (an `NSKeyedArchiver` binary plist covering apps, login items,
and SMAppService-registered agents/daemons). `sfltool dumpbtm` is a read-only
diagnostic that prints current entries — there is no public CLI to write/delete BTM
entries directly; the sanctioned path is removing the source app and/or the app's own
`SMAppService.unregister()` call, not editing the `.btm` file. Pre-Ventura equivalent:
`~/Library/Application Support/com.apple.backgroundtaskmanagementagent/
backgrounditems.btm` and `~/Library/Application Support/com.apple.sharedfilelist/
com.apple.LSSharedFileList.SessionLoginItems.sfl2`.

### Kernel extensions / system extensions

`kextstat` lists loaded legacy kexts (filter third-party with `grep -v com.apple`);
`kmutil` replaces deprecated `kextload` on macOS 11+. `systemextensionsctl list` shows
installed system extensions and state; `systemextensionsctl reset` force-clears all of
them. Storage at `/Library/SystemExtensions` (`/Library/DriverExtensions` for driver
extensions) is **not directly deletable** — correct removal goes through the owning
app's uninstall path (which calls the SystemExtensions API) or `systemextensionsctl`,
sometimes followed by a reboot before the extension fully clears. Pearcleaner
explicitly skips `/Library/SystemExtensions` in its global denylist rather than trying
to manage it.

### Package receipts (pkgutil)

For apps installed via `.pkg` rather than drag-installed `.app` bundles, receipts are
the authoritative leftover list — more reliable than heuristic directory scanning:

| Command                                  | Purpose                                                          |
|-------------------------------------------|--------------------------------------------------------------------|
| `pkgutil --pkgs`                          | List all installed package IDs (default volume `/`)                |
| `pkgutil --pkg-info <package-id>`         | Install date, version, volume, install location                    |
| `pkgutil --files <package-id>`            | Exact file list the installer laid down (`--only-files`/`--only-dirs`) |
| `pkgutil --forget <package-id>`           | Discards the receipt only — **does not delete any files**          |

Practical flow: enumerate `--pkgs` for IDs matching the app's reverse-DNS prefix,
confirm identity with `--pkg-info`, get the real file list with `--files`, delete those
files, then `--forget` to clear the receipt.

## Design implications for rosie

- Bundle-ID substring matching is the universal first signal across every tool
  surveyed; name-based and depth-2 directory-walk matching is what catches the rest
  (vendor-named folders without the bundle ID embedded).
- Pearcleaner's tiered sensitivity (Strict/Enhanced/Deep) and per-app override table are
  the most mature false-positive defense in the family and are a reasonable model for
  rosie's own rule format — plain bundle-ID/name matching plus an escape hatch for
  known collisions (the DSL/TOML target format the spec already calls for can carry
  this).
- No tool in the family fully automates launch-agent/daemon unload *and* privileged
  helper teardown *and* pkg-receipt cleanup in one coherent, modern (`bootout`,
  `SMAppService`) pipeline — rosie doing all three correctly, with the older `bootout`
  syntax instead of Pearcleaner's legacy `load`/`unload`, would be a genuine
  improvement over the surveyed prior art.
- Orphan-mode leftover scanning (cleaning up after an app that's already gone) needs a
  cross-check against currently-installed apps' identifiers as its main safeguard, not
  just a keyword denylist — Pearcleaner's design confirms the denylist alone is not
  sufficient.
