# Rosie — working plan

Status: draft

This is the working plan for rosie, kept close to the words chakrit used when describing
it. Quoted text is chakrit's wording. Lines marked **Interpretation** are the agent's
reading of that wording and still need confirmation. Open questions are tracked outside
this file until settled; each settled answer amends this file.

## What rosie is

- "Rosie" is inspired by the Jetsons' robot maid.
- "basically a mac cleanup cli geared for developers."
- Written in Rust, following the PRODIGY9 school skills and conventions.
- "everything should be fast fast fast as per rust standard."

## Two phases: scan and clean

- "CLI design should allow for basic interactive scan/clean in two phases."
- "it should also allow for inspectable plan mode (ala terraform)."
- Basic invocation: "rosie invocation -> scan, present plan, user confirm y/n, execute
  plan".
- The phases can run independently: "can be called with rosie scan or rosie run [plan]
  or some such to execute phases independently."

**Interpretation:** `rosie scan` writes a plan file that a person can read (and
possibly edit) before `rosie run <plan>` executes exactly that plan, like
`terraform plan -out` followed by `terraform apply <plan>`.

## Privilege elevation

- "it detects and self-elevate (if possible) using sudo if needed."
- "let user type into regular sudo prompt so we don't have to worry our selves about any
  security stuff."

**Interpretation:** rosie never handles passwords itself; when a plan contains items
that need root, rosie re-invokes itself through the system `sudo` and the user answers
sudo's own prompt.

## Cleanup targets are data, not code

- "the things to scan should be a large mini-dsl collection or a lua vm or somesuch."
- "we can manually edit/publish targets independently of what's built-in w rosie (i.e.
  future devs can add their own rules/new unsupported tools etc.)."
- "Prefer toml configuration where needed or even use toml as the cleanup target
  configuration layer."

**Interpretation:** built-in targets ship as the same rule format that users and
third parties write, so a built-in rule is only a rule that happens to be bundled.

## Built-in coverage and smart detection

- "the built-ins should cover all standard coding tool artifacts like node_modules cargo
  target/ etc."
- "it should be a bit smart in how those folders are detected as well not just by name
  (i.e. check cargo.toml before treating a folder named target for cleanup)."

## Restorable mode

- "it sihould support restorable mode somehow (like moving this to trash bin instead of
  immediately deleting etc.)"

## Targeted cleanup

- "it should also allow a targeted cleanup like a specific project dir, specific cleanup
  groups (i.e. all node_modules or dockers or cargo targets)."

**Interpretation:** a scan can be narrowed by location (a directory) and by group (a
named set of rules), and both narrowings can combine.

## macOS application cleanup

- "another special mode is for macos application cleanups (look at how appcleaner.app
  does it or similar app in the family)."
- "so i can go something like `rosie clean SomeRandom.app` and it'd know to scan for
  user prefs, launchdaemons etc."

**Interpretation:** given an app bundle, rosie reads its bundle identifier and name, then
finds the files that app leaves across `~/Library`, `/Library`, and launch agent and
daemon folders. The result is a plan like any other scan.

## Working order

- "Do not start coding just yet, only basic inits are allowed. Docs/specs/architect/plan
  first."
