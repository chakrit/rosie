# Spec & architecture

Store the project's current design, intended behavior, architecture, interface contracts,
and exact config, CLI, API, and schema. Update each file in place as the system changes.

Put every settled instruction, agreement, library choice, convention, and preference
here. Write the rule at the length it was given. Do not add a reason or an alternative
the user did not state. Do not create a separate ruling artifact.

**Read the relevant spec before working, and comply.** Raise a wrong spec with the user
and amend it. Do not reopen, annotate, or route around a spec because it differs from a
common default or personal preference.

Route third-party lookup to `../vendor/` when that folder exists. Route unsettled
research, surveys, and drafts to `../scratch/` when that folder exists. If the matching
folder is absent, report the missing destination and stop; do not file the artifact here.

## Index

Keep one row per spec file. Update this table in the same change when adding, renaming, or
retiring a file. Read it before adding a spec; amend the existing file when its subject
already has a row.

| File        | Covers                             | Status      |
|-------------|------------------------------------|-------------|
| `<slug>.md` | one line on what this file governs | implemented |

## Format

Use one file per subject: `<slug>.md`. Start it with a status: `draft`, `accepted`,
`implemented`, or `superseded`.
