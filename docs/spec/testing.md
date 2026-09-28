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
| Fixture           | `cargo test`   | rule matching against recorded trees in the fake         |
| Contract          | `cargo test`   | the fake and `RealBackend` behave the same               |
| Sandbox           | `cargo test`   | CLI end to end, in-process, with the fake injected       |
| Smoke             | `cargo test`   | the real binary on temp folders                          |
| Benchmark         | gz44, manual   | speed budgets on a real temp-folder tree                 |
| Record-and-replay | Docker, manual | fixture manifests match what the real tools produce      |

## Record-and-replay fixtures

- A capture script runs the real tools (for example `npm install`) inside Docker and
  records the resulting tree shapes (paths and sizes) as committed manifests.
- `cargo test` loads each manifest into the in-memory fake and checks which paths
  rosie's rules match and which they leave alone.
- Re-run the capture when a tool's layout changes.

## Fakes and sandboxing

- The FS interaction layer is used during tests: an in-memory fake `Backend` is injected
  into everything ([safety.md](safety.md#fs-interaction-layer)).
- The fake simulates root-owned items, other volumes, dataless placeholders, large
  sizes, running processes, and errors partway through a delete. Its mutations sit
  behind a `Mutex`.
- Tests assert on the fake's recorded calls, including which tool `cmd` a rule ran.
- One contract suite runs against both the fake and `RealBackend` on a temp folder.
- The sandbox tier runs the CLI in-process through `App<B>` with the fake. A few smoke
  tests run the real binary on temp folders with `env_clear()` and a sandboxed `HOME`
  and `PATH`.
- Roots are injected, so tests point every root at the fake tree or a temp folder.

## Benchmark

- Walk 1M entries in under 1 second, warm cache, on gz44.
- The sizing budget is set from the first measurement.
