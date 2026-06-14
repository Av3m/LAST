//! Downloading with mandatory hash verification (specification sections 4.2
//! and 4.5).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::hash;
use crate::manifest::ResolvedDownload;
use crate::path::is_local_path;
use crate::ui::Ui;

/// Downloads package archives to a cache directory, verifying their hash.
pub struct Downloader {
    client: reqwest::blocking::Client,
}

impl Downloader {
    /// Creates a downloader, optionally routing HTTP(S) traffic through
    /// `proxy` (see `download_proxy` in `config.json` / `LAST_PROXY`).
    pub fn new(proxy: Option<&str>) -> Result<Self> {
        let mut builder = reqwest::blocking::Client::builder().user_agent("last/0.1");
        if let Some(proxy) = proxy.filter(|p| !p.is_empty()) {
            let proxy = reqwest::Proxy::all(proxy)
                .with_context(|| format!("invalid download proxy '{proxy}'"))?;
            builder = builder.proxy(proxy);
        }
        let client = builder.build().context("failed to build HTTP client")?;
        Ok(Self { client })
    }

    /// Downloads (or copies, for local/UNC sources) a single resolved
    /// download into `dest_dir`, verifying its hash if one is specified.
    /// Returns the path to the downloaded file.
    ///
    /// If the destination file already exists and matches the expected
    /// hash, the download is skipped (cache hit).
    pub fn fetch(&self, download: &ResolvedDownload, dest_dir: &Path, ui: &Ui) -> Result<PathBuf> {
        std::fs::create_dir_all(dest_dir)?;
        let dest = dest_dir.join(download.download.file_name());

        if dest.is_file() {
            if let Some(expected) = &download.hash {
                if hash::verify_file(&dest, expected).is_ok() {
                    ui.debug(format!("using cached download {}", dest.display()));
                    return Ok(dest);
                }
            }
        }

        let url = &download.download.url;
        if ui.is_dry_run() {
            ui.step(format!("would download {url}"));
            return Ok(dest);
        }

        ui.step(format!("downloading {url}"));
        if is_local_path(url) {
            self.copy_local(url, &dest)?;
        } else if url.starts_with("http://") || url.starts_with("https://") {
            self.download_http(url, &dest)?;
        } else if url.starts_with("ftp://") {
            bail!("ftp:// URLs are not yet supported by this version of LAST: {url}");
        } else {
            bail!("unsupported URL scheme: {url}");
        }

        if let Some(expected) = &download.hash {
            hash::verify_file(&dest, expected).with_context(|| {
                format!("hash verification failed for {url} (downloaded to {})", dest.display())
            })?;
            ui.debug(format!("hash OK: {expected}"));
        } else {
            ui.warn(format!("manifest does not specify a hash for {url} - skipping verification"));
        }

        Ok(dest)
    }

    fn download_http(&self, url: &str, dest: &Path) -> Result<()> {
        let mut response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("failed to download {url}"))?
            .error_for_status()
            .with_context(|| format!("server returned an error for {url}"))?;
        let mut file = std::fs::File::create(dest)
            .with_context(|| format!("failed to create {}", dest.display()))?;
        std::io::copy(&mut response, &mut file)
            .with_context(|| format!("failed to write {}", dest.display()))?;
        Ok(())
    }

    fn copy_local(&self, source: &str, dest: &Path) -> Result<()> {
        let normalized = source.replace('/', "\\");
        std::fs::copy(&normalized, dest)
            .with_context(|| format!("failed to copy {normalized} to {}", dest.display()))?;
        Ok(())
    }
}
