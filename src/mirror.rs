//! `last mirror` - import a package from a public Scoop bucket into the
//! local bucket and binary share (specification section 7).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use crate::config::MirrorConfig;
use crate::download::Downloader;
use crate::manifest::{Architecture, Manifest, OneOrMany};
use crate::ui::Ui;

/// Public Scoop bucket to mirror from.
#[derive(Debug, Clone, Copy)]
pub enum MirrorSource {
    Main,
    Extras,
}

impl MirrorSource {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "main" => Ok(Self::Main),
            "extras" => Ok(Self::Extras),
            other => bail!("unknown mirror source '{other}', expected 'main' or 'extras'"),
        }
    }

    fn manifest_url(&self, app: &str) -> String {
        let bucket = match self {
            MirrorSource::Main => "Main",
            MirrorSource::Extras => "Extras",
        };
        format!("https://raw.githubusercontent.com/ScoopInstaller/{bucket}/master/bucket/{app}.json")
    }
}

/// Options for [`Mirror::run`] (specification section 7.2).
pub struct MirrorOptions {
    pub source: MirrorSource,
    /// Architectures to mirror; falls back to `mirror.architectures` from
    /// config if empty.
    pub architectures: Vec<Architecture>,
    pub vendor: Option<String>,
    pub app_name: Option<String>,
    pub list_only: bool,
    pub skip_download: bool,
}

pub struct Mirror<'a> {
    config: &'a MirrorConfig,
    ui: Arc<Ui>,
    downloader: Downloader,
}

impl<'a> Mirror<'a> {
    pub fn new(config: &'a MirrorConfig, ui: Arc<Ui>) -> Result<Self> {
        let proxy = if config.download_proxy.is_empty() {
            std::env::var("LAST_PROXY").ok()
        } else {
            Some(config.download_proxy.clone())
        };
        let downloader = Downloader::new(proxy.as_deref())?;
        Ok(Self { config, ui, downloader })
    }

    /// Mirrors `app` according to `options`.
    pub fn run(&self, app: &str, options: &MirrorOptions) -> Result<()> {
        let url = options.source.manifest_url(app);
        self.ui.step(format!("fetching manifest from {url}"));
        let text = reqwest::blocking::get(&url)
            .with_context(|| format!("failed to fetch {url}"))?
            .error_for_status()
            .with_context(|| format!("manifest '{app}' not found in source bucket ({url})"))?
            .text()?;
        let mut manifest = Manifest::load_from_str(&text, Path::new(&format!("{app}.json")))?;

        let vendor = options
            .vendor
            .clone()
            .unwrap_or_else(|| detect_vendor(manifest.homepage.as_deref()));
        let app_name = options.app_name.clone().unwrap_or_else(|| app.to_string());

        let requested: Vec<Architecture> = if options.architectures.is_empty() {
            self.config
                .architectures
                .iter()
                .filter_map(|a| Architecture::parse(a).ok())
                .collect()
        } else {
            options.architectures.clone()
        };

        let archs: Vec<Architecture> = requested
            .into_iter()
            .filter(|a| manifest.resolved(*a).is_ok())
            .collect();
        if archs.is_empty() {
            bail!(
                "manifest '{app}' does not provide any of the requested architectures (available: {})",
                manifest.available_architectures().join(", ")
            );
        }

        self.ui.info(format!(
            "mirroring '{app}' {} (vendor: {vendor}, app name: {app_name}, architectures: {})",
            manifest.version,
            archs.iter().map(|a| a.key()).collect::<Vec<_>>().join(", ")
        ));

        if options.list_only {
            return self.print_plan(app, &manifest, &vendor, &app_name, &archs);
        }

        if self.config.binary_share.is_empty() || self.config.bucket_path.is_empty() {
            bail!(
                "mirror.binary_share and mirror.bucket_path must be configured first \
                 (see 'last config set mirror.binary_share <path>' and 'last config set mirror.bucket_path <path>')"
            );
        }

        let mut new_arch_blocks = HashMap::new();
        for arch in &archs {
            let resolved = manifest.resolved(*arch)?;
            let mut urls = Vec::new();
            let mut hashes = Vec::new();

            for download in &resolved.downloads {
                let rel = share_relative_path(&vendor, &app_name, &manifest.version, *arch, &download.download.file_name());
                let target = Path::new(&self.config.binary_share).join(&rel);

                if !options.skip_download {
                    let cache_dir = std::env::temp_dir().join("last-mirror");
                    let file = self.downloader.fetch(download, &cache_dir, &self.ui)?;
                    if let Some(parent) = target.parent() {
                        if !self.ui.is_dry_run() {
                            std::fs::create_dir_all(parent)
                                .with_context(|| format!("failed to create {}", parent.display()))?;
                        }
                    }
                    self.ui.step(format!("copying to share: {}", target.display()));
                    if !self.ui.is_dry_run() {
                        std::fs::copy(&file, &target)
                            .with_context(|| format!("failed to copy to {}", target.display()))?;
                    }
                }

                urls.push(target.display().to_string());
                if let Some(hash) = &download.hash {
                    hashes.push(hash.clone());
                }
            }
            new_arch_blocks.insert(arch.key().to_string(), (urls, hashes));
        }

        rewrite_manifest(&mut manifest, new_arch_blocks);

        let bucket_dir = Path::new(&self.config.bucket_path).join("bucket");
        if !self.ui.is_dry_run() {
            std::fs::create_dir_all(&bucket_dir)
                .with_context(|| format!("failed to create {}", bucket_dir.display()))?;
        }
        let out_path = bucket_dir.join(format!("{app}.json"));
        self.ui.step(format!("writing {}", out_path.display()));
        if !self.ui.is_dry_run() {
            std::fs::write(&out_path, serde_json::to_string_pretty(&manifest)?)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
        }

        self.ui.success(format!("mirrored '{app}' {} -> {}", manifest.version, out_path.display()));
        Ok(())
    }

