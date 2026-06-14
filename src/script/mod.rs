//! Script engine abstraction (specification section 5).
//!
//! Manifest scripts (`pre_install`, `post_install`, `installer.script`,
//! `uninstaller.script`, `pre_uninstall`, `post_uninstall`) are executed
//! through a [`ScriptEngine`]. [`ScriptRunner`] is the entry point used by
//! `install.rs`: it runs scripts with the sandboxed [`rhai_engine::RhaiEngine`]
//! and, if the script looks like PowerShell and `powershell_compat` is
//! enabled, falls back to [`powershell::PowerShellEngine`].

pub mod powershell;
pub mod rhai_engine;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::ui::Ui;

/// Variables made available to every script (specification section 5.3).
#[derive(Debug, Clone)]
pub struct ScriptContext {
    /// Installation directory of the current version.
    pub dir: PathBuf,
    /// Persist directory for this app.
    pub persist_dir: PathBuf,
    pub version: String,
    pub app: String,
    /// `64bit`, `32bit` or `arm64`.
    pub architecture: String,
    /// LAST root directory (also exposed as `scoopdir` for compatibility).
    pub last_root: PathBuf,
    pub global: bool,
}

/// A script execution backend.
pub trait ScriptEngine {
    /// Runs `source` with the given context. Implementations should return
    /// an error if the script fails or cannot be executed - per
    /// specification 5.4, `error()` calls *within* a script do not abort it,
    /// but a Rust-level error (parse failure, panic, non-zero PowerShell
    /// exit code) does.
    fn run(&self, source: &str, context: &ScriptContext) -> Result<()>;
}

/// Dispatches scripts to the Rhai engine, with optional PowerShell
/// compatibility fallback (specification section 5.5).
pub struct ScriptRunner {
    rhai: rhai_engine::RhaiEngine,
    powershell: powershell::PowerShellEngine,
    powershell_compat: bool,
}

impl ScriptRunner {
    pub fn new(ui: Arc<Ui>, powershell_compat: bool) -> Self {
        Self {
            rhai: rhai_engine::RhaiEngine::new(ui.clone()),
            powershell: powershell::PowerShellEngine::new(ui),
            powershell_compat,
        }
    }

    /// Runs a manifest script. An explicit `#!powershell` marker on the
    /// first line always selects the PowerShell engine; otherwise the
    /// script is executed as Rhai, falling back to PowerShell if it fails to
    /// parse and looks like PowerShell syntax.
    pub fn run(&self, source: &str, context: &ScriptContext) -> Result<()> {
        let trimmed = source.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#!powershell") {
            if !self.powershell_compat {
                anyhow::bail!("script requires PowerShell compatibility mode, which is disabled");
            }
            return self.powershell.run(rest, context);
        }

        match self.rhai.run(source, context) {
            Ok(()) => Ok(()),
            Err(rhai_err) => {
                if self.powershell_compat && looks_like_powershell(source) {
                    self.powershell.run(source, context)
                } else {
                    Err(rhai_err)
                }
            }
        }
    }
}

/// Heuristic detection of PowerShell syntax in legacy Scoop manifest
/// scripts (specification section 5.5).
fn looks_like_powershell(source: &str) -> bool {
    const MARKERS: &[&str] = &[
        "$env:",
        "Write-Host",
        "Write-Output",
        "Get-Item",
        "Set-Item",
        "New-Item",
        "Remove-Item",
        "Copy-Item",
        "-ErrorAction",
        "Start-Process",
        "Test-Path",
    ];
    MARKERS.iter().any(|m| source.contains(m))
}
