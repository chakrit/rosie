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
- A match claimed by an equal-or-longer installed bundle ID is unticked (for example
  Chrome vs Chrome Canary).
- `group.<id>` containers are not matched.
- An app whose bundle ID cannot be read is a report entry.
- `launchctl bootout` uses the domain `gui/<uid>` for a user LaunchAgent and `system`
  for `/Library/LaunchDaemons`, with the plist path. Bootout runs before that plist is
  deleted. A bootout of a job that is not loaded fails normally.
- `pkgutil --forget` is a tool command, outside the roots gate
  ([safety.md](safety.md#cleanup-targets)).
- An app argument with a symlink component is refused unless `enter_links` is `roots` or
  `all` ([safety.md](safety.md#symlinks)).
- Before listing leftovers, app mode runs the bundle's own delete checks. Any refusal
  (outside roots, mount point, running process, …) refuses the whole operation, naming
  the reason ([safety.md](safety.md#running-processes)).
- Every filesystem item is gated by `roots` ([safety.md](safety.md#roots)).

## `rosie clean orphans`

Orphans mode finds leftovers of apps that are no longer installed. It is its own mode,
gated by `roots`, and every orphan item is aggressive.

- It searches the same locations as `clean app`.
- An app is installed when its bundle ID is found under `/Applications`,
  `~/Applications`, or `/System/Applications`. iOS and iPad apps are read from
  `Wrapper/<name>.app`. Installed checks fold case.
- An ID is an orphan only when nothing claims it. An ID is claimed by:
  - an installed app with that bundle ID;
  - an installed app sharing its vendor prefix (the first two dot-parts);
  - a currently loaded launchd job;
  - an installed Homebrew formula (`homebrew.mxcl.<formula>`).
- Loaded launchd jobs are those of the user domain (`launchctl list`) and of the system
  domain (`launchctl print system`, its `services` block).
- Claimed IDs never appear in orphans, ticked or not.
- An app whose bundle ID cannot be read fails the orphans scan.
- An app without a bundle ID is a note in orphans.
- `com.apple.*` is excluded.
