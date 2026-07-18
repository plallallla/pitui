use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::sanitize_log_text;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GitLogLevel {
    Info,
    Warn,
    Error,
}

impl GitLogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitLogStatus {
    Success,
    Failure,
    ConflictAborted,
}

impl GitLogStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::ConflictAborted => "conflict-aborted",
        }
    }

    fn level(self) -> GitLogLevel {
        match self {
            Self::Success => GitLogLevel::Info,
            Self::ConflictAborted => GitLogLevel::Warn,
            Self::Failure => GitLogLevel::Error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitOperationRecord {
    pub operation: String,
    pub repository: PathBuf,
    pub started_at: SystemTime,
    pub duration: Duration,
    pub status: GitLogStatus,
    pub message: String,
    pub abort_attempted: bool,
    pub abort_result: Option<String>,
}

pub trait GitOperationLogSink: Send + Sync + 'static {
    fn record(&self, record: &GitOperationRecord);

    fn flush(&self) {}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopGitOperationLogSink;

impl GitOperationLogSink for NoopGitOperationLogSink {
    fn record(&self, _record: &GitOperationRecord) {}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonlGitLogConfig {
    pub path: PathBuf,
    pub level: GitLogLevel,
    pub max_bytes: u64,
    pub keep_files: usize,
    pub rotate_on_start: bool,
    pub flush_interval: Duration,
    pub buffer_capacity: usize,
    pub max_message_chars: usize,
}

#[derive(Clone)]
pub struct JsonlGitOperationLogSink {
    inner: Arc<Mutex<LogFile>>,
    config: Arc<JsonlGitLogConfig>,
}

struct LogFile {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    bytes_written: u64,
    last_flush: Instant,
}

impl JsonlGitOperationLogSink {
    pub fn open(config: JsonlGitLogConfig) -> io::Result<Self> {
        if config.max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Git log max_bytes must be greater than zero",
            ));
        }
        if config.buffer_capacity == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Git log buffer_capacity must be greater than zero",
            ));
        }
        if let Some(parent) = config
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = LogFile {
            bytes_written: fs::metadata(&config.path).map_or(0, |metadata| metadata.len()),
            file: None,
            path: config.path.clone(),
            last_flush: Instant::now(),
        };
        if config.rotate_on_start && file.bytes_written > 0 {
            file.rotate(&config)?;
        } else {
            file.file = Some(open_append(&config.path, config.buffer_capacity)?);
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
            config: Arc::new(config),
        })
    }

    pub fn path(&self) -> &Path {
        &self.config.path
    }

    fn with_file<T>(&self, callback: impl FnOnce(&mut LogFile) -> T) -> T {
        match self.inner.lock() {
            Ok(mut file) => callback(&mut file),
            Err(poisoned) => callback(&mut poisoned.into_inner()),
        }
    }
}

impl GitOperationLogSink for JsonlGitOperationLogSink {
    fn record(&self, record: &GitOperationRecord) {
        if record.status.level() < self.config.level {
            return;
        }
        let message = sanitize_log_text(&record.message, self.config.max_message_chars);
        let abort_result = record
            .abort_result
            .as_deref()
            .map(|result| sanitize_log_text(result, self.config.max_message_chars));
        let started_at_unix_ms = record
            .started_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut line = format!(
            "{{\"ts_unix_ms\":{},\"level\":\"{}\",\"operation\":\"{}\",\"repository\":\"{}\",\"started_at_unix_ms\":{},\"duration_ms\":{},\"status\":\"{}\",\"message\":\"{}\",\"abort_attempted\":{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            record.status.level().as_str(),
            json_escape(&record.operation),
            json_escape(&record.repository.to_string_lossy()),
            started_at_unix_ms,
            record.duration.as_millis(),
            record.status.as_str(),
            json_escape(&message),
            record.abort_attempted,
        );
        if let Some(abort_result) = abort_result {
            line.push_str(&format!(
                ",\"abort_result\":\"{}\"",
                json_escape(&abort_result)
            ));
        }
        line.push_str("}\n");
        self.with_file(|file| {
            // Once opened, logging is intentionally best-effort and cannot
            // interrupt Git execution or Dataset replacement.
            let _ = file.write_line(line.as_bytes(), &self.config);
        });
    }

