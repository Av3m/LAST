//! ZIP-archive bucket (Format A, see specification section 6.2).
//!
//! The ZIP is fetched from its registration URL (HTTP(S), UNC, or a local
//! path) into `%LAST_ROOT%\buckets\<name>\bucket.zip` and extracted into
//! `%LAST_ROOT%\buckets\<name>\extracted\`. `last bucket update` re-fetches
//! and re-extracts.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Bucket;
use crate::extract::zip::extract_zip;
use crate::path::is_local_path;

/// A bucket backed by a ZIP archive.
pub struct ZipBucket {
    name: String,
    /// Registration source (URL or local/UNC path) - re-used by `update`.
    source: String,
    /// `%LAST_ROOT%\buckets\<name>`
    cache_dir: PathBuf,
}

impl ZipBucket {
    pub fn new(name: impl Into<String>, source: impl Into<String>, buckets_root: &Path) -> Self {
        let name = name.into();
        let cache_dir = buckets_root.join(&name);
        Self {
            name,
            source: source.into(),
            cache_dir,
        }
    }

    fn archive_path(&self) -> PathBuf {
        self.cache_dir.join("bucket.zip")
    }

    fn extracted_dir(&self) -> PathBuf {
        self.cache_dir.join("extracted")
    }

    /// Downloads/copies the ZIP from its registration source and extracts it,
    /// replacing any previously cached copy.
    pub fn fetch(&self) -> Result<()> {
        std::fs::create_dir_all(&self.cache_dir)?;
        let archive_path = self.archive_path();
        fetch_to_file(&self.source, &archive_path)
            .with_context(|| format!("failed to fetch bucket '{}' from {}", self.name, self.source))?;

        let extracted = self.extracted_dir();
        if extracted.exists() {
            std::fs::remove_dir_all(&extracted)
                .with_context(|| format!("failed to clear old bucket cache for '{}'", self.name))?;
        }
        std::fs::create_dir_all(&extracted)?;

        extract_zip(&archive_path, &extracted)
            .with_context(|| format!("failed to extract bucket archive for '{}'", self.name))?;
        Ok(())
    }
}

impl Bucket for ZipBucket {
    fn name(&self) -> &str {
        &self.name
    }

    fn manifest_dir(&self) -> PathBuf {
        let extracted = self.extracted_dir();
        let nested = extracted.join("bucket");
        if nested.is_dir() {
            nested
        } else {
            extracted
        }
    }

    fn update(&self) -> Result<()> {
        self.fetch()
    }
}

/// Fetches `source` (HTTP(S) URL, UNC path or local path) to `dest`.
fn fetch_to_file(source: &str, dest: &Path) -> Result<()> {
    if is_local_path(source) {
        std::fs::copy(source, dest)
            .with_context(|| format!("failed to copy {source} to {}", dest.display()))?;
        return Ok(());
    }

    let response = reqwest::blocking::get(source)
        .with_context(|| format!("failed to download {source}"))?
        .error_for_status()
        .with_context(|| format!("server returned an error for {source}"))?;
    let bytes = response.bytes()?;
    std::fs::write(dest, &bytes)
        .with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}

