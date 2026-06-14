//! PowerShell compatibility layer (specification section 5.5).
//!
//! Executes legacy Scoop PowerShell scripts via `powershell.exe`. The script
//! variables from [`super::ScriptContext`] are injected as PowerShell
//! variables (`$dir`, `$persist_dir`, `$version`, `$app`, `$architecture`,
//! `$scoopdir`, `$last_root`, `$global`) before the script body runs.

use std::process::Command;
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use super::{ScriptContext, ScriptEngine};
use crate::ui::Ui;

pub struct PowerShellEngine {
    ui: Arc<Ui>,
}

impl PowerShellEngine {
    pub fn new(ui: Arc<Ui>) -> Self {
        Self { ui }
    }
}

impl ScriptEngine for PowerShellEngine {
    fn run(&self, source: &str, context: &ScriptContext) -> Result<()> {
        self.ui.warn("running script via PowerShell compatibility mode");

        if self.ui.is_dry_run() {
            self.ui.step("would run PowerShell script");
            return Ok(());
        }

        let prelude = format!(
            "$dir = '{dir}'; $persist_dir = '{persist_dir}'; $version = '{version}'; \
             $app = '{app}'; $architecture = '{architecture}'; $scoopdir = '{scoopdir}'; \
             $last_root = '{last_root}'; $global = ${global}; ",
            dir = escape(&context.dir.display().to_string()),
            persist_dir = escape(&context.persist_dir.display().to_string()),
            version = escape(&context.version),
            app = escape(&context.app),
            architecture = escape(&context.architecture),
            scoopdir = escape(&context.last_root.display().to_string()),
            last_root = escape(&context.last_root.display().to_string()),
            global = context.global,
        );

        let script = format!("{prelude}\n{source}");

        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .output()
            .context("failed to launch powershell.exe (PowerShell compatibility mode requires PowerShell to be installed)")?;

        if !output.stdout.is_empty() {
            self.ui.info(String::from_utf8_lossy(&output.stdout).trim_end());
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("PowerShell script failed: {}", stderr.trim());
        }
        Ok(())
    }
}

/// Escapes a value for embedding in a single-quoted PowerShell string.
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}
