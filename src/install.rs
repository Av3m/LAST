//! Install / update / remove / list orchestration.
//!
//! [`InstallManager`] ties together the bucket registry, downloader,
//! extractor, persist manager, shim manager and script runner to implement
//! `last install`, `last update`, `last remove` and `last list`.
//!
//! Per-installation metadata is stored alongside each installed version as
//! `install.json` ([`InstallState`]) and a copy of the manifest
//! (`manifest.json`), so that updates and removal do not depend on the
//! originating bucket still being registered or unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::bucket::BucketManager;
use crate::config::Config;
use crate::download::Downloader;
use crate::error::LastError;
use crate::extract::ExtractorRegistry;
use crate::log::{LogEntry, LogResult, OperationLogger, Operation};
use crate::manifest::{Architecture, Manifest, ResolvedManifest};
use crate::path::{add_user_path_entry, create_junction, remove_user_path_entry, set_user_env, unset_user_env};
use crate::persist::PersistManager;
use crate::script::{ScriptContext, ScriptRunner};
use crate::shim::ShimManager;
use crate::ui::Ui;

/// Per-installed-version metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallState {
    pub bucket: String,
    pub version: String,
    pub architecture: String,
    pub shims: Vec<String>,
    pub persist: Vec<String>,
    pub env_set: HashMap<String, String>,
    pub env_add_path: Vec<String>,
}

/// Metadata for one installed version, stored as
/// `apps\<app>\<version>.meta.json` - a sibling of the version directory
/// rather than a file inside it. This keeps the version directory itself a
/// pristine copy of the extracted package (useful for `last export`) and
/// means a version directory that is a junction to an external location
/// (see `migrate.rs`) is never written to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallMeta {
    pub install: InstallState,
    pub manifest: Manifest,
}

/// Summary of an installed app, as returned by [`InstallManager::list`].
pub struct InstalledApp {
    pub name: String,
    pub version: String,
    pub bucket: String,
    pub architecture: String,
}

/// Splits a package specification (`<package>` or `<package>@<version>`)
/// into the package name/query and an optional requested version.
pub fn parse_package_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('@') {
        Some((name, version)) if !version.is_empty() => (name, Some(version)),
        _ => (spec, None),
    }
}

/// Orchestrates install/update/remove/list operations.
pub struct InstallManager<'a> {
    root: PathBuf,
    config: &'a Config,
    buckets: &'a BucketManager,
    downloader: Downloader,
    extractor: ExtractorRegistry,
    persist: PersistManager,
    shims: ShimManager,
    scripts: ScriptRunner,
    logger: OperationLogger,
    ui: Arc<Ui>,
}

impl<'a> InstallManager<'a> {
    pub fn new(root: PathBuf, config: &'a Config, buckets: &'a BucketManager, ui: Arc<Ui>) -> Result<Self> {
        let proxy = if config.download_proxy.is_empty() {
            std::env::var("LAST_PROXY").ok()
        } else {
            Some(config.download_proxy.clone())
        };
        let downloader = Downloader::new(proxy.as_deref())?;
        let persist = PersistManager::new(root.clone());
        let shims = ShimManager::new(&root);
        let scripts = ScriptRunner::new(ui.clone(), config.powershell_compat);
        let logger = OperationLogger::new(&root);
        Ok(Self {
            root,
            config,
            buckets,
            downloader,
            extractor: ExtractorRegistry::new(),
            persist,
            shims,
            scripts,
            logger,
            ui,
        })
    }

    fn apps_dir(&self) -> PathBuf {
        self.root.join("apps")
    }

    fn app_dir(&self, app: &str) -> PathBuf {
        self.apps_dir().join(app)
    }

    fn current_dir(&self, app: &str) -> PathBuf {
        self.app_dir(app).join("current")
    }

