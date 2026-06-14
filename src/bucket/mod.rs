//! Bucket abstraction and registry.
//!
//! A [`Bucket`] is a source of manifest JSON files. LAST supports local
//! directories ([`local::LocalBucket`]) and ZIP archives
//! ([`zip::ZipBucket`]); both are exposed through the same trait so the rest
//! of the application does not need to care which kind it is dealing with.

pub mod local;
pub mod zip;

pub use local::LocalBucket;
pub use zip::ZipBucket;

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::{BucketSource, Config};
use crate::error::LastError;
use crate::manifest::Manifest;

/// A source of package manifests.
pub trait Bucket {
    /// The bucket's registered name.
    fn name(&self) -> &str;

    /// Directory containing the manifest `*.json` files.
    fn manifest_dir(&self) -> PathBuf;

    /// Re-fetches the bucket contents (no-op for local directory buckets).
    fn update(&self) -> Result<()>;

    /// Lists the app names provided by this bucket.
    fn list_apps(&self) -> Result<Vec<String>> {
        let dir = self.manifest_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut apps = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read bucket directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.eq_ignore_ascii_case("last-bucket") {
                    continue;
                }
                apps.push(stem.to_string());
            }
        }
        apps.sort();
        Ok(apps)
    }

    /// Loads the manifest for `app`, if this bucket provides it.
    fn get_manifest(&self, app: &str) -> Result<Option<Manifest>> {
        let path = self.manifest_dir().join(format!("{app}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(Manifest::load_from_file(&path)?))
    }
}

/// Registry of all configured buckets, in registration order.
pub struct BucketManager {
    buckets: Vec<Box<dyn Bucket>>,
}

impl BucketManager {
    /// Builds the bucket registry from configuration. Buckets are
    /// constructed but not fetched - call [`ZipBucket::fetch`] (e.g. via
    /// `last bucket update`) to populate the ZIP cache.
    pub fn load(root: &std::path::Path, config: &Config) -> Self {
        let buckets_root = root.join("buckets");
        let mut buckets: Vec<Box<dyn Bucket>> = Vec::new();
        for (name, entry) in config.ordered_buckets() {
            match &entry.source {
                BucketSource::Local { path } => {
                    buckets.push(Box::new(LocalBucket::new(name.clone(), PathBuf::from(path))));
                }
                BucketSource::Zip { url } => {
                    buckets.push(Box::new(ZipBucket::new(name.clone(), url.clone(), &buckets_root)));
                }
            }
        }
        Self { buckets }
    }

    pub fn buckets(&self) -> &[Box<dyn Bucket>] {
        &self.buckets
    }

    pub fn get(&self, name: &str) -> Option<&dyn Bucket> {
        self.buckets
            .iter()
            .find(|b| b.name() == name)
            .map(|b| b.as_ref())
    }

    /// Re-fetches all buckets (no-op for local directory buckets).
    pub fn update_all(&self) -> Result<Vec<String>> {
        let mut updated = Vec::new();
        for bucket in &self.buckets {
            bucket
                .update()
                .with_context(|| format!("failed to update bucket '{}'", bucket.name()))?;
            updated.push(bucket.name().to_string());
        }
        Ok(updated)
    }

    /// Resolves `query` (`app` or `bucket/app`) to a manifest, searching
    /// buckets in registration order.
    pub fn find_manifest(&self, query: &str) -> Result<(String, Manifest)> {
        if let Some((bucket_name, app)) = query.split_once('/') {
            let bucket = self
                .get(bucket_name)
                .ok_or_else(|| LastError::BucketNotFound(bucket_name.to_string()))?;
            let manifest = bucket
                .get_manifest(app)?
                .ok_or_else(|| LastError::PackageNotFound(query.to_string()))?;
            return Ok((bucket.name().to_string(), manifest));
        }

        for bucket in &self.buckets {
            if let Some(manifest) = bucket.get_manifest(query)? {
                return Ok((bucket.name().to_string(), manifest));
            }
        }
        Err(LastError::PackageNotFound(query.to_string()).into())
    }

    /// Searches all buckets for app names containing `query`
    /// (case-insensitive substring match).
    pub fn search(&self, query: &str) -> Result<Vec<(String, Manifest)>> {
        let query = query.to_ascii_lowercase();
        let mut results = Vec::new();
        for bucket in &self.buckets {
            for app in bucket.list_apps()? {
                if !app.to_ascii_lowercase().contains(&query) {
                    continue;
                }
                if let Some(manifest) = bucket.get_manifest(&app)? {
                    results.push((bucket.name().to_string(), manifest));
                }
            }
        }
        Ok(results)
    }
}
