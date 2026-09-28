# Decisions log — exception only

This folder is not a routing destination. Every settled rule remains an amendment to
`../spec/`. A decision record may preserve one substantial losing argument in addition
to that amendment.

## When an entry is warranted

Require both conditions:

1. An argument happened. Two positions were considered, one lost, and the conversation or
   review thread contains the evidence.
2. The losing case cannot be preserved adequately in one sentence and would otherwise be
   argued again from the beginning.

When either condition fails, amend `../spec/` only. A surprising rule, stated preference,
chosen library, format, name, or default does not qualify.

**Read the relevant spec before working, and comply.** Raise a wrong spec with the user
and amend it. Do not use a decision record to route around the spec.

Route unsettled research, surveys, drafts, and transcripts to `../scratch/` when that
folder exists. Otherwise report the missing destination and stop; do not keep it here.

## Format

Use one file per decision: `YYYY-MM-DD-slug.md`.

```markdown
# Short title

- **Date:** YYYY-MM-DD
- **PR:** #N or `manual`
- **Status:** accepted | superseded | revised

## Decision

State the current decision in one sentence.

## Rationale

Record the substantial losing case and why it lost.
```

Amend `../spec/` in the same change. Readers learn the current rule from the spec.

## Statuses

- **accepted** — the decision is active.
- **superseded** — a newer linked decision replaces it.
- **revised** — the entry incorporates new context.
