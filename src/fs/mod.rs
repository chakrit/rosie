//! The FS interaction layer: every filesystem, command, and metadata interaction goes
//! through it (`docs/spec/safety.md#fs-interaction-layer`).

mod backend;
mod error;
pub mod fake;
mod gate;
mod real;

pub use backend::{Argv, Backend, CommandOutput, Exit, FileKind, Metadata, ROOT_UID, SF_DATALESS};
pub use error::{Error, Op, RealPath};
pub use gate::{Bounds, Exposure, Gate, resolve_dots};
pub use real::RealBackend;
