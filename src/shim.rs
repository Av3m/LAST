//! Shim generation (specification section 9).
//!
//! A shim is a small `.cmd` stub placed in `%LAST_ROOT%\shims\` that
//! forwards execution (and arguments) to the real binary inside an app's
//! `current` directory. `.cmd` is used because Windows' default `PATHEXT`
//! already includes it, so shims work as drop-in commands without requiring
//! a separate native shim executable.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::manifest::BinEntry;
use crate::ui::Ui;

/// Manages `%LAST_ROOT%\shims\`.
pub struct ShimManager {
    shims_dir: PathBuf,
}

impl ShimManager {
    pub fn new(root: &Path) -> Self {
        Self {
            shims_dir: root.join("shims"),
        }
    }

    pub fn shims_dir(&self) -> &Path {
        &self.shims_dir
    }

    /// Creates shims for every `bin` entry, pointing at `current_dir`
    /// (the app's `current` junction, so updates don't require regenerating
    /// shims). Returns the shim names that were created, for bookkeeping in
    /// the app's install state.
    pub fn create_all(&self, bins: &[BinEntry], current_dir: &Path, ui: &Ui) -> Result<Vec<String>> {
        let mut created = Vec::new();
        for bin in bins {
            created.push(self.create_one(bin, current_dir, ui)?);
        }
        Ok(created)
    }

    fn create_one(&self, bin: &BinEntry, current_dir: &Path, ui: &Ui) -> Result<String> {
        let exe_rel = bin.exe().replace('/', "\\");
        let target = current_dir.join(&exe_rel);
        let alias = bin
            .alias()
            .map(str::to_string)
            .unwrap_or_else(|| file_stem(&exe_rel));

        let shim_path = self.shims_dir.join(format!("{alias}.cmd"));
        ui.step(format!("creating shim '{alias}' -> {}", target.display()));

        if ui.is_dry_run() {
            return Ok(alias);
        }

        std::fs::create_dir_all(&self.shims_dir)?;

        let extra_args = bin
            .extra_args()
            .iter()
            .map(|a| quote_arg(a))
            .collect::<Vec<_>>()
            .join(" ");

        let target_display = target.display();
        let content = if exe_rel.to_ascii_lowercase().ends_with(".ps1") {
            format!(
                "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"{target_display}\" {extra_args} %*\r\n"
            )
        } else {
            format!("@echo off\r\n\"{target_display}\" {extra_args} %*\r\n")
        };

        std::fs::write(&shim_path, content)
            .with_context(|| format!("failed to write shim {}", shim_path.display()))?;
        Ok(alias)
    }

    /// Removes the shims with the given names (without extension).
    pub fn remove_all(&self, names: &[String], ui: &Ui) -> Result<()> {
        for name in names {
            let path = self.shims_dir.join(format!("{name}.cmd"));
            if path.exists() {
                ui.step(format!("removing shim '{name}'"));
                if !ui.is_dry_run() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("failed to remove shim {}", path.display()))?;
                }
            }
        }
        Ok(())
    }
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn quote_arg(arg: &str) -> String {
    if arg.contains(' ') {
        format!("\"{arg}\"")
    } else {
        arg.to_string()
    }
}
