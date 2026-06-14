//! `last migrate --from-scoop` - register an existing Scoop installation in
//! LAST without re-downloading anything (specification section 15).
//!
//! Each migrated app is registered by creating a junction from
//! `%LAST_ROOT%\apps\<app>\<version>` to the existing Scoop installation
//! directory, so LAST can manage it (list, export, remove the registration)
//! without copying any files. Persisted data is linked the same way.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::install::{InstallMeta, InstallState};
use crate::manifest::{Architecture, Manifest};
use crate::path::create_junction;
use crate::ui::Ui;

/// Minimal shape of Scoop's `current\install.json`.
#[derive(Debug, Deserialize, Default)]
struct ScoopInstallInfo {
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    architecture: Option<String>,
}

pub struct Migrator {
    last_root: PathBuf,
    scoop_root: PathBuf,
    ui: Arc<Ui>,
}

impl Migrator {
    pub fn new(last_root: PathBuf, scoop_root: PathBuf, ui: Arc<Ui>) -> Self {
        Self { last_root, scoop_root, ui }
    }

    /// Migrates every installed Scoop app. Returns the names of apps that
    /// were registered.
    pub fn migrate(&self) -> Result<Vec<String>> {
        let scoop_apps = self.scoop_root.join("apps");
        if !scoop_apps.is_dir() {
            anyhow::bail!("Scoop apps directory not found: {}", scoop_apps.display());
        }

        let mut migrated = Vec::new();
        for entry in std::fs::read_dir(&scoop_apps)
            .with_context(|| format!("failed to read {}", scoop_apps.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let app = entry.file_name().to_string_lossy().to_string();
            if app.eq_ignore_ascii_case("scoop") {
                continue; // Scoop's own bootstrap app
            }

            match self.migrate_app(&app) {
                Ok(true) => migrated.push(app),
                Ok(false) => self.ui.warn(format!("skipping '{app}': no 'current' version found")),
                Err(e) => self.ui.error(format!("failed to migrate '{app}': {e}")),
            }
        }
        Ok(migrated)
    }

    /// Migrates a single app. Returns `false` if the app has no `current`
    /// link (e.g. a broken/partial install).
    fn migrate_app(&self, app: &str) -> Result<bool> {
        let scoop_current = self.scoop_root.join("apps").join(app).join("current");
        if !scoop_current.exists() {
            return Ok(false);
        }
        let real_dir = std::fs::canonicalize(&scoop_current)
            .with_context(|| format!("failed to resolve {}", scoop_current.display()))?;
        let version = real_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let manifest_path = scoop_current.join("manifest.json");
        let manifest = if manifest_path.is_file() {
            let content = std::fs::read_to_string(&manifest_path)?;
            let mut m = Manifest::load_from_str(&content, std::path::Path::new("manifest.json"))?;
            m.name = app.to_string();
            Some(m)
        } else {
            None
        };

        let install_info: ScoopInstallInfo = std::fs::read_to_string(scoop_current.join("install.json"))
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();

        let architecture = install_info
            .architecture
            .as_deref()
            .and_then(|a| Architecture::parse(a).ok())
            .unwrap_or_else(Architecture::detect);

        let last_app_dir = self.last_root.join("apps").join(app);
        let last_version_dir = last_app_dir.join(&version);
        let last_current = last_app_dir.join("current");

        self.ui.step(format!("registering '{app}' {version} -> {}", real_dir.display()));
        if !self.ui.is_dry_run() {
            std::fs::create_dir_all(&last_app_dir)?;
            create_junction(&real_dir, &last_version_dir)?;
            create_junction(&last_version_dir, &last_current)?;
        }

        let persist_entries = manifest.as_ref().map(|m| m.persist_entries()).unwrap_or_default();
        if !persist_entries.is_empty() {
            let scoop_persist = self.scoop_root.join("persist").join(app);
            let last_persist = self.last_root.join("persist").join(app);
            if scoop_persist.exists() && !last_persist.exists() {
                self.ui.step(format!("linking persisted data for '{app}'"));
                if !self.ui.is_dry_run() {
                    std::fs::create_dir_all(self.last_root.join("persist"))?;
                    create_junction(&scoop_persist, &last_persist)?;
                }
            }
        }

        let state = InstallState {
            bucket: install_info.bucket.unwrap_or_else(|| "migrated".to_string()),
            version: version.clone(),
            architecture: architecture.key().to_string(),
            shims: Vec::new(),
            persist: persist_entries,
            env_set: Default::default(),
            env_add_path: Vec::new(),
        };

        if !self.ui.is_dry_run() {
            let meta = InstallMeta {
                install: state,
                manifest: manifest.unwrap_or_default(),
            };
            let meta_path = last_app_dir.join(format!("{version}.meta.json"));
            std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)
                .with_context(|| format!("failed to write {}", meta_path.display()))?;
        }

        Ok(true)
    }
}
