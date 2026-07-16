use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::config::{LogLevel, ResolvedLoggingConfig};

use super::{GitJobId, GitRequest, GitResponse};

#[cfg(test)]
const DEFAULT_MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationLog {
    pub name: &'static str,
    pub details: String,
}

#[derive(Clone)]
pub(crate) struct BackendLogger {
    inner: Arc<Mutex<LogFile>>,
    session_id: Arc<str>,
    path: Arc<PathBuf>,
    level: LogLevel,
    max_detail_chars: usize,
}

struct LogFile {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    bytes_written: u64,
    max_bytes: u64,
    rotation_enabled: bool,
    keep_files: usize,
    flush_interval: Duration,
    last_flush: Instant,
    buffer_capacity: usize,
}

struct LogEvent<'a> {
    level: LogLevel,
    event: &'a str,
    job_id: Option<GitJobId>,
    cwd: Option<&'a Path>,
    operation: Option<&'a OperationLog>,
    summary: Option<&'a str>,
    outcome: Option<(&'a str, u128)>,
}

impl BackendLogger {
    #[cfg(test)]
    pub(crate) fn open(path: PathBuf) -> io::Result<Self> {
        Self::open_with_limit(path, DEFAULT_MAX_LOG_BYTES)
    }

    #[cfg(test)]
    fn open_with_limit(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let config = ResolvedLoggingConfig {
            enabled: true,
            level: LogLevel::Info,
            path,
            flush_interval: Duration::ZERO,
            buffer_capacity: 1024,
            max_detail_chars: 4096,
            fail_on_open_error: false,
            rotation: crate::config::LogRotationConfig {
                enabled: true,
                max_bytes: max_bytes.max(1),
                keep_files: 1,
                rotate_on_start: false,
            },
            targets: Default::default(),
        };
        Self::open_config(&config)
    }

