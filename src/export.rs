//! `last export` - copy an installed package to a portable destination
//! (specification section 10).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::Local;

use crate::error::LastError;
use crate::install::InstallMeta;
use crate::ui::Ui;

/// Options for [`Exporter::export`].
#[derive(Default)]
pub struct ExportOptions {
    pub include_persist: bool,
    pub overwrite: bool,
    pub backup_suffix: Option<String>,
}

pub struct Exporter {
    root: PathBuf,
    ui: Arc<Ui>,
}

impl Exporter {
    pub fn new(root: PathBuf, ui: Arc<Ui>) -> Self {
        Self { root, ui }
    }

    /// Copies the `current` version of `app` (and optionally its persisted
    /// data) into `dest`.
    pub fn export(&self, app: &str, dest: &Path, options: &ExportOptions) -> Result<()> {
        let current = self.root.join("apps").join(app).join("current");
        if !current.exists() {
            bail!(LastError::AppNotInstalled(app.to_string()));
        }

        let persist_entries = self.load_persist_entries(app)?;

        self.ui.step(format!("exporting '{app}' to {}", dest.display()));
        if !self.ui.is_dry_run() {
            std::fs::create_dir_all(dest)?;
        }

        self.copy_tree(&current, dest, &persist_entries, options)?;

        if options.include_persist {
            let persist_dir = self.root.join("persist").join(app);
            for entry in &persist_entries {
                let src = persist_dir.join(entry);
                if !src.exists() {
                    continue;
                }
                let dst = dest.join(entry);
                self.copy_with_backup(&src, &dst, options)?;
            }
        }

        self.ui.success(format!("exported '{app}' to {}", dest.display()));
        Ok(())
    }

    /// Reads the persist entries for `app`'s currently active version from
    /// its `.meta.json` sidecar file.
    fn load_persist_entries(&self, app: &str) -> Result<Vec<String>> {
        let current = self.root.join("apps").join(app).join("current");
        let Ok(real) = std::fs::canonicalize(&current) else {
            return Ok(Vec::new());
        };
        let Some(version) = real.file_name().map(|n| n.to_string_lossy().to_string()) else {
            return Ok(Vec::new());
        };
        let meta_path = self.root.join("apps").join(app).join(format!("{version}.meta.json"));
        if !meta_path.is_file() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&meta_path)?;
        let meta: InstallMeta = serde_json::from_str(&content)?;
        Ok(meta.manifest.persist_entries())
    }

    /// Copies the contents of `src` into `dst`, skipping (unless
    /// `include_persist`) the top-level entries listed in
    /// `persist_entries`.
    fn copy_tree(&self, src: &Path, dst: &Path, persist_entries: &[String], options: &ExportOptions) -> Result<()> {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            if !options.include_persist && is_persist_entry(&name, persist_entries) {
                continue;
            }

            let dst_path = dst.join(&name);
            self.copy_with_backup(&entry.path(), &dst_path, options)?;
        }
        Ok(())
    }

    /// Copies `src` to `dst`, backing up or removing any existing file/dir
    /// at `dst` first.
    fn copy_with_backup(&self, src: &Path, dst: &Path, options: &ExportOptions) -> Result<()> {
        if dst.exists() {
            if options.overwrite {
                self.ui.step(format!("overwriting {}", dst.display()));
                if !self.ui.is_dry_run() {
                    remove_path(dst)?;
                }
            } else {
                let backup = backup_path(dst, options.backup_suffix.as_deref());
                self.ui.step(format!("backing up existing {} -> {}", dst.display(), backup.display()));
                if !self.ui.is_dry_run() {
                    std::fs::rename(dst, &backup)
                        .with_context(|| format!("failed to back up {}", dst.display()))?;
                }
            }
        }

        self.ui.step(format!("copy {} -> {}", src.display(), dst.display()));
        if self.ui.is_dry_run() {
            return Ok(());
        }
        copy_recursive(src, dst)
    }
}

fn is_persist_entry(name: &str, persist_entries: &[String]) -> bool {
    persist_entries
        .iter()
        .any(|entry| Path::new(entry).iter().next().map(|c| c == name.as_ref() as &std::ffi::OsStr).unwrap_or(false))
}

fn backup_path(path: &Path, suffix: Option<&str>) -> PathBuf {
    let suffix = suffix
        .map(str::to_string)
        .unwrap_or_else(|| format!(".bak.{}", Local::now().format("%Y%m%d%H%M%S")));
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
    .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("failed to remove {}", path.display()))
}
