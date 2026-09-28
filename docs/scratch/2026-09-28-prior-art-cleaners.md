<!-- not spec because: still exploring; nothing settled yet -->

# Prior-art research: developer/system cleanup CLIs

Background research for rosie's design (two-phase scan/clean like `terraform
plan`/`apply`, data-driven TOML rules, restorable/trash mode, targeted cleanup by
directory or named group, and a dedicated `.app` bundle cleanup mode).

## kondo (tbillington/kondo, Rust)

- Detects build/dependency artifacts for 20+ project ecosystems (Rust/Cargo, Node,
  C/C++/CMake, PHP/Composer, Java Gradle/Maven, Python Jupyter/Pixi, Haskell
  Stack/Cabal, Unity, Swift, Dart/Flutter, Ruby, and more) by recognizing marker
  files per project type.
- Detection rules are **hardcoded in Rust** (`kondo-lib/src/project.rs`), not a
  data-driven or plugin format. No TOML/YAML rule file was found in the repo.
- `kondo-lib/Cargo.toml` depends on the `ignore` crate (0.4.25) and `walkdir` (2.x)
  for directory traversal — **no `jwalk` or `rayon`**, so traversal is not
  explicitly parallelized in the library layer (contrary to some marketing copy
  describing it as "parallelised"). `ignore` does respect `.gitignore` while
  walking, which is a natural way to skip descending into e.g. `node_modules` once
  matched.
- UX: interactive terminal UI (TUI) lists discovered projects and reclaimable size
  per project before deletion; also has a non-interactive mode.
- Safety: `--older <duration>` (e.g. `--older 3M`) filters to projects whose last
  modification is older than a threshold — the closest thing to an age-based
  safety gate. No explicit git-dirty check documented. No trash integration
  documented — README explicitly warns "Kondo is essentially `rm -rf` with a
  prompt," i.e. permanent delete, back up first.
- No plan/apply split beyond "list, then confirm" — it's closer to `terraform
  apply` with an inline `-auto-approve`-style prompt than a saved plan artifact.

## npkill (Ludvins/npkill, TypeScript/Node)

- Single-purpose: scans the filesystem for `node_modules` directories and reports
  their size.
- Detection is a **hardcoded, single-target** directory-name match — no rule file,
  no plugin system, no other ecosystem support.
- UX: interactive list (curses-like TUI) of found directories sorted/browsable,
  `Space`/`Del` to select and delete.
- Safety: flags directories that look like they belong to installed apps (e.g.
  Spotify, Discord) with a warning icon, but does not block deletion — no age
  threshold, no git check, no trash (deletes directly, "low level" delete for
  speed per its own docs).
- No dry-run/plan file; the interactive list itself functions as an implicit
  preview before you press delete.

## cargo-sweep (holmgr/cargo-sweep, Rust) — **unmaintained**

- Purpose-built for Cargo's `target/` directories; not a general project cleaner.
- Detection/safety model is **time-marker based** rather than static rules:
  `cargo sweep --stamp` writes a timestamp file, and a later `cargo sweep --file`
  removes only artifacts older than that marker (i.e., untouched since the last
  stamp). Also supports `--time <days>` (age threshold) and toolchain-matching
  (removing artifacts built by toolchains no longer installed).
- Rules are **hardcoded** (single-purpose logic in the binary), not
  configurable/data-driven.
- Dry-run: `--dry-run` flag combinable with `--time`/`--file`, prints what would be
  removed without deleting.
- No git-dirty check, no trash integration (permanent delete via `target/`
  cleanup), no plan-file artifact — dry-run output is print-only, not persisted.

## cargo-cache (matthiaskrgr/cargo-cache, Rust)

- Targets `$CARGO_HOME` (`~/.cargo`): installed binaries, registry crate archives,
  registry source checkouts, and git-repo checkouts/bare repos used by
  git-dependency builds.
- Rules are **hardcoded flags per cache category**, not data-driven; categories are
  fixed to what Cargo itself stores (binaries, registry, git db).
- Safety/selectivity: `--autoclean` removes only reconstructable data (extracted
  source checkouts) while keeping compressed archives needed to regenerate them;
  `--autoclean-expensive` additionally recompresses git repos; `--remove-if-
  older-than` / `--remove-if-younger-than` give age-threshold filtering;
  `--keep-duplicate-crates N` limits duplicate-version retention instead of
  deleting all older versions outright.
- Dry-run: `--dry-run` prints a before/after size diff (e.g. "Size changed 3.38 GB
  => 3.28 GB (-100.29 MB, -2.96%)") without touching disk — a concrete example of
  a size-delta-oriented plan output, worth mirroring in rosie's plan phase.
- No trash (permanent delete) — recoverability instead comes from Cargo
  re-downloading registry data on demand, which is domain-specific to
  package-manager caches rather than a general restorable mode.

## mac-cleanup-py (mac-cleanup/mac-cleanup-py, Python)

- General macOS system + developer-tool cleaner covering 40+ modules: Homebrew,
  npm/yarn/pnpm/pip/poetry/composer/gradle/conan caches, Xcode DerivedData/
  archives, Android, JetBrains IDEs, Docker, browser caches (Chrome/Chromium/Arc),
  game clients (Steam, Minecraft), cloud-sync apps (Dropbox, Google Drive, Teams),
  system logs/DNS cache/inactive memory/Trash emptying.
- Rules are **hardcoded as individual Python modules** (one file per
  app/ecosystem, e.g. `adobe.py`, `chrome.py`, `docker.py`) rather than a single
  declarative rule format, but the module boundary is a plausible template for
  rosie's per-target TOML rule if ported to data form. It does support
  user-supplied **custom modules** via `-p/--custom-path`, which is a lightweight
  plugin/extension mechanism (Python code, not declarative data).
- UX: `-n/--dry-run` runs without deleting; `-v/--verbose` prints the folders that
  would be deleted; `-f/--force` skips per-category confirmation warnings —
  implying default behavior prompts per category.
- Safety: dry-run + per-module warning prompts are the main gates; no age
  threshold or git-dirty check found; permanently deletes (calls into system
  Trash emptying as one of its *targets*, not as its deletion mechanism for other
  files) — i.e. no restorable/trash-based delete for its own actions.

## Mole (tw93/Mole) — corrected from initial search noise

- **Correction:** initial web search results and marketing text described Mole as
  "written in Rust," but `gh api repos/tw93/mole/languages` (read 2026-09-28) shows
  the actual repo is **Shell (4.0 MB) + Go (728 KB) + Python (23 KB) + Makefile**,
  with no Rust/Cargo.toml at the repo root (fetching one 404s). The Rust/Tauri
  description belongs to a *separate* third-party GUI wrapper repo
  (`Zhili1004/MoleUI`), not to `tw93/Mole` itself. Treat any Rust-related claim
  about `tw93/Mole`'s implementation with caution; verify against source before
  relying on it for design decisions.
- Functional scope (per README/marketing, not yet cross-checked against source):
  `mo clean` for deep system/app caches, `mo uninstall` for app + leftover-file
  removal (launch agents, prefs, hidden remnants), `mo purge` for dev artifacts
  (`node_modules`, `target`, `.build`, `build`, `dist`), plus installer-file
  sweep (DMG/PKG/MPKG/ISO/XIP) across Downloads/Desktop/Homebrew caches/iCloud.
- Claimed safety features: dry-run preview (`--dry-run`), moves to Trash by
  default for `mo analyze`-driven actions (reversible), protects git-tracked
  files/nested repos and deploy keypair files from deletion, skips paths with
  recent file activity by default (an implicit age threshold), logs all cleanup
  operations to `~/Library/Logs/mole/operations.log`, and supports a whitelist
  (`mo clean --whitelist`) to permanently exclude paths.
- Rule implementation: no TOML/YAML rule file surfaced in search; given the
  Shell+Go composition, cleanup categories are most likely hardcoded per-category
  logic rather than a declarative/plugin rule format — this should be verified
  directly against source before citing as fact, since the language mismatch above
  shows secondary sources on this project are unreliable.

## "dev-cleaner" style tools

No single canonical project matched this name; closest matches are the
general-purpose cleaners above (mac-cleanup-py, Mole) plus narrower dev-only tools
(kondo, npkill, cargo-sweep, cargo-cache). No additional distinct tool was found
worth a separate entry within this research pass.

## CleanMyMac's Xcode/dev-junk module (closed source, public docs only)

- Ships a dedicated "Xcode Junk" cleaner targeting: DerivedData, unused
  simulators, old build archives, and iOS device-support files.
- Per MacPaw's own marketing/help copy, it uses "a built-in list of known-safe
  items" — i.e., an internal (undisclosed, presumably hardcoded, not
  user-extensible) allowlist of paths known safe to remove — and shows every file
  or category before the user confirms; nothing is removed without confirmation.
- No public detail on age thresholds, git-awareness, or whether deletion goes
  through Trash vs. permanent delete; as closed-source software this can't be
  verified beyond marketing copy. Treat this section as lower-confidence than the
  open-source entries above.

## Comparison table

| Tool          | Language   | Rule format              | Plan/dry-run UX            | Trash or delete       | Age/git safety gates                  | Traversal perf notes                         |
|---------------|------------|---------------------------|-----------------------------|------------------------|-----------------------------------------|-----------------------------------------------|
| kondo         | Rust       | hardcoded per ecosystem   | interactive TUI list        | permanent delete       | `--older <duration>`, no git check      | `ignore` + `walkdir`, no jwalk/rayon found    |
| npkill        | TypeScript | hardcoded, single target  | interactive list             | permanent delete       | none (warns on app-owned dirs only)     | low-level scan per its own docs               |
| cargo-sweep   | Rust       | hardcoded, single target  | `--dry-run` print           | permanent delete       | timestamp marker, `--time` age filter   | n/a (Cargo `target/` only)                    |
| cargo-cache   | Rust       | hardcoded per cache class | `--dry-run` size-delta print| permanent delete       | age filters, autoclean reconstructable  | n/a (`$CARGO_HOME` only)                      |
| mac-cleanup-py| Python     | hardcoded modules + custom module plugin (code, not data) | `-n/--dry-run`, `-v` verbose | permanent delete (Trash-emptying is one *target*, not the delete mechanism) | none found | n/a |
| Mole          | Shell + Go | likely hardcoded (unverified) | `--dry-run` (claimed)   | Trash by default (claimed), whitelist exclusions | skips recently-active paths, protects git/keys (claimed) | unverified |
| CleanMyMac (Xcode module) | closed source | undisclosed internal allowlist | per-item confirmation UI | unknown | unknown | unknown |

## Implications worth flagging for rosie's design

- **None of the surveyed tools use a declarative, third-party-extensible rule
  format (TOML or otherwise).** mac-cleanup-py's "custom module" mechanism is the
  closest analog, but it's Python code, not data. Rosie's TOML-rule design is
  differentiated territory, not an established pattern to copy — this is a
  design risk to validate (rule schema expressiveness) as much as a strength.
- **cargo-sweep's timestamp-marker approach** (`--stamp` then sweep-by-age) is a
  useful alternative/complement to a flat `--older` threshold for CI-style
  "artifacts since last known-good point" cleanup.
- **cargo-cache's size-delta dry-run output** ("before => after, -X, -Y%") is a
  good concrete model for rosie's `plan` phase summary.
- **kondo's `ignore`-crate-based traversal** (respects `.gitignore`, skips
  descending into ignored trees) is a directly reusable technique for rosie's
  scanner, and is a lighter dependency than `jwalk`; none of the surveyed tools
  actually use `jwalk`, so that expectation is not corroborated by this batch of
  prior art.
- **Git-dirty checks are essentially absent** across every tool surveyed — no
  tool refuses to clean because of uncommitted changes. If rosie wants this
  safety feature, it appears to be a genuine gap in prior art rather than a
  convention to follow.
- **Trash-based restorable delete is rare** — only Mole claims it by default (and
  that claim is unverified against source), while every Rust dev-tool in this
  survey (kondo, cargo-sweep, cargo-cache) does permanent delete only. Rosie's
  restorable-mode goal is a real differentiator, not table stakes.

## Sources

- [tbillington/kondo README](https://github.com/tbillington/kondo/blob/master/README.md) — read 2026-09-28
- [tbillington/kondo kondo-lib/Cargo.toml](https://github.com/tbillington/kondo/blob/master/kondo-lib/Cargo.toml) — read 2026-09-28
- [Ludvins/npkill](https://github.com/Ludvins/npkill) — read 2026-09-28
- [holmgr/cargo-sweep README](https://github.com/holmgr/cargo-sweep/blob/master/README.md) — read 2026-09-28
- [matthiaskrgr/cargo-cache](https://github.com/matthiaskrgr/cargo-cache) — read 2026-09-28
- [mac-cleanup/mac-cleanup-py README](https://github.com/mac-cleanup/mac-cleanup-py/blob/main/README.md) — read 2026-09-28
- [tw93/Mole](https://github.com/tw93/mole) — read 2026-09-28
- [GitHub API: repos/tw93/mole/languages](https://api.github.com/repos/tw93/mole/languages) — read 2026-09-28 (used to correct language claim)
- [Zhili1004/MoleUI](https://github.com/Zhili1004/MoleUI) — read 2026-09-28 (source of the Rust/Tauri confusion; this is a separate GUI wrapper repo, not tw93/Mole itself)
- [MacPaw: How to clear Xcode cache](https://macpaw.com/how-to/clear-xcode-cache) — read 2026-09-28