    pub(crate) fn open_config(config: &ResolvedLoggingConfig) -> io::Result<Self> {
        let path = config.path.clone();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let bytes_written = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let started_at = unix_millis();
        let sequence = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let shared_path = Arc::new(path.clone());
        let logger = Self {
            inner: Arc::new(Mutex::new(LogFile {
                path,
                file: Some(BufWriter::with_capacity(config.buffer_capacity, file)),
                bytes_written,
                max_bytes: config.rotation.max_bytes.max(1),
                rotation_enabled: config.rotation.enabled,
                keep_files: config.rotation.keep_files,
                flush_interval: config.flush_interval,
                last_flush: Instant::now(),
                buffer_capacity: config.buffer_capacity,
            })),
            session_id: Arc::from(format!("{}-{started_at}-{sequence}", std::process::id())),
            path: shared_path,
            level: config.target_level("git_worker"),
            max_detail_chars: config.max_detail_chars,
        };
        if config.rotation.enabled && config.rotation.rotate_on_start && bytes_written > 0 {
            logger.with_file(LogFile::rotate)?;
        }
        logger.write_event(LogEvent {
            level: LogLevel::Info,
            event: "session_started",
            job_id: None,
            cwd: None,
            operation: None,
            summary: Some(&format!(
                "version={} log_format=jsonl rotation_bytes={}",
                env!("CARGO_PKG_VERSION"),
                config.rotation.max_bytes.max(1)
            )),
            outcome: None,
        });
        Ok(logger)
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub(crate) fn queued(&self, id: GitJobId, cwd: &Path, operation: &OperationLog) {
        self.write_event(LogEvent {
            level: LogLevel::Info,
            event: "queued",
            job_id: Some(id),
            cwd: Some(cwd),
            operation: Some(operation),
            summary: None,
            outcome: None,
        });
    }

    pub(crate) fn started(&self, id: GitJobId, cwd: &Path, operation: &OperationLog) {
        self.write_event(LogEvent {
            level: LogLevel::Info,
            event: "started",
            job_id: Some(id),
            cwd: Some(cwd),
            operation: Some(operation),
            summary: None,
            outcome: None,
        });
    }

    pub(crate) fn completed(
        &self,
        id: GitJobId,
        cwd: &Path,
        operation: &OperationLog,
        response: &GitResponse,
        elapsed: Duration,
    ) {
        let (level, outcome, summary) = response_log(response, operation.name);
        self.write_event(LogEvent {
            level,
            event: "completed",
            job_id: Some(id),
            cwd: Some(cwd),
            operation: Some(operation),
            summary: Some(&summary),
            outcome: Some((outcome, elapsed.as_millis())),
        });
    }

    pub(crate) fn channel_closed(
        &self,
        id: GitJobId,
        cwd: &Path,
        operation: &OperationLog,
        channel: &str,
    ) {
        self.write_event(LogEvent {
            level: LogLevel::Error,
            event: "channel_closed",
            job_id: Some(id),
            cwd: Some(cwd),
            operation: Some(operation),
            summary: Some(channel),
            outcome: None,
        });
    }

    fn write_event(&self, event: LogEvent<'_>) {
        if event.level > self.level {
            return;
        }
        let mut line = format!(
            "{{\"ts_unix_ms\":{},\"level\":\"{}\",\"component\":\"git-worker\",\"session_id\":\"{}\",\"event\":\"{}\"",
            unix_millis(),
            event.level.as_str().to_ascii_uppercase(),
            json_escape(&self.session_id),
            json_escape(event.event)
        );
        if let Some(job_id) = event.job_id {
            line.push_str(&format!(",\"job_id\":{job_id}"));
        }
        if let Some(cwd) = event.cwd {
            line.push_str(&format!(
                ",\"cwd\":\"{}\"",
                json_escape(&cwd.to_string_lossy())
            ));
        }
        if let Some(operation) = event.operation {
            line.push_str(&format!(
                ",\"operation\":\"{}\"",
                json_escape(operation.name)
            ));
            if !operation.details.is_empty() {
                line.push_str(&format!(
                    ",\"details\":\"{}\"",
                    json_escape(&truncate(&operation.details, self.max_detail_chars))
                ));
            }
        }
        if let Some(summary) = event.summary.filter(|summary| !summary.is_empty()) {
            line.push_str(&format!(
                ",\"summary\":\"{}\"",
                json_escape(&truncate(summary, self.max_detail_chars))
            ));
        }
        if let Some((status, duration_ms)) = event.outcome {
            line.push_str(&format!(
                ",\"status\":\"{}\",\"duration_ms\":{duration_ms}",
                json_escape(status)
            ));
        }
        line.push_str("}\n");

        self.with_file(|file| {
            // Logging must never stop Git/UI progress. Runtime I/O failures are
            // intentionally best-effort after initialization.
            let _ = file.write_line(line.as_bytes());
        });
    }

    fn with_file<T>(&self, callback: impl FnOnce(&mut LogFile) -> T) -> T {
        match self.inner.lock() {
            Ok(mut file) => callback(&mut file),
            Err(poisoned) => callback(&mut poisoned.into_inner()),
        }
    }

    pub(crate) fn flush_due(&self) {
        self.with_file(|file| {
            let _ = file.flush_due();
        });
    }

    pub(crate) fn flush(&self) {
        self.with_file(|file| {
            let _ = file.flush();
        });
    }
}

impl LogFile {
    fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if self.rotation_enabled
            && self.bytes_written > 0
            && self.bytes_written.saturating_add(line.len() as u64) > self.max_bytes
        {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("backend log file is closed"))?;
        file.write_all(line)?;
        self.bytes_written = self.bytes_written.saturating_add(line.len() as u64);
        if self.flush_interval.is_zero() || self.last_flush.elapsed() >= self.flush_interval {
            self.flush()?;
        }
        Ok(())
    }

