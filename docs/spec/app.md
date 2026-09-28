# App and orphan cleanup

Status: accepted. Target behavior; not implemented.

## `rosie clean app <X.app>`

Given an app bundle, rosie finds the files the app leaves behind (user preferences,
launch daemons, and similar), in the manner of AppCleaner.app and its family. The result
is a plan like any other ([plan.md](plan.md)).

| Item                               | Treatment                                              |
|------------------------------------|--------------------------------------------------------|
| the bundle itself                  | included, ticked                                       |
| bundle-ID matches                  | ticked                                                 |
| name-only matches                  | aggressive (unticked)                                  |
| launch agents and daemons          | `launchctl bootout` before the plist is deleted        |
| package receipts                   | `pkgutil --forget`                                     |
| login items, system extensions     | report-only, with manual steps                         |

- `Info.plist` is read with `plutil`.
- A bundle-ID match is a file or folder whose name equals the bundle ID or starts with
  `<id>.` (dot-bounded), plus the team-prefixed Group Containers form
  (`<team>.<id>`).
- A name-only match uses `CFBundleName`.
- `launchctl bootout` uses the domain `gui/<uid>` for a user LaunchAgent and `system`
  for `/Library/LaunchDaemons`, with the plist path. Bootout runs before that plist is
  deleted. A bootout of a job that is not loaded fails normally.
- `pkgutil --forget` is a tool command, outside the roots gate
  ([safety.md](safety.md#cleanup-targets)).
- An app argument with a symlink component is refused
  ([safety.md](safety.md#symlinks)).
- The plan is refused while the app is running
  ([safety.md](safety.md#running-processes)).
- Rule packs may add per-bundle-ID leftover paths, for example from Homebrew zap data
  ([rules.md](rules.md#packs)).
- Every filesystem item is gated by `roots` ([safety.md](safety.md#roots)).

## `rosie clean orphans`

Orphans mode finds leftovers of apps that are no longer installed. It is its own mode,
gated by `roots`, and every orphan item is aggressive.

- It searches the same locations as `clean app`.
- An app is installed when its bundle ID is found under `/Applications`,
  `~/Applications`, or `/System/Applications`.
- `com.apple.*` is excluded.
