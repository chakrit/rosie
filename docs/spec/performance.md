# Performance

Status: accepted. Target behavior; not implemented.

Everything should be fast, as per Rust standard. Speed budgets live in
[testing.md](testing.md#benchmark).

## Scan

- The walk is parallel and work-stealing, on `rayon` scopes, through `Gate<B>`
  ([safety.md](safety.md#fs-interaction-layer)). Walk crates that call std directly,
  such as `jwalk`, are not used.
- The walk stops descending once a folder matches. Walk skips prune the walk
  ([safety.md](safety.md#walk-skips)).
- Rules are compiled once into a folder-name → rules index. Lua runs only for
  candidates whose name matches, with one Lua state per worker thread.
- Sizing starts as soon as a target is found, while the walk continues. Sizes are
  allocated bytes, deduplicated by dev and inode.
- Progress shows live "found N · X GB".
- Filesystem access uses std `readdir` and `lstat` only. `getattrlistbulk` is not used.

## Run

- Deletes run in parallel across items and within large trees.
- Trash is one rename per item.
- Tool commands run one at a time, alongside filesystem deletes, each with its own
  3-line output window ([plan.md](plan.md#run)).