    fn flush_due(&mut self) -> io::Result<()> {
        if !self.flush_interval.is_zero() && self.last_flush.elapsed() >= self.flush_interval {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
            self.last_flush = Instant::now();
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }

        let rotate_result = (|| -> io::Result<BufWriter<File>> {
            if self.keep_files == 0 {
                remove_if_exists(&self.path)?;
            } else {
                remove_if_exists(&rotated_path(&self.path, self.keep_files))?;
                for index in (2..=self.keep_files).rev() {
                    let previous = rotated_path(&self.path, index - 1);
                    if previous.exists() {
                        fs::rename(previous, rotated_path(&self.path, index))?;
                    }
                }
                if self.path.exists() {
                    fs::rename(&self.path, rotated_path(&self.path, 1))?;
                }
            }
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?;
            Ok(BufWriter::with_capacity(self.buffer_capacity, file))
        })();

        match rotate_result {
            Ok(file) => {
                self.file = Some(file);
                self.bytes_written = 0;
                self.last_flush = Instant::now();
                Ok(())
            }
            Err(error) => {
                // Re-open the current path so a failed rotation does not
                // permanently disable subsequent log writes.
                self.file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .ok()
                    .map(|file| BufWriter::with_capacity(self.buffer_capacity, file));
                self.bytes_written = fs::metadata(&self.path).map_or(0, |metadata| metadata.len());
                Err(error)
            }
        }
    }
}

