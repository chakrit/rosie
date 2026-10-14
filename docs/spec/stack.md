# Stack

Status: accepted.

- Rust stable, pinned in `rust-toolchain.toml`; edition 2024.
- Target: Apple Silicon (`aarch64-apple-darwin`), macOS 11 and later.
- User-facing configuration is TOML.
- No ObjC bindings and no FFI, beyond the deliberate picks `mlua` and
  `security-framework` (via native-tls). Do not add FFI crates or invent new FFI. No
  `libc` crate.
- Prefer system CLIs (`ps`, `lsof`, `id`, `launchctl`, `pkgutil`, `plutil`) and std.
- Simplify uses of Rust types as much as we can.
- No `dyn` for the FS interaction layer; static generics
  ([safety.md](safety.md#fs-interaction-layer)).
- `mlua` is used without its `send` feature; Lua borrows the gate through `lua.scope`.
- No `#![deny(warnings)]`; a clean `cargo clippy --all-targets --all-features` is the
  gate.

## Disk space

- Monitor disk space and clean as you go, regularly.
- Keep monitoring while subagents build and during anything else that eats up space.

## Crates

| Crate                         | Use                                                                  |
|-------------------------------|----------------------------------------------------------------------|
| `clap`                        | CLI parsing, `--help`, `--version`                                   |
| `console`                     | terminal styling of stderr lines ([cli.md](cli.md#output))           |
| `indicatif`                   | progress and spinners                                                |
| `inquire`                     | pickers, confirmation, checklist                                     |
| `thiserror`                   | error types                                                          |
| `serde` + `toml`              | config, rules, and plan files                                        |
| `toml_edit`                   | in-place `config.toml` edits                                         |
| `serde_json`                  | Lua `read_json`                                                      |
| `mlua`                        | Lua 5.4 (`vendored`) rule strategy ([rules.md](rules.md#lua))        |
| `ureq`                        | pack downloads over HTTPS                                            |
| `tar` + `flate2`              | pack unpacking ([safety.md](safety.md#rosies-own-data))              |
| `rayon`                       | parallel walk, sizing, and delete ([performance.md](performance.md)) |

`ureq` uses native TLS (macOS Security framework, Keychain trust, no `ring`). Rosie does
not use `rustls` or another pure-Rust TLS stack, for macOS adherence, not for any TLS
technicality:

```toml
ureq = { version = "3", default-features = false, features = ["native-tls"] }
```
