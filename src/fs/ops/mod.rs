//! Every filesystem access is a typed op that performs itself against an
//! [`FsContext`], checked by the policy of the owner it belongs to
//! (`docs/spec/safety.md#fs-interaction-layer`):
//!
//! - [`Cleanup`]: the trees rosie scans and the cleanup targets it deletes.
//! - [`Rosie`]: rosie's own config and data, confined to its own folders.
//! - [`UserInput`]: paths and files the user names.
//!
//! The owners' constructors are the only way to build an op, so every filesystem access
//! is a call to one of them and no combination their policies do not allow can be
//! expressed.

mod cleanup;
mod rosie;
mod user_input;

use super::backend::Backend;
use super::context::FsContext;
use super::error::Error;

pub use cleanup::Cleanup;
pub use rosie::Rosie;
pub use user_input::UserInput;

/// An op that performs itself against a context. Sealed: only the owners above build
/// one.
pub trait FsOp<B: Backend>: sealed::Sealed {
    type Output;

    fn perform(self, ctx: &FsContext<B>) -> Result<Self::Output, Error>;
}

mod sealed {
    pub trait Sealed {}
}