    fn print_plan(&self, app: &str, manifest: &Manifest, vendor: &str, app_name: &str, archs: &[Architecture]) -> Result<()> {
        self.ui.info(format!("app:         {app}"));
        self.ui.info(format!("version:     {}", manifest.version));
        self.ui.info(format!("vendor:      {vendor}"));
        self.ui.info(format!("app name:    {app_name}"));
        for arch in archs {
            let resolved = manifest.resolved(*arch)?;
            for download in &resolved.downloads {
                let rel = share_relative_path(vendor, app_name, &manifest.version, *arch, &download.download.file_name());
                let target = Path::new(&self.config.binary_share).join(&rel);
                self.ui.info(format!(
                    "  [{}] {} -> {}",
                    arch.key(),
                    download.download.url,
                    target.display()
                ));
            }
        }
        Ok(())
    }
}

/// `<share>\Windows\<Vendor>\<AppName> <Version>\<arch>\<file>`
/// (specification section 7.4).
fn share_relative_path(vendor: &str, app_name: &str, version: &str, arch: Architecture, file_name: &str) -> PathBuf {
    let arch_dir = match arch {
        Architecture::X64 => "x64",
        Architecture::X86 => "x86",
        Architecture::Arm64 => "arm64",
    };
    PathBuf::from("Windows")
        .join(vendor)
        .join(format!("{app_name} {version}"))
        .join(arch_dir)
        .join(file_name)
}

/// Best-effort vendor detection from the manifest's `homepage` URL.
/// Override with `--vendor` when the heuristic is wrong.
fn detect_vendor(homepage: Option<&str>) -> String {
    let Some(homepage) = homepage else {
        return "Unknown".to_string();
    };
    let host = homepage.split("://").nth(1).unwrap_or(homepage);
    let host = host.split('/').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    let parts: Vec<&str> = host.split('.').collect();
    let name = if parts.len() >= 2 { parts[parts.len() - 2] } else { parts[0] };
    if name.is_empty() {
        return "Unknown".to_string();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".to_string(),
    }
}

/// Rewrites `manifest` in place: drops root-level `url`/`hash`,
/// `autoupdate`/`checkver`, and replaces the `architecture` block with the
/// mirrored architectures only (specification section 7.3, steps 5-7).
fn rewrite_manifest(manifest: &mut Manifest, new_arch_blocks: HashMap<String, (Vec<String>, Vec<String>)>) {
    manifest.url = None;
    manifest.hash = None;
    manifest.extra.remove("autoupdate");
    manifest.extra.remove("checkver");

    let mut arch_map = HashMap::new();
    for (key, (urls, hashes)) in new_arch_blocks {
        let mut entry = manifest
            .architecture
            .as_ref()
            .and_then(|m| m.get(&key))
            .cloned()
            .unwrap_or_default();
        entry.url = Some(to_one_or_many(urls));
        entry.hash = if hashes.is_empty() {
            None
        } else {
            Some(to_one_or_many(hashes))
        };
        arch_map.insert(key, entry);
    }
    manifest.architecture = Some(arch_map);
}

fn to_one_or_many(mut values: Vec<String>) -> OneOrMany<String> {
    if values.len() == 1 {
        OneOrMany::One(values.remove(0))
    } else {
        OneOrMany::Many(values)
    }
}
