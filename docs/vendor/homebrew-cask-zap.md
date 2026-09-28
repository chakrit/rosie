<!-- derived from: docs.brew.sh/Cask-Cookbook#stanza-zap, github.com/Homebrew/brew (Library/Homebrew/cask/artifact/{zap,uninstall,abstract_uninstall}.rb, main branch), github.com/Homebrew/homebrew-cask (Casks/{s/slack,v/visual-studio-code,d/docker-desktop}.rb, master branch) — read 2026-09-28 -->

# Homebrew Cask `zap`/`uninstall` stanzas as an uninstall-leftover data source

Homebrew Cask maintains a crowd-sourced, per-app database of exactly which files an app
leaves behind on macOS, expressed as `zap`/`uninstall` stanzas in each cask's Ruby
formula. Since these are reviewed by Homebrew maintainers and cover thousands of real
apps, they're a directly reusable data source for rosie's built-in targets — either by
periodically importing them, or by pointing users at
`github.com/Homebrew/homebrew-cask/blob/master/Casks/<letter>/<name>.rb` as a reference
when writing a rosie target for an app.

## `uninstall` vs `zap`

Both stanzas share identical syntax (`Cask::Zap` and `Cask::Uninstall` both subclass
`AbstractUninstall` and dispatch the same directive list) — they differ only in when
Homebrew runs them:

| Stanza      | Runs on                              | Intent                                                                 |
|-------------|----------------------------------------|--------------------------------------------------------------------------|
| `uninstall` | every `brew uninstall <cask>`         | Remove what's needed for the app to be gone and cleanly reinstallable (binaries, helper daemons, pkg receipts) |
| `zap`       | only `brew uninstall --zap <cask>`    | Additionally remove user data a normal uninstall deliberately leaves for a clean reinstall: preferences, caches, saved state, containers, cookies, crash logs |

