//! Compatibility facade for the 0.1.0 Legacy runtime.
//!
//! The canonical Git value types and diff algorithms now live in
//! `pitui-core`, where the next-generation ECS runtime can reuse them without
//! depending on the Legacy application layer.

pub use pitui_core::*;
