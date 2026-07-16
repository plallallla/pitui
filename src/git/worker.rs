use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::config::{ResolvedConfig, ResolvedLoggingConfig};

use super::{
    GitJobId, GitRequest, GitResponseEnvelope, execute_request,
    logging::{BackendLogger, operation_log},
};

#[derive(Debug)]
struct GitJob {
    id: GitJobId,
    cwd: PathBuf,
    request: GitRequest,
}

/// Single-consumer Git command bus. Serial execution prevents two mutating Git
/// operations from racing; job ids still let the application discard stale
/// read responses after rapid navigation.
pub struct GitCommandBus {
    request_tx: Sender<GitJob>,
    response_rx: Receiver<GitResponseEnvelope>,
    next_id: GitJobId,
    logger: Option<BackendLogger>,
    logging_warning: Option<String>,
}

impl GitCommandBus {
    pub fn spawn() -> Self {
        let config = ResolvedConfig::default();
        Self::spawn_with_logging_config(&config.logging)
            .expect("default best-effort logging must not prevent startup")
    }

    pub fn spawn_with_logging_config(config: &ResolvedLoggingConfig) -> io::Result<Self> {
        if !config.enabled {
            return Ok(Self::spawn_with_logger(None, None));
        }
        let requested_path = config.path.clone();
        let bus = match BackendLogger::open_config(config) {
            Ok(logger) => Self::spawn_with_logger(Some(logger), None),
            Err(primary_error) if config.fail_on_open_error => return Err(primary_error),
            Err(primary_error) => {
                let fallback_path = std::env::temp_dir()
                    .join("pitui")
                    .join(format!("pitui-{}.jsonl", std::process::id()));
                let mut fallback = config.clone();
                fallback.path = fallback_path.clone();
                match BackendLogger::open_config(&fallback) {
                    Ok(logger) => Self::spawn_with_logger(
                        Some(logger),
                        Some(format!(
                            "Could not open {} ({primary_error}); logging to {} instead",
                            requested_path.display(),
                            fallback_path.display()
                        )),
                    ),
                    Err(fallback_error) => Self::spawn_with_logger(
                        None,
                        Some(format!(
                            "Backend logging unavailable: {} ({primary_error}); {} ({fallback_error})",
                            requested_path.display(),
                            fallback_path.display()
                        )),
                    ),
                }
            }
        };
        Ok(bus)
    }

    /// Creates a command bus that writes its backend operation log to an
    /// explicit path. This is useful for embedders and deterministic tests.
    pub fn spawn_with_log_path(path: impl Into<PathBuf>) -> io::Result<Self> {
        let mut config = ResolvedConfig::default().logging;
        config.path = path.into();
        config.fail_on_open_error = true;
        Self::spawn_with_logging_config(&config)
    }

    fn spawn_with_logger(logger: Option<BackendLogger>, logging_warning: Option<String>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<GitJob>();
        let (response_tx, response_rx) = mpsc::channel::<GitResponseEnvelope>();
        let worker_logger = logger.clone();

        thread::Builder::new()
            .name("pitui-git-worker".into())
            .spawn(move || {
                loop {
                    let job = match request_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some(logger) = worker_logger.as_ref() {
                                logger.flush_due();
                            }
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            if let Some(logger) = worker_logger.as_ref() {
                                logger.flush();
                            }
                            break;
                        }
                    };
                    let operation = operation_log(&job.request);
                    if let Some(logger) = worker_logger.as_ref() {
                        logger.started(job.id, &job.cwd, &operation);
                    }
                    let started_at = Instant::now();
                    let response = execute_request(&job.cwd, job.request);
                    if let Some(logger) = worker_logger.as_ref() {
                        logger.completed(
                            job.id,
                            &job.cwd,
                            &operation,
                            &response,
                            started_at.elapsed(),
                        );
                    }
                    if response_tx
                        .send(GitResponseEnvelope {
                            id: job.id,
                            response,
                        })
                        .is_err()
                    {
                        if let Some(logger) = worker_logger.as_ref() {
                            logger.channel_closed(
                                job.id,
                                &job.cwd,
                                &operation,
                                "response receiver disconnected",
                            );
                        }
                        break;
                    }
                }
            })
            .expect("failed to spawn Git worker");

        Self {
            request_tx,
            response_rx,
            next_id: 1,
            logger,
            logging_warning,
        }
    }

    pub fn submit(&mut self, cwd: PathBuf, request: GitRequest) -> GitJobId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let operation = operation_log(&request);
        if let Some(logger) = self.logger.as_ref() {
            logger.queued(id, &cwd, &operation);
        }
        // If the worker disappeared there is no useful recovery inside the UI;
        // the response side will disconnect and the app can still quit cleanly.
        if self
            .request_tx
            .send(GitJob {
                id,
                cwd: cwd.clone(),
                request,
            })
            .is_err()
            && let Some(logger) = self.logger.as_ref()
        {
            logger.channel_closed(id, &cwd, &operation, "request receiver disconnected");
        }
        id
    }

    pub fn try_recv(&self) -> Result<GitResponseEnvelope, TryRecvError> {
        self.response_rx.try_recv()
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.logger.as_ref().map(BackendLogger::path)
    }

    pub fn logging_warning(&self) -> Option<&str> {
        self.logging_warning.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_can_be_disabled_without_creating_a_sink() {
        let mut config = ResolvedConfig::default().logging;
        config.enabled = false;
        let bus = GitCommandBus::spawn_with_logging_config(&config).unwrap();
        assert_eq!(bus.log_path(), None);
        assert_eq!(bus.logging_warning(), None);
    }

    #[test]
    fn strict_log_open_failures_are_returned_before_the_tui_starts() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = ResolvedConfig::default().logging;
        config.path = directory.path().to_path_buf();
        config.fail_on_open_error = true;
        assert!(GitCommandBus::spawn_with_logging_config(&config).is_err());
    }

    #[test]
    fn best_effort_log_open_failures_fall_back_and_report_the_actual_path() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = ResolvedConfig::default().logging;
        config.path = directory.path().to_path_buf();
        config.fail_on_open_error = false;
        let bus = GitCommandBus::spawn_with_logging_config(&config).unwrap();
        assert!(bus.log_path().is_some_and(|path| path != directory.path()));
        assert!(
            bus.logging_warning()
                .is_some_and(|warning| warning.contains("logging to"))
        );
    }
}