impl Drop for LogFile {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

pub fn default_backend_log_path() -> PathBuf {
    crate::config::default_backend_log_path()
}

pub(crate) fn operation_log(request: &GitRequest) -> OperationLog {
    let (name, details) = match request {
        GitRequest::LoadRepositoryStatus => ("load_repository_status", String::new()),
        GitRequest::LoadBranches => ("load_branches", String::new()),
        GitRequest::LoadRemotes => ("load_remotes", String::new()),
        GitRequest::LoadCommits { branch, limit } => {
            ("load_commits", format!("branch={} limit={limit}", branch.0))
        }
        GitRequest::LoadCommitDetail { commit } => {
            ("load_commit_detail", format!("commit={}", commit.0))
        }
        GitRequest::LoadCommitMessage { commit } => {
            ("load_commit_message", format!("commit={}", commit.0))
        }
        GitRequest::LoadFileDiff {
            commit,
            path,
            old_path,
        } => (
            "load_file_diff",
            format!(
                "commit={} path={}{}",
                commit.0,
                path,
                old_path
                    .as_ref()
                    .map(|path| format!(" old_path={path}"))
                    .unwrap_or_default()
            ),
        ),
        GitRequest::LoadReflog { limit } => ("load_reflog", format!("limit={limit}")),
        GitRequest::LoadWorkingTree => ("load_working_tree", String::new()),
        GitRequest::LoadWorkingTreeDiff {
            path,
            old_path,
            include_staged,
            include_worktree,
            untracked,
        } => (
            "load_working_tree_diff",
            format!(
                "path={path}{} staged={include_staged} worktree={include_worktree} untracked={untracked}",
                old_path
                    .as_ref()
                    .map(|path| format!(" old_path={path}"))
                    .unwrap_or_default()
            ),
        ),
        GitRequest::StagePaths { paths } => ("stage_paths", path_list(paths)),
        GitRequest::UnstagePaths { paths } => ("unstage_paths", path_list(paths)),
        GitRequest::Commit { message } => ("commit", format!("message_bytes={}", message.len())),
        GitRequest::Fetch => ("fetch", String::new()),
        GitRequest::PullRebase => ("pull_rebase", String::from("strategy=rebase")),
        GitRequest::Push => ("push", String::new()),
        GitRequest::AddRemote { name, .. } => ("add_remote", format!("remote={name}")),
        GitRequest::SetRemoteUrl { name, .. } => ("set_remote_url", format!("remote={name}")),
        GitRequest::SetUpstreamRemote { name } => ("set_upstream_remote", format!("remote={name}")),
        GitRequest::SwitchBranch { branch } => ("switch_branch", format!("branch={}", branch.0)),
        GitRequest::CherryPick { commits } => (
            "cherry_pick",
            format!(
                "commits={}",
                commits
                    .iter()
                    .map(|commit| commit.0.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        GitRequest::Reset { commit, mode } => {
            ("reset", format!("mode={} commit={}", mode.flag(), commit.0))
        }
        GitRequest::Rebase { upstream } => ("rebase", format!("upstream={}", upstream.0)),
    };
    OperationLog { name, details }
}

fn response_log(response: &GitResponse, operation: &str) -> (LogLevel, &'static str, String) {
    match response {
        GitResponse::RepositoryStatusLoaded(repository) => (
            LogLevel::Info,
            "success",
            format!(
                "branch={} head={} staged={} modified={} untracked={} conflicted={}",
                repository
                    .current_branch
                    .as_ref()
                    .map_or("detached", |branch| branch.0.as_str()),
                repository.head.0,
                repository.status.staged,
                repository.status.modified,
                repository.status.untracked,
                repository.status.conflicted
            ),
        ),
        GitResponse::BranchesLoaded(branches) => (
            LogLevel::Info,
            "success",
            format!("branches={}", branches.len()),
        ),
        GitResponse::RemotesLoaded(remotes) => (
            LogLevel::Info,
            "success",
            format!(
                "remotes={} inconsistent={} upstream={}",
                remotes.len(),
                remotes.iter().filter(|remote| !remote.urls_match()).count(),
                remotes
                    .iter()
                    .find(|remote| remote.is_upstream)
                    .map_or("none", |remote| remote.name.as_str())
            ),
        ),
        GitResponse::CommitsLoaded { branch, commits } => (
            LogLevel::Info,
            "success",
            format!("branch={} commits={}", branch.0, commits.len()),
        ),
        GitResponse::CommitDetailLoaded(detail) => (
            LogLevel::Info,
            "success",
            format!(
                "commit={} files={}",
                detail.commit.hash.0,
                detail.files.len()
            ),
        ),
        GitResponse::CommitMessageLoaded { commit, message } => (
            LogLevel::Info,
            "success",
            format!("commit={} message_bytes={}", commit.0, message.len()),
        ),
        GitResponse::FileDiffLoaded(diff) => (
            LogLevel::Info,
            "success",
            format!(
                "path={} hunks={} binary={}",
                diff.path,
                diff.hunks.len(),
                diff.is_binary
            ),
        ),
        GitResponse::ReflogLoaded(entries) => (
            LogLevel::Info,
            "success",
            format!("entries={}", entries.len()),
        ),
        GitResponse::WorkingTreeLoaded(changes) => (
            LogLevel::Info,
            "success",
            format!("changes={}", changes.len()),
        ),
        GitResponse::WorkingTreeDiffLoaded(diff) => (
            LogLevel::Info,
            "success",
            format!("path={} sections={}", diff.path, diff.sections.len()),
        ),
        GitResponse::CommandSucceeded { .. } => (LogLevel::Info, "success", String::new()),
        GitResponse::CommandFailed { command, stderr } => {
            let command = match operation {
                "commit" => "git commit <redacted>",
                "add_remote" => "git remote add <name> <redacted-url>",
                "set_remote_url" => "git remote set-url <name> <redacted-url>",
                _ => command,
            };
            (
                LogLevel::Error,
                "failure",
                format!(
                    "command={} stderr={}",
                    redact_url_tokens(command),
                    redact_url_tokens(stderr)
                ),
            )
        }
        GitResponse::RebaseConflictAborted { command, stderr } => (
            LogLevel::Warn,
            "conflict_aborted",
            format!(
                "command={} stderr={}",
                redact_url_tokens(command),
                redact_url_tokens(stderr)
            ),
        ),
    }
}

fn path_list(paths: &[crate::domain::GitPath]) -> String {
    format!(
        "count={} paths={}",
        paths.len(),
        paths
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn redact_url_tokens(value: &str) -> String {
    value
        .split_inclusive(char::is_whitespace)
        .map(|segment| {
            let content_len = segment.trim_end_matches(char::is_whitespace).len();
            let (content, whitespace) = segment.split_at(content_len);
            let lower = content.to_ascii_lowercase();
            let looks_like_url = lower.contains("://")
                || lower.starts_with("git@")
                || (lower.contains('@') && lower.contains(':'));
            if looks_like_url {
                format!("<redacted-url>{whitespace}")
            } else {
                segment.to_string()
            }
        })
        .collect()
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "pitui.jsonl".into(), |name| name.to_os_string());
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::{
        domain::GitPath,
        git::{GitRequest, GitResponse},
    };

    #[test]
    fn writes_jsonl_lifecycle_redacts_commit_message_and_escapes_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backend.jsonl");
        let logger = BackendLogger::open(path.clone()).unwrap();
        let request = GitRequest::Commit {
            message: "secret subject\nsecond line".into(),
        };
        let operation = operation_log(&request);
        logger.queued(7, Path::new("/repo\nname"), &operation);
        logger.started(7, Path::new("/repo\nname"), &operation);
        logger.completed(
            7,
            Path::new("/repo\nname"),
            &operation,
            &GitResponse::CommandFailed {
                command: "git commit -m 'secret subject'".into(),
                stderr: "hook said \"no\"\ntry again".into(),
            },
            Duration::from_millis(12),
        );

        let contents = fs::read_to_string(path).unwrap();
        assert!(
            contents
                .lines()
                .all(|line| line.starts_with('{') && line.ends_with('}'))
        );
        assert!(contents.contains("\"event\":\"queued\""));
        assert!(contents.contains("\"event\":\"started\""));
        assert!(contents.contains("\"event\":\"completed\""));
        assert!(contents.contains("\"operation\":\"commit\""));
        assert!(contents.contains("message_bytes=26"));
        assert!(contents.contains("git commit <redacted>"));
        assert!(contents.contains("hook said \\\"no\\\"\\ntry again"));
        assert!(contents.contains("\"duration_ms\":12"));
        assert!(!contents.contains("secret subject"));
    }

    #[test]
    fn rotates_at_the_configured_size_and_keeps_a_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backend.jsonl");
        let logger = BackendLogger::open_with_limit(path.clone(), 350).unwrap();
        let request = GitRequest::StagePaths {
            paths: vec![GitPath::from("a-file-with-a-long-name.txt")],
        };
        let operation = operation_log(&request);
        for id in 1..=8 {
            logger.queued(id, directory.path(), &operation);
        }

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(!fs::read_to_string(&path).unwrap().is_empty());
        assert!(
            !fs::read_to_string(rotated_path(&path, 1))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn honors_level_flush_interval_and_multiple_retained_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backend.jsonl");
        let mut config = ResolvedLoggingConfig {
            enabled: true,
            level: LogLevel::Error,
            path: path.clone(),
            flush_interval: Duration::from_millis(50),
            buffer_capacity: 1024,
            max_detail_chars: 4096,
            fail_on_open_error: true,
            rotation: crate::config::LogRotationConfig {
                enabled: true,
                max_bytes: 350,
                keep_files: 3,
                rotate_on_start: false,
            },
            targets: Default::default(),
        };
        let logger = BackendLogger::open_config(&config).unwrap();
        let request = GitRequest::StagePaths {
            paths: vec![GitPath::from("a-file-with-a-long-name.txt")],
        };
        let operation = operation_log(&request);
        logger.queued(1, directory.path(), &operation);
        logger.flush();
        assert!(fs::read_to_string(&path).unwrap().is_empty());

        logger.completed(
            1,
            directory.path(),
            &operation,
            &GitResponse::CommandFailed {
                command: "git add -- file".into(),
                stderr: "failed".into(),
            },
            Duration::from_millis(1),
        );
        thread::sleep(Duration::from_millis(60));
        logger.flush_due();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"level\":\"ERROR\"")
        );

        config.level = LogLevel::Info;
        let rotation_path = directory.path().join("rotation.jsonl");
        config.path = rotation_path.clone();
        let rotating = BackendLogger::open_config(&config).unwrap();
        for id in 1..=30 {
            rotating.queued(id, directory.path(), &operation);
            rotating.flush();
        }
        assert!(rotated_path(&rotation_path, 1).exists());
        assert!(rotated_path(&rotation_path, 2).exists());
        assert!(rotated_path(&rotation_path, 3).exists());
        assert!(!rotated_path(&rotation_path, 4).exists());
    }

    #[test]
    fn target_level_override_and_rotate_on_start_are_applied() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backend.jsonl");
        fs::write(&path, "{\"event\":\"previous-session\"}\n").unwrap();
        let mut config = ResolvedLoggingConfig {
            enabled: true,
            level: LogLevel::Error,
            path: path.clone(),
            flush_interval: Duration::ZERO,
            buffer_capacity: 1024,
            max_detail_chars: 4096,
            fail_on_open_error: true,
            rotation: crate::config::LogRotationConfig {
                enabled: true,
                max_bytes: 1024 * 1024,
                keep_files: 2,
                rotate_on_start: true,
            },
            targets: Default::default(),
        };
        config.targets.insert("git_worker".into(), LogLevel::Info);

        let logger = BackendLogger::open_config(&config).unwrap();
        let operation = operation_log(&GitRequest::LoadBranches);
        logger.queued(1, directory.path(), &operation);
        logger.flush();

        assert!(
            fs::read_to_string(rotated_path(&path, 1))
                .unwrap()
                .contains("previous-session")
        );
        let current = fs::read_to_string(path).unwrap();
        assert!(current.contains("session_started"));
        assert!(current.contains("\"event\":\"queued\""));
    }

    #[test]
    fn summarizes_every_mutating_request_without_commit_contents() {
        let requests = [
            GitRequest::StagePaths {
                paths: vec![GitPath::from("one.txt")],
            },
            GitRequest::UnstagePaths {
                paths: vec![GitPath::from("two.txt")],
            },
            GitRequest::Commit {
                message: "do not log me".into(),
            },
            GitRequest::AddRemote {
                name: "origin".into(),
                url: "https://user:secret@example.invalid/repo.git".into(),
            },
            GitRequest::SetRemoteUrl {
                name: "origin".into(),
                url: "ssh://secret.example.invalid/repo.git".into(),
            },
            GitRequest::SetUpstreamRemote {
                name: "origin".into(),
            },
        ];
        let logs = requests.iter().map(operation_log).collect::<Vec<_>>();
        assert_eq!(logs[0].name, "stage_paths");
        assert_eq!(logs[1].name, "unstage_paths");
        assert_eq!(logs[2].name, "commit");
        assert_eq!(logs[2].details, "message_bytes=13");
        assert!(!logs[2].details.contains("do not log me"));
        assert_eq!(logs[3].details, "remote=origin");
        assert_eq!(logs[4].details, "remote=origin");
        assert_eq!(logs[5].details, "remote=origin");
        assert!(logs.iter().all(|log| !log.details.contains("secret")));

        let (_, _, summary) = response_log(
            &GitResponse::CommandFailed {
                command: "git push https://user:secret@example.invalid/repo.git".into(),
                stderr:
                    "fatal: unable to access 'https://user:secret@example.invalid/repo.git': denied"
                        .into(),
            },
            "push",
        );
        assert!(summary.contains("<redacted-url>"));
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("example.invalid"));
    }
}
