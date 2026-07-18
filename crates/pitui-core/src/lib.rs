//! Pure, runtime-independent data used by Pitui.
//!
//! This crate deliberately has no dependency on `bevy_ecs`, terminal code or
//! the Git executable. It is the value/payload boundary shared by the Legacy
//! application and the next-generation Dataset ECS runtime.

#![forbid(unsafe_code)]

mod diff;
mod model;

pub use diff::*;
pub use model::*;
