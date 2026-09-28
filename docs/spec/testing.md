# Testing

Status: accepted. Target behavior; not implemented.

Tests prove that rosie cleans the right files and not the wrong ones, and that it runs
the right strategy or script for each rule.

- Temp folders are fine.
- Tests must not depend on locally installed tools. A tool absent from the machine (for
  example Gradle) is still tested.

## Tiers

| Tier              | Runs in        | Proves                                                   |
|-------------------|----------------|----------------------------------------------------------|
| Fixture           | `cargo test`   | rule matching against materialized, recorded trees       |
| Sandbox           | `cargo test`   | CLI end to end with a sandboxed environment and fakes    |
| Record-and-replay | Docker, manual | fixture manifests match what the real tools produce      |

## Record-and-replay fixtures

- A capture script runs the real tools (for example `npm install`) inside Docker and
  records the resulting tree shapes (paths and sizes) as committed manifests.
- `cargo test` materializes each manifest in a temp folder and checks which paths
  rosie's rules match and which they leave alone.
- Re-run the capture when a tool's layout changes.

## Fakes and sandboxing

- Commands, sudo, and Trash sit behind the FS interaction layer's interface
  ([safety.md](safety.md#fs-interaction-layer)); tests use recording fakes and assert on
  the recorded calls, including which tool `cmd` a rule ran.
- Roots are injected, so tests point every root at a temp folder.
- CLI tests use `env_clear()` with a sandboxed `HOME` and `PATH`.
