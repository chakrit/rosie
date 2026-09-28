# docs

File durable artifacts by this gate. Walk it from top to bottom and stop at the first
matching predicate. Nothing defaults to `scratch/`.

This scaffold may omit categories the repository does not use. Third-party lookup belongs
in `vendor/`, task-oriented how-tos in `guides/`, system design and exact surfaces in
`spec/`, and residual unsettled exploration in `scratch/`. If the matching folder has no
gate row, report the missing destination and stop. Do not use another predicate or create
the folder.

## Where does this go?

1. Third-party facts kept for lookup about a framework, external API, or CLI? →
   [`vendor/`](vendor/).
2. The system's design, intended behavior, or exact config, CLI, API, or schema? →
   [`spec/`](spec/).
3. Genuinely unsettled exploration that matches none of the above? →
   [`scratch/`](scratch/).

## Everything settled amends `spec/`

Put every settled instruction, agreement, library choice, convention, and preference in
[`spec/`](spec/). Write the rule at the length it was given. Do not add a reason or an
alternative the user did not state. A one-sentence rule such as "use RESTful routes" is
complete. Do not create a separate ruling artifact.

**Read the relevant spec before working, and comply.** Raise a wrong spec with the user
and amend it. Do not reopen, annotate, or route around a spec because it differs from a
common default or personal preference.

Record agreed but unimplemented behavior in `spec/` and mark the affected section as
intended or target. Remove abandoned behavior from the spec.

Folders named by the gate define their lifecycle in their own `README.md`.

**Read a folder's `README.md` before writing there.** It defines the folder's filing
test, filename format, and lifecycle. The repository's always-loaded instructions point
here as the docs index.
