# Vendor reference

Store retained lookup facts about third-party tools and services: framework commands,
external API signatures, CLI flags, config keys, and error codes.

Route the project's own config, CLI, API, and schema to `../spec/`. Route task
walkthroughs to `../guides/` when that folder exists. Otherwise report the missing
destination and stop.

Link to the authoritative upstream source. Keep only the slice the project reuses and
project-specific gotchas. Do not mirror an entire external surface.

Start every file with its source and the version or date read:

```
<!-- derived from: <source-or-url> @ <version-or-date> -->
```

Re-read the upstream source before correcting stale material.

## Format

Use one file per subject: `<slug>.md`. Prefer tables and lists, keep entries skimmable,
and update them in place.
