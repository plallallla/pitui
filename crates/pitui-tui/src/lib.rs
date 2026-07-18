//! Terminal adapter for the Data Driven runtime.
//!
//! It will translate terminal events into input data and present immutable
//! frame projections. It must not receive mutable ECS World access.

#![forbid(unsafe_code)]

mod input;
mod render;
mod terminal;

pub use input::{TerminalEvent, event_to_intent, event_to_terminal_event, key_event_to_stroke};
pub use render::render;
pub use terminal::TerminalSession;
