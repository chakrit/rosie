//! The FS interaction layer: every filesystem, command, and metadata interaction goes
//! through it (`docs/spec/safety.md#fs-interaction-layer`).

mod backend;
mod context;
mod error;
pub mod fake;
mod links;
mod ops;
mod paths;
mod real;

pub use backend::{
    Argv, Backend, CommandOutput, Exit, FileKind, Metadata, ROOT_UID, SF_DATALESS, SystemArgv,
    SystemTool,
};
pub use context::{Acting, Bounds, FsContext, resolve_dots, running_argv};
pub use error::{Error, Op, RealPath};
pub use links::EnterLinks;
pub use ops::{Cleanup, FsOp, Rosie, UserInput};
pub use paths::Paths;
pub use real::RealBackend;