Docs quote (docs.brew.sh/Cask-Cookbook#stanza-zap): "zap procedures will never be
performed by default" — opt-in only. Cask authors are told zap "should not remove files
created by the user directly" (e.g. documents), only app-owned state — the same
distinction rosie's own restorable-mode boundary needs to draw.

## Directive list

From `Library/Homebrew/cask/artifact/abstract_uninstall.rb`, `ORDERED_DIRECTIVES`
(lines 23-35). Directives always execute in this fixed order regardless of the order
written in the stanza, so that e.g. the app is told to quit before its kext is unloaded,
and a helper script isn't deleted before it runs:

| Order | Directive       | Behavior                                                                                          |
|------:|------------------|------------------------------------------------------------------------------------------------------|
| 1     | `early_script`   | Runs a script before everything else; discouraged, "should not delete files"                        |
| 2     | `launchctl`      | Removes a launchd job by label (supports `*` wildcard); deletes matching `/Library/Launch{Agents,Daemons}/<id>.plist` |
| 3     | `quit`           | Sends an AppleEvent quit to the bundle ID via `osascript`, 10s timeout, `*` wildcard supported       |
| 4     | `signal`         | Sends a Unix signal (e.g. `TERM`, `KILL`) to PIDs found via `launchctl list` matching a bundle ID; takes `[signal, bundle_id]` pairs |
| 5     | `login_item`     | Removes a login item by `name:` or `path:` via System Events AppleScript                             |
| 6     | `kext`           | `kextunload` + remove the kext bundle by ID                                                          |
| 7     | `script`         | Runs an arbitrary executable staged in the cask, with args; `sudo:` optional                         |
| 8     | `pkgutil`        | Matches pkg receipt IDs (string or regex) and calls `pkgutil --forget` on each match                 |
| 9     | `delete`         | `sudo rm -rf` on absolute paths — irreversible, no Trash; treated as last resort                     |
| 10    | `trash`          | Moves paths to the macOS Trash — reversible, the preferred directive for `zap`                       |
| 11    | `rmdir`          | Removes directories only if empty (ignoring stray `.DS_Store`)                                       |

`on_upgrade:` is a stanza-key modifier (not a directive) controlling whether `signal`
runs during `upgrade`/`reinstall` — normally skipped then.

## Path syntax

- Paths use `~` for home (expanded via `Dir.home`) and `*` for shell-style globs
  (resolved via Ruby's `Pathname.glob`, **not** full regex).
- Relative paths and paths containing `.`/`..` segments are rejected.
- No `#{appdir}`-style interpolation appears in `zap`/`uninstall` paths (that token is
  used elsewhere, in `app`/`binary` stanzas) — zap/uninstall paths are written as
  literal strings.
- `pkgutil:` values are regex against installed package receipt IDs, resolved via
  `Cask::Pkg.all_matching`.

**For rosie's own target parser**: implement `~`-expansion plus glob support, not a
regex engine, when treating `trash:`/`delete:` values as a leftover-path source.

## Real examples (quoted verbatim, `Homebrew/homebrew-cask` @ `master`)

### Slack — `Casks/s/slack.rb`

```ruby
uninstall quit: "com.tinyspeck.slackmacgap"

zap trash: [
  "/Library/Logs/DiagnosticReports/Slack_*",
  "~/Library/Application Scripts/com.tinyspeck.slackmacgap",
  "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/com.tinyspeck.slackmacgap.sfl*",
  "~/Library/Application Support/Slack",
  "~/Library/Caches/com.tinyspeck.slackmacgap*",
  "~/Library/Containers/com.tinyspeck.slackmacgap*",
  "~/Library/Cookies/com.tinyspeck.slackmacgap.binarycookies",
  "~/Library/Group Containers/*.com.tinyspeck.slackmacgap",
  "~/Library/Group Containers/*.slack",
  "~/Library/HTTPStorages/com.tinyspeck.slackmacgap*",
  "~/Library/Logs/Slack",
  "~/Library/Preferences/ByHost/com.tinyspeck.slackmacgap.ShipIt.*.plist",
  "~/Library/Preferences/com.tinyspeck.slackmacgap*",
  "~/Library/Saved Application State/com.tinyspeck.slackmacgap.savedState",
  "~/Library/WebKit/com.tinyspeck.slackmacgap",
]
```

### Visual Studio Code — `Casks/v/visual-studio-code.rb`

```ruby
uninstall launchctl: "com.microsoft.VSCode.ShipIt",
          quit:      "com.microsoft.VSCode"

zap trash: [
  "~/.vscode",
  "~/Library/Application Support/Code",
  "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/com.microsoft.vscode.sfl*",
  "~/Library/Caches/com.microsoft.VSCode",
  "~/Library/Caches/com.microsoft.VSCode.ShipIt",
  "~/Library/HTTPStorages/com.microsoft.VSCode",
  "~/Library/Preferences/ByHost/com.microsoft.VSCode.ShipIt.*.plist",
  "~/Library/Preferences/com.microsoft.VSCode.helper.plist",
  "~/Library/Preferences/com.microsoft.VSCode.plist",
  "~/Library/Saved Application State/com.microsoft.VSCode.savedState",
]
```

### Docker Desktop — `Casks/d/docker-desktop.rb`

```ruby
uninstall launchctl: [
            "com.docker.helper",
            "com.docker.socket",
            "com.docker.vmnetd",
          ],
          quit:      [
            "com.docker.docker",
            "com.electron.dockerdesktop",
          ],
          delete:    [
            "/Library/PrivilegedHelperTools/com.docker.socket",
            "/Library/PrivilegedHelperTools/com.docker.vmnetd",
          ],
          rmdir:     "~/.docker/bin"

zap trash: [
      "/usr/local/bin/docker-compose.backup",
      "/usr/local/bin/docker.backup",
      "~/.docker",
      "~/Library/Application Scripts/com.docker.helper",
      "~/Library/Application Scripts/group.com.docker",
      "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/com.docker.helper.sfl*",
      "~/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/com.electron.dockerdesktop.sfl*",
      "~/Library/Application Support/com.bugsnag.Bugsnag/com.docker.docker",
      "~/Library/Application Support/Docker Desktop",
      "~/Library/Application Support/docker-secrets-engine",
      "~/Library/Caches/com.docker.docker",
      "~/Library/Caches/com.plausiblelabs.crashreporter.data/com.docker.docker",
      "~/Library/Caches/Docker Desktop",
      "~/Library/Caches/docker-secrets-engine",
      "~/Library/Caches/KSCrashReports/Docker",
      "~/Library/Containers/com.docker.docker",
      "~/Library/Containers/com.docker.helper",
      "~/Library/Group Containers/group.com.docker",
      "~/Library/HTTPStorages/com.docker.docker",
      "~/Library/HTTPStorages/com.docker.docker.binarycookies",
      "~/Library/Logs/Docker Desktop",
      "~/Library/Preferences/com.docker.docker.plist",
      "~/Library/Preferences/com.electron.docker-frontend.plist",
      "~/Library/Preferences/com.electron.dockerdesktop.plist",
      "~/Library/Saved Application State/com.electron.docker-frontend.savedState",
      "~/Library/Saved Application State/com.electron.dockerdesktop.savedState",
    ],
    rmdir: [
      "~/Library/Caches/com.plausiblelabs.crashreporter.data",
      "~/Library/Caches/KSCrashReports",
    ]
```

Note Docker's zap stanza combines two directives (`trash:` and `rmdir:`) in one call —
a zap stanza can carry multiple directive keys, same as `uninstall`.

## Design implications for rosie

- `delete:` entries in `uninstall` stanzas (irreversible `rm -rf`) are the closest
  upstream precedent for what rosie should treat as privileged/system-owned paths
  needing elevation — e.g. Docker's `delete:` targets are both under
  `/Library/PrivilegedHelperTools`.
- `launchctl:` entries name the exact launchd label to unload before deleting its
  plist — directly reusable as the "what to bootout" list for an app's launch
  agents/daemons, without needing to guess the label from the bundle ID.
- The per-cask `zap` block is effectively a hand-verified, human-reviewed leftover list
  for that one app — a much higher-confidence source than heuristic directory/bundle-ID
  matching for any app that has a cask. Rosie's target format could treat "does this app
  have a homebrew-cask entry with a `zap` stanza" as a first-class, preferred data
  source, falling back to heuristic matching (per
  `docs/vendor/macos-app-uninstallers.md`) only when no cask exists.
