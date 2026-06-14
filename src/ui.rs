//! Console output formatting.
//!
//! `Ui` is the single place that knows how to talk to the user. It is shared
//! (via `Arc`) between the CLI command handlers and the script engine, so
//! that `log()`/`warn()`/`error()`/`success()` calls from Rhai scripts use
//! exactly the same formatting as the rest of the application.

use std::sync::atomic::{AtomicBool, Ordering};

/// Central console writer.
///
/// All fields are atomics so `Ui` can be shared behind an `Arc` without a
/// `Mutex`, including from inside the sandboxed script engine.
pub struct Ui {
    verbose: AtomicBool,
    dry_run: AtomicBool,
}

impl Ui {
    pub fn new(verbose: bool, dry_run: bool) -> Self {
        Self {
            verbose: AtomicBool::new(verbose),
            dry_run: AtomicBool::new(dry_run),
        }
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose.load(Ordering::Relaxed)
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run.load(Ordering::Relaxed)
    }

    /// Plain informational message.
    pub fn info(&self, msg: impl AsRef<str>) {
        println!("{}", msg.as_ref());
    }

    /// Debug message, only printed with `--verbose`.
    pub fn debug(&self, msg: impl AsRef<str>) {
        if self.is_verbose() {
            println!("\x1b[90m[debug]\x1b[0m {}", msg.as_ref());
        }
    }

    /// Warning message (does not abort the current operation).
    pub fn warn(&self, msg: impl AsRef<str>) {
        eprintln!("\x1b[33m[warn]\x1b[0m {}", msg.as_ref());
    }

    /// Error message (does not abort by itself - the caller decides).
    pub fn error(&self, msg: impl AsRef<str>) {
        eprintln!("\x1b[31m[error]\x1b[0m {}", msg.as_ref());
    }

    /// Success message.
    pub fn success(&self, msg: impl AsRef<str>) {
        println!("\x1b[32m{}\x1b[0m", msg.as_ref());
    }

    /// Marks a step that would change state. Prefixes with `[dry-run]` when
    /// `--dry-run` is active and skips the actual action - callers should
    /// check [`Ui::is_dry_run`] before performing the side effect.
    pub fn step(&self, msg: impl AsRef<str>) {
        if self.is_dry_run() {
            println!("\x1b[36m[dry-run]\x1b[0m {}", msg.as_ref());
        } else {
            println!("{}", msg.as_ref());
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new(false, false)
    }
}
