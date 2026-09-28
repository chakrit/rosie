# Stack

Status: accepted.

- Rust stable, pinned in `rust-toolchain.toml`; edition 2024.
- Target: Apple Silicon (`aarch64-apple-darwin`), macOS 11 and later.
- User-facing configuration is TOML.
- No ObjC bindings and no FFI, beyond the deliberate picks `mlua` and
  `security-framework` (via native-tls). Do not add FFI crates or invent new FFI. No
  `libc` crate.
- Prefer system CLIs (`ps`, `id`, `launchctl`, `pkgutil`, `plutil`) and std.
- Simplify uses of Rust types as much as we can.
- No `dyn` for the FS interaction layer; static generics
  ([safety.md](safety.md#fs-interaction-layer)).
- `mlua` is used without its `send` feature; Lua borrows the gate through `lua.scope`.
- No `#![deny(warnings)]`; a clean `cargo clippy --all-targets --all-features` is the
  gate.

## Crates

| Crate                         | Use                                                                  |
|-------------------------------|----------------------------------------------------------------------|
| `clap`                        | CLI parsing, `--help`, `--version`                                   |
| `console`                     | terminal styling                                                     |
| `indicatif`                   | progress and spinners                                                |
| `inquire`                     | pickers, confirmation, checklist                                     |
| `thiserror`                   | error types                                                          |
| `serde` + `toml`              | config, rules, and plan files                                        |
| `mlua`                        | Lua 5.4 (`vendored`) rule strategy ([rules.md](rules.md#lua))        |
| `ureq`                        | pack downloads over HTTPS                                            |
| `tar` + `flate2`              | pack unpacking ([safety.md](safety.md#rosies-own-data))              |
| `rayon`                       | parallel walk, sizing, and delete ([performance.md](performance.md)) |
| `unicode-ident`               | pack and rule name checks ([rules.md](rules.md#names))               |

`ureq` uses native TLS (macOS Security framework, Keychain trust, no `ring`):

```toml
ureq = { version = "3", default-features = false, features = ["native-tls-no-default"] }
```
