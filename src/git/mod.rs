mod logging;
mod parser;
mod protocol;
mod runner;
mod worker;

pub use logging::default_backend_log_path;
pub use parser::*;
pub use protocol::*;
pub use runner::execute_request;
pub use worker::GitCommandBus;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitFailure {
    pub command: String,
    pub stderr: String,
}

impl From<pitui_git::GitFailure> for GitFailure {
    fn from(failure: pitui_git::GitFailure) -> Self {
        Self {
            command: failure.command,
            stderr: failure.stderr,
        }
    }
}
