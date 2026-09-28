# Scratch

Store only unsettled exploration that fails every earlier predicate in the docs gate:
research dumps, surveys, investigations, drafts, and transcripts. Route settled design,
instructions, conventions, and the project's exact surface to `../spec/`. Route retained
third-party lookup to `../vendor/` when that folder exists and task walkthroughs to
`../guides/` when that folder exists. If the matching folder is absent, report the
missing destination and stop; do not keep the artifact here.

Start every scratch file with a truthful reason it is not a spec:

```
<!-- not spec because: still exploring; nothing settled yet -->
```

If that line is not true, route the artifact through the gate again.

## Format

Use one file per artifact: `YYYY-MM-DD-slug.md`. Use the shape the exploration needs.
Only a consolidated `prior-art.md` may omit the date.

## Lifecycle

Scratch files are disposable. When exploration settles, move the durable claim into
`../spec/`. Preserve raw material only when it remains useful.

Apply both deletion rules:

1. Keep material cited as provenance until every citation is repointed.
2. Consolidate many notes on one theme into `prior-art.md`. Give each source its own
   section, link each section to the live spec it informed, repoint citations, then remove
   the absorbed notes.
