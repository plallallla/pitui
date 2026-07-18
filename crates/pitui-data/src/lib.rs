//! Canonical ECS data for the next-generation Pitui runtime.
//!
//! This crate contains facts and declarative descriptions only. It has no Git
//! process runner, terminal renderer, input loop or application controller.

#![forbid(unsafe_code)]

mod context;
mod dataset;
mod identity;
mod metadata;
mod operation;
mod render;
mod template;

pub use context::*;
pub use dataset::*;
pub use identity::*;
pub use metadata::*;
pub use operation::*;
pub use render::*;
pub use template::*;