    fn flush(&self) {
        self.with_file(|file| {
            let _ = file.flush();
        });
    }
}

impl LogFile {
    fn write_line(&mut self, line: &[u8], config: &JsonlGitLogConfig) -> io::Result<()> {
        if self.bytes_written > 0
            && self.bytes_written.saturating_add(line.len() as u64) > config.max_bytes
        {
            self.rotate(config)?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("Git operation log file is closed"))?;
        file.write_all(line)?;
        self.bytes_written = self.bytes_written.saturating_add(line.len() as u64);
        if config.flush_interval.is_zero() || self.last_flush.elapsed() >= config.flush_interval {
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

    fn rotate(&mut self, config: &JsonlGitLogConfig) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        let result = (|| {
            if config.keep_files == 0 {
                remove_if_exists(&self.path)?;
            } else {
                remove_if_exists(&rotated_path(&self.path, config.keep_files))?;
                for index in (2..=config.keep_files).rev() {
                    let previous = rotated_path(&self.path, index - 1);
                    if previous.exists() {
                        fs::rename(previous, rotated_path(&self.path, index))?;
                    }
                }
                if self.path.exists() {
                    fs::rename(&self.path, rotated_path(&self.path, 1))?;
                }
            }
            open_truncated(&self.path, config.buffer_capacity)
        })();
        match result {
            Ok(file) => {
                self.file = Some(file);
                self.bytes_written = 0;
                self.last_flush = Instant::now();
                Ok(())
            }
            Err(error) => {
                self.file = open_append(&self.path, config.buffer_capacity).ok();
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

fn open_append(path: &Path, capacity: usize) -> io::Result<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|file| BufWriter::with_capacity(capacity, file))
}

fn open_truncated(path: &Path, capacity: usize) -> io::Result<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map(|file| BufWriter::with_capacity(capacity, file))
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "pitui-git.jsonl".into(), |name| name.to_os_string());
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

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: PathBuf) -> JsonlGitLogConfig {
        JsonlGitLogConfig {
            path,
            level: GitLogLevel::Info,
            max_bytes: 4096,
            keep_files: 2,
            rotate_on_start: false,
            flush_interval: Duration::ZERO,
            buffer_capacity: 1024,
            max_message_chars: 512,
        }
    }

    #[test]
    fn writes_redacted_bounded_jsonl_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("git.jsonl");
        let sink = JsonlGitOperationLogSink::open(config(path.clone())).unwrap();
        sink.record(&GitOperationRecord {
            operation: "push".into(),
            repository: directory.path().into(),
            started_at: UNIX_EPOCH + Duration::from_secs(1),
            duration: Duration::from_millis(12),
            status: GitLogStatus::Failure,
            message: "failed https://user:token@example.invalid/repo\ntry again".into(),
            abort_attempted: false,
            abort_result: None,
        });
        sink.flush();

        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.contains("\"operation\":\"push\""));
        assert!(contents.contains("\"status\":\"failure\""));
        assert!(contents.contains("<redacted-url>\\ntry again"));
        assert!(!contents.contains("token"));
    }

    #[test]
    fn rotates_without_interrupting_subsequent_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("git.jsonl");
        let mut settings = config(path.clone());
        settings.max_bytes = 300;
        settings.keep_files = 1;
        let sink = JsonlGitOperationLogSink::open(settings).unwrap();
        for index in 0..8 {
            sink.record(&GitOperationRecord {
                operation: "load_repository".into(),
                repository: directory.path().into(),
                started_at: UNIX_EPOCH,
                duration: Duration::from_millis(index),
                status: GitLogStatus::Success,
                message: format!("record {index} with enough text to trigger rotation"),
                abort_attempted: false,
                abort_result: None,
            });
        }
        sink.flush();
        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(!fs::read_to_string(path).unwrap().is_empty());
    }
}
