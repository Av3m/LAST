//! Persist link management (specification section 8).
//!
//! On first install, paths listed in a manifest's `persist` field are moved
//! to `%LAST_ROOT%\persist\<app>\` and replaced in the install directory by
//! a junction (directories) or hard link (files). On update, the existing
//! persist data is preserved and re-linked into the new version's directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::path::{create_hardlink, create_junction, remove_link};
use crate::ui::Ui;

/// Manages the `%LAST_ROOT%\persist\` tree.
pub struct PersistManager {
    root: PathBuf,
}

impl PersistManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `%LAST_ROOT%\persist\<app>`
    pub fn persist_dir(&self, app: &str) -> PathBuf {
        self.root.join("persist").join(app)
    }

    /// Links every entry in `entries` between `install_dir` (the freshly
    /// extracted version directory) and this app's persist directory.
    ///
    /// - If the persist directory does not yet contain the entry, any data
    ///   extracted into `install_dir` is moved there (first install).
    /// - If the persist directory already contains the entry (update), the
    ///   freshly extracted copy in `install_dir` is discarded in favor of
    ///   the persisted data.
    /// - In both cases, `install_dir/<entry>` ends up as a junction (for
    ///   directories) or hard link (for files) pointing at the persisted
    ///   data.
    pub fn link_all(&self, app: &str, install_dir: &Path, entries: &[String], ui: &Ui) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let persist_dir = self.persist_dir(app);
        std::fs::create_dir_all(&persist_dir)?;

        for entry in entries {
            self.link_one(&persist_dir, install_dir, entry, ui)?;
        }
        Ok(())
    }

    fn link_one(&self, persist_dir: &Path, install_dir: &Path, entry: &str, ui: &Ui) -> Result<()> {
        let entry_path = Path::new(entry);
        let persisted = persist_dir.join(entry_path);
        let installed = install_dir.join(entry_path);

        if ui.is_dry_run() {
            ui.step(format!("would persist '{entry}'"));
            return Ok(());
        }

        if persisted.exists() {
            // Persisted data already exists: drop the freshly extracted copy.
            if installed.exists() {
                remove_path(&installed)?;
            }
        } else if installed.exists() {
            // First install: move the extracted data into the persist store.
            if let Some(parent) = persisted.parent() {
                std::fs::create_dir_all(parent)?;
            }
            move_path(&installed, &persisted)
                .with_context(|| format!("failed to move '{entry}' to persist store"))?;
        } else {
            // Neither side has the entry yet: create an empty placeholder.
            if let Some(parent) = persisted.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if looks_like_file(entry) {
                std::fs::write(&persisted, b"")?;
            } else {
                std::fs::create_dir_all(&persisted)?;
            }
        }

        if let Some(parent) = installed.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if persisted.is_dir() {
            create_junction(&persisted, &installed)
        } else {
            create_hardlink(&persisted, &installed)
        }
    }

    /// Removes the link at `install_dir/<entry>` without touching the
    /// persisted data, for each entry. Used before removing an app version
    /// directory so the persisted data on disk is not deleted along with it.
    pub fn unlink_all(&self, install_dir: &Path, entries: &[String]) -> Result<()> {
        for entry in entries {
            let installed = install_dir.join(entry);
            if installed.exists() {
                remove_link(&installed)?;
            }
        }
        Ok(())
    }

    /// Permanently deletes an app's persisted data (`last remove --purge`).
    pub fn purge(&self, app: &str) -> Result<()> {
        let dir = self.persist_dir(app);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to remove persist directory {}", dir.display()))?;
        }
        Ok(())
    }
}

fn looks_like_file(entry: &str) -> bool {
    Path::new(entry)
        .extension()
        .is_some_and(|ext| !ext.is_empty())
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("failed to remove {}", path.display()))
}

fn move_path(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // rename() can fail across volumes; fall back to copy + remove.
        Err(_) => {
            copy_recursive(from, to)?;
            remove_path(from)
        }
    }
}

fn copy_recursive(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
    .with_context(|| format!("failed to copy {} to {}", from.display(), to.display()))
}
