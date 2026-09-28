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

- The plan is refused while the app is running
  ([safety.md](safety.md#running-processes)).
- Rule packs may add per-bundle-ID leftover paths, for example from Homebrew zap data
  ([rules.md](rules.md#packs)).
- Every item is gated by `roots` ([safety.md](safety.md#roots)).

## `rosie clean orphans`

Orphans mode finds leftovers of apps that are no longer installed. It is its own mode,
gated by `roots`, and every orphan item is aggressive.
