# Stack

Status: accepted.

- Rust stable, pinned in `rust-toolchain.toml`; edition 2024.
- User-facing configuration is TOML.
- No ObjC bindings and no FFI, beyond the deliberate picks `mlua` and
  `security-framework` (via native-tls). Do not add FFI crates or invent new FFI.
- Prefer system CLIs (`ps`, `/usr/bin/trash`, `launchctl`, `pkgutil`) and std.
- Simplify uses of Rust types as much as we can.
- No `dyn` for the FS interaction layer; static generics
  ([safety.md](safety.md#fs-interaction-layer)).
- `mlua` is used without its `send` feature; Lua borrows the gate through `lua.scope`.

## Crates

| Crate           | Use                                                                  |
|-----------------|----------------------------------------------------------------------|
| `mlua`          | Lua rule strategy ([rules.md](rules.md#lua))                         |
| `ureq`          | pack downloads over HTTPS                                            |
| `rayon`         | parallel walk, sizing, and delete ([performance.md](performance.md)) |
| `unicode-ident` | pack and rule name checks ([rules.md](rules.md#names))               |

`ureq` uses native TLS (macOS Security framework, Keychain trust, no `ring`):

```toml
ureq = { version = "3", default-features = false, features = ["native-tls-no-default"] }
```
