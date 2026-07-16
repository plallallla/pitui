use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Instant,
};

use super::{
    GitJobId, GitRequest, GitResponseEnvelope, execute_request,
    logging::{BackendLogger, default_backend_log_path, operation_log},
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
        let requested_path = default_backend_log_path();
        match BackendLogger::open(requested_path.clone()) {
            Ok(logger) => Self::spawn_with_logger(Some(logger), None),
            Err(primary_error) => {
                let fallback_path = std::env::temp_dir()
                    .join("pitui")
                    .join(format!("pitui-{}.jsonl", std::process::id()));
                match BackendLogger::open(fallback_path.clone()) {
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
        }
    }

    /// Creates a command bus that writes its backend operation log to an
    /// explicit path. This is useful for embedders and deterministic tests.
    pub fn spawn_with_log_path(path: impl Into<PathBuf>) -> io::Result<Self> {
        BackendLogger::open(path.into()).map(|logger| Self::spawn_with_logger(Some(logger), None))
    }

    fn spawn_with_logger(logger: Option<BackendLogger>, logging_warning: Option<String>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<GitJob>();
        let (response_tx, response_rx) = mpsc::channel::<GitResponseEnvelope>();
        let worker_logger = logger.clone();

        thread::Builder::new()
            .name("pitui-git-worker".into())
            .spawn(move || {
                while let Ok(job) = request_rx.recv() {
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