    /// Resolves the architecture to use, applying the `--arch` override,
    /// falling back to the configured default, and validating it against
    /// the manifest's available architectures.
    fn resolve_architecture(&self, manifest: &Manifest, arch_override: Option<Architecture>) -> Result<Architecture> {
        let arch = if let Some(arch) = arch_override {
            arch
        } else if let Some(first) = self.config.architectures.first() {
            Architecture::parse(first)?
        } else {
            Architecture::detect()
        };

        if manifest.resolved(arch).is_ok() {
            return Ok(arch);
        }
        let available = manifest.available_architectures();
        bail!(
            "manifest '{}' does not support architecture '{}' (available: {})",
            manifest.name,
            arch.key(),
            available.join(", ")
        );
    }

    pub fn is_installed(&self, app: &str) -> bool {
        self.current_dir(app).exists()
    }

    /// Path to an installed version's metadata file.
    fn meta_path(&self, app: &str, version: &str) -> PathBuf {
        self.app_dir(app).join(format!("{version}.meta.json"))
    }

    fn load_meta(&self, app: &str, version: &str) -> Result<(InstallState, Manifest)> {
        let path = self.meta_path(app, version);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let meta: InstallMeta =
            serde_json::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))?;
        let mut manifest = meta.manifest;
        manifest.name = app.to_string();
        Ok((meta.install, manifest))
    }

    fn save_meta(&self, app: &str, version: &str, state: &InstallState, manifest: &Manifest) -> Result<()> {
        let path = self.meta_path(app, version);
        let meta = InstallMeta {
            install: state.clone(),
            manifest: manifest.clone(),
        };
        std::fs::write(&path, serde_json::to_string_pretty(&meta)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    /// Resolves the version currently active for `app` by following the
    /// `current` junction.
    fn current_version(&self, app: &str) -> Result<String> {
        let current = self.current_dir(app);
        if !current.exists() {
            bail!(LastError::AppNotInstalled(app.to_string()));
        }
        let real = std::fs::canonicalize(&current)
            .with_context(|| format!("failed to resolve {}", current.display()))?;
        Ok(real
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default())
    }

    fn load_current_state(&self, app: &str) -> Result<(InstallState, Manifest)> {
        let version = self.current_version(app)?;
        self.load_meta(app, &version)
    }

    fn script_context(&self, app: &str, version: &str, dir: &Path, persist_dir: &Path, architecture: Architecture) -> ScriptContext {
        ScriptContext {
            dir: dir.to_path_buf(),
            persist_dir: persist_dir.to_path_buf(),
            version: version.to_string(),
            app: app.to_string(),
            architecture: architecture.key().to_string(),
            last_root: self.root.clone(),
            global: false,
        }
    }

    // ------------------------------------------------------------------
    // install
    // ------------------------------------------------------------------

    /// Installs `query` (`<package>` or `<bucket>/<package>`). `version`, if
    /// given, must match the version provided by the resolved manifest -
    /// installing an older/specific version that isn't the bucket's current
    /// manifest version is not supported in this release.
    pub fn install(&self, query: &str, version: Option<&str>, arch_override: Option<Architecture>) -> Result<()> {
        let (bucket_name, manifest) = self.buckets.find_manifest(query)?;

        if let Some(requested) = version {
            if manifest.version != requested {
                bail!(
                    "version '{requested}' of '{}' is not available (bucket '{bucket_name}' provides {})",
                    manifest.name,
                    manifest.version
                );
            }
        }

        let app = manifest.name.clone();
        if self.is_installed(&app) {
            let (state, _) = self.load_current_state(&app)?;
            bail!(LastError::AppAlreadyInstalled(app, state.version));
        }

        for ignored in manifest.ignored_fields() {
            self.ui.debug(format!("ignoring unsupported field '{ignored}' in manifest '{app}'"));
        }

        for dep in manifest.depends() {
            let (_, dep_name) = dep.split_once('/').unwrap_or(("", dep.as_str()));
            if !self.is_installed(dep_name) {
                self.ui.warn(format!(
                    "'{app}' depends on '{dep}', which is not installed - install it separately"
                ));
            }
        }

        let arch = self.resolve_architecture(&manifest, arch_override)?;
        self.ui.info(format!("installing '{app}' {} ({}) from bucket '{bucket_name}'", manifest.version, arch.key()));

        let version_dir = self.install_version(&bucket_name, &manifest, arch)?;
        self.activate(&app, &manifest.version, &version_dir)?;

        self.logger.log(
            &LogEntry::new(Operation::Install, &app, &manifest.version).with_result(LogResult::Success),
        )?;
        self.ui.success(format!("installed '{app}' {}", manifest.version));
        Ok(())
    }

    /// Downloads, extracts and prepares a specific version of `manifest` in
    /// `apps\<app>\<version>\`, running pre/post install scripts, persisting
    /// data and writing `install.json` / `manifest.json`. Does not touch the
    /// `current` link.
    fn install_version(&self, bucket_name: &str, manifest: &Manifest, arch: Architecture) -> Result<PathBuf> {
        let app = &manifest.name;
        let resolved = manifest.resolved(arch)?;
        let bucket_manifest_dir = self
            .buckets
            .get(bucket_name)
            .map(|b| b.manifest_dir())
            .unwrap_or_default();

        let version_dir = self.app_dir(app).join(&manifest.version);
        self.ui.step(format!("creating {}", version_dir.display()));
        if !self.ui.is_dry_run() {
            std::fs::create_dir_all(&version_dir)?;
        }

        self.download_and_extract(&resolved, &version_dir)?;

        let persist_dir = self.persist.persist_dir(app);
        let ctx = self.script_context(app, &manifest.version, &version_dir, &persist_dir, arch);

        if let Some(script) = &resolved.pre_install {
            self.ui.step("running pre_install script");
            let source = script.resolve(&bucket_manifest_dir)?;
            self.scripts.run(&source, &ctx)?;
        }

        let persist_entries = manifest.persist_entries();
        self.persist.link_all(app, &version_dir, &persist_entries, &self.ui)?;

        if let Some(script) = &resolved.post_install {
            self.ui.step("running post_install script");
            let source = script.resolve(&bucket_manifest_dir)?;
            self.scripts.run(&source, &ctx)?;
        }

        let state = InstallState {
            bucket: bucket_name.to_string(),
            version: manifest.version.clone(),
            architecture: arch.key().to_string(),
            shims: Vec::new(), // filled in by `activate`
            persist: persist_entries,
            env_set: resolved.env_set.clone(),
            env_add_path: resolved.env_add_path.clone(),
        };

        if !self.ui.is_dry_run() {
            self.save_meta(app, &manifest.version, &state, manifest)?;
        }

        Ok(version_dir)
    }

    /// Activates `version` as the `current` version: creates the `current`
    /// junction, (re)creates shims pointing at it, and applies `env_set` /
    /// `env_add_path`.
    fn activate(&self, app: &str, version: &str, version_dir: &Path) -> Result<()> {
        let current_dir = self.current_dir(app);
        self.ui.step(format!("activating {} as current", version_dir.display()));
        if !self.ui.is_dry_run() {
            create_junction(version_dir, &current_dir)?;
        }

        let (mut state, manifest) = self.load_meta(app, version)?;

        let arch = Architecture::parse(&state.architecture).unwrap_or_else(|_| Architecture::detect());
        let resolved = manifest.resolved(arch)?;
        let shim_names = self.shims.create_all(&resolved.bin, &current_dir, &self.ui)?;
        state.shims = shim_names;

        for (key, value) in &state.env_set {
            self.ui.step(format!("set_env {key}={value}"));
            if !self.ui.is_dry_run() {
                set_user_env(key, value)?;
            }
        }
        for entry in &state.env_add_path {
            let path = current_dir.join(entry).display().to_string();
            self.ui.step(format!("env_add_path {path}"));
            if !self.ui.is_dry_run() {
                add_user_path_entry(&path)?;
            }
        }

        if !self.ui.is_dry_run() {
            self.save_meta(app, version, &state, &manifest)?;
        }
        Ok(())
    }

    /// Downloads every resolved URL and extracts/copies it into
    /// `version_dir`.
    fn download_and_extract(&self, resolved: &ResolvedManifest, version_dir: &Path) -> Result<()> {
        let cache_dir = self.root.join("cache");
        for (i, download) in resolved.downloads.iter().enumerate() {
            let archive = self.downloader.fetch(download, &cache_dir, &self.ui)?;
            if self.ui.is_dry_run() {
                continue;
            }
            let file_name = download.download.file_name();

            if self.extractor.is_supported(&file_name) {
                let temp_dir = version_dir.join(format!(".extract-{i}"));
                self.extractor.extract(&archive, &temp_dir)?;

                let source_dir = match &resolved.extract_dir {
                    Some(extract_dir) => temp_dir.join(extract_dir),
                    None => temp_dir.clone(),
                };
                let dest_dir = match &resolved.extract_to {
                    Some(extract_to) => {
                        let dest = version_dir.join(extract_to);
                        std::fs::create_dir_all(&dest)?;
                        dest
                    }
                    None => version_dir.to_path_buf(),
                };
                move_dir_contents(&source_dir, &dest_dir)?;
                if temp_dir.exists() {
                    std::fs::remove_dir_all(&temp_dir)
                        .with_context(|| format!("failed to clean up {}", temp_dir.display()))?;
                }
            } else if file_name.to_ascii_lowercase().ends_with(".exe") {
                // Portable executable: copy as-is (specification 4.3).
                let dest = version_dir.join(&file_name);
                std::fs::copy(&archive, &dest)
                    .with_context(|| format!("failed to copy portable executable to {}", dest.display()))?;
            } else {
                bail!(LastError::UnsupportedArchiveFormat(file_name));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // update
    // ------------------------------------------------------------------

    /// Updates a single app, or all installed apps if `app` is `None`
    /// (`last update *`).
    pub fn update(&self, app: Option<&str>, arch_override: Option<Architecture>) -> Result<()> {
        match app {
            Some(app) => self.update_one(app, arch_override),
            None => {
                let apps = self.list()?;
                if apps.is_empty() {
                    self.ui.info("no apps installed");
                    return Ok(());
                }
                for installed in apps {
                    if let Err(e) = self.update_one(&installed.name, arch_override) {
                        self.ui.error(format!("failed to update '{}': {e}", installed.name));
                    }
                }
                Ok(())
            }
        }
    }

    fn update_one(&self, app: &str, arch_override: Option<Architecture>) -> Result<()> {
        let (state, _) = self.load_current_state(app)?;
        let query = format!("{}/{app}", state.bucket);
        let (bucket_name, manifest) = self
            .buckets
            .find_manifest(&query)
            .or_else(|_| self.buckets.find_manifest(app))?;

        if manifest.version == state.version {
            self.ui.info(format!("'{app}' is already up to date ({})", state.version));
            return Ok(());
        }

        let arch = arch_override.unwrap_or_else(|| Architecture::parse(&state.architecture).unwrap_or_else(|_| Architecture::detect()));
        let arch = self.resolve_architecture(&manifest, Some(arch))?;

        self.ui.info(format!(
            "updating '{app}' {} -> {} ({})",
            state.version, manifest.version, arch.key()
        ));

        let version_dir = self.install_version(&bucket_name, &manifest, arch)?;
        self.activate(app, &manifest.version, &version_dir)?;

        self.logger.log(
            &LogEntry::new(Operation::Update, app, &manifest.version).with_result(LogResult::Success),
        )?;
        self.ui.success(format!("updated '{app}' to {}", manifest.version));
        Ok(())
    }

    // ------------------------------------------------------------------
    // remove
    // ------------------------------------------------------------------

    /// Removes an installed app. If `purge` is set, persisted user data is
    /// also deleted.
    pub fn remove(&self, app: &str, purge: bool) -> Result<()> {
        let app_dir = self.app_dir(app);
        if !app_dir.exists() {
            bail!(LastError::AppNotInstalled(app.to_string()));
        }
        let (state, manifest) = self.load_current_state(app)?;
        let current_dir = self.current_dir(app);

        let arch = Architecture::parse(&state.architecture).unwrap_or_else(|_| Architecture::detect());
        let resolved = manifest.resolved(arch)?;
        let bucket_manifest_dir = self
            .buckets
            .get(&state.bucket)
            .map(|b| b.manifest_dir())
            .unwrap_or_default();
        let persist_dir = self.persist.persist_dir(app);
        let ctx = self.script_context(app, &state.version, &current_dir, &persist_dir, arch);

        if let Some(script) = &resolved.pre_uninstall {
            self.ui.step("running pre_uninstall script");
            let source = script.resolve(&bucket_manifest_dir)?;
            self.scripts.run(&source, &ctx)?;
        }

        self.shims.remove_all(&state.shims, &self.ui)?;

        for (key, _) in &state.env_set {
            self.ui.step(format!("unset_env {key}"));
            if !self.ui.is_dry_run() {
                unset_user_env(key)?;
            }
        }
        for entry in &state.env_add_path {
            let path = current_dir.join(entry).display().to_string();
            self.ui.step(format!("removing '{path}' from PATH"));
            if !self.ui.is_dry_run() {
                remove_user_path_entry(&path)?;
            }
        }

        if let Some(script) = &resolved.post_uninstall {
            self.ui.step("running post_uninstall script");
            let source = script.resolve(&bucket_manifest_dir)?;
            self.scripts.run(&source, &ctx)?;
        }

        // Unlink persisted data in every installed version before removing
        // the app directory, so `remove_dir_all` doesn't follow junctions
        // into the persist store.
        if app_dir.is_dir() {
            for entry in std::fs::read_dir(&app_dir)? {
                let entry = entry?;
                // Skip the `current` junction: it points at one of the
                // version directories below, which is processed directly.
                // Unlinking through `current` traverses two reparse points
                // (`current` -> version dir -> persist dir) and fails with
                // access denied on Windows.
                if entry.file_name() == "current" {
                    continue;
                }
                if entry.file_type()?.is_dir() {
                    self.persist.unlink_all(&entry.path(), &state.persist)?;
                }
            }
        }

        self.ui.step(format!("removing {}", app_dir.display()));
        if !self.ui.is_dry_run() {
            std::fs::remove_dir_all(&app_dir)
                .with_context(|| format!("failed to remove {}", app_dir.display()))?;
        }

        if purge {
            self.ui.step(format!("purging persisted data for '{app}'"));
            if !self.ui.is_dry_run() {
                self.persist.purge(app)?;
            }
        }

        self.logger.log(
            &LogEntry::new(Operation::Remove, app, &state.version).with_result(LogResult::Success),
        )?;
        self.ui.success(format!("removed '{app}'"));
        Ok(())
    }

    // ------------------------------------------------------------------
    // list / info
    // ------------------------------------------------------------------

    /// Lists all installed apps.
    pub fn list(&self) -> Result<Vec<InstalledApp>> {
        let apps_dir = self.apps_dir();
        if !apps_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for entry in std::fs::read_dir(&apps_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Ok((state, _)) = self.load_current_state(&name) {
                result.push(InstalledApp {
                    name,
                    version: state.version,
                    bucket: state.bucket,
                    architecture: state.architecture,
                });
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    /// Returns manifest and install-location details for `query`, used by
    /// `last info`. Looks at the bucket manifest first, falling back to the
    /// installed copy if the app is not (or no longer) provided by any
    /// registered bucket.
    pub fn info(&self, query: &str) -> Result<(Manifest, Option<InstallState>)> {
        match self.buckets.find_manifest(query) {
            Ok((_, manifest)) => {
                let app = manifest.name.clone();
                let state = self.load_current_state(&app).ok().map(|(s, _)| s);
                Ok((manifest, state))
            }
            Err(e) => {
                if let Ok((state, manifest)) = self.load_current_state(query) {
                    Ok((manifest, Some(state)))
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn current_dir_for(&self, app: &str) -> PathBuf {
        self.current_dir(app)
    }

    pub fn persist_dir_for(&self, app: &str) -> PathBuf {
        self.persist.persist_dir(app)
    }
}

/// Moves all entries from `src` into `dst`, creating `dst` if needed.
/// Existing entries at the destination are replaced.
fn move_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        bail!("expected extracted contents at {}, but it does not exist (check 'extract_dir')", src.display());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if target.exists() {
            if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            } else {
                std::fs::remove_file(&target)?;
            }
        }
        std::fs::rename(entry.path(), &target)
            .with_context(|| format!("failed to move {} to {}", entry.path().display(), target.display()))?;
    }
    Ok(())
}
