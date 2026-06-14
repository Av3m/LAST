//! Operation logging to `%LAST_ROOT%\log\last.log`.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

/// The kind of operation being logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Install,
    Update,
    Remove,
    Mirror,
    Export,
    Migrate,
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Remove => "remove",
            Self::Mirror => "mirror",
            Self::Export => "export",
            Self::Migrate => "migrate",
        };
        f.write_str(s)
    }
}

/// Outcome of a logged operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogResult {
    Success,
    Failure,
}

impl fmt::Display for LogResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Success => "success",
            Self::Failure => "failure",
        })
    }
}

/// A single entry written to `last.log`.
pub struct LogEntry {
    pub operation: Operation,
    pub package: String,
    pub version: String,
    pub hashes: Vec<String>,
    pub result: LogResult,
}

impl LogEntry {
    pub fn new(operation: Operation, package: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            operation,
            package: package.into(),
            version: version.into(),
            hashes: Vec::new(),
            result: LogResult::Success,
        }
    }

    pub fn with_hash(mut self, hash: impl Into<String>) -> Self {
        self.hashes.push(hash.into());
        self
    }

    pub fn with_result(mut self, result: LogResult) -> Self {
        self.result = result;
        self
    }
}

/// Appends [`LogEntry`] records to `%LAST_ROOT%\log\last.log`.
pub struct OperationLogger {
    log_file: PathBuf,
}

impl OperationLogger {
    pub fn new(root: &Path) -> Self {
        Self {
            log_file: root.join("log").join("last.log"),
        }
    }

    pub fn log(&self, entry: &LogEntry) -> Result<()> {
        if let Some(parent) = self.log_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create log directory {}", parent.display()))?;
        }
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S%:z");
        let hashes = if entry.hashes.is_empty() {
            "-".to_string()
        } else {
            entry.hashes.join(",")
        };
        let line = format!(
            "{timestamp} {op} {package} {version} hashes={hashes} result={result}\n",
            op = entry.operation,
            package = entry.package,
            version = entry.version,
            result = entry.result,
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .with_context(|| format!("failed to open log file {}", self.log_file.display()))?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }
}
