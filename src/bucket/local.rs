//! Local-directory bucket (Format B, see specification section 6.1).

use std::path::PathBuf;

use anyhow::Result;

use super::Bucket;

/// A bucket backed directly by a local directory.
///
/// The directory may either contain the manifest JSON files directly, or a
/// `bucket\` subdirectory containing them (matching the layout used inside
/// ZIP buckets). [`LocalBucket::manifest_dir`] picks whichever is present.
pub struct LocalBucket {
    name: String,
    path: PathBuf,
}

impl LocalBucket {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }
}

impl Bucket for LocalBucket {
    fn name(&self) -> &str {
        &self.name
    }

    fn manifest_dir(&self) -> PathBuf {
        let nested = self.path.join("bucket");
        if nested.is_dir() {
            nested
        } else {
            self.path.clone()
        }
    }

    /// Local directory buckets are read directly - no update step needed.
    fn update(&self) -> Result<()> {
        Ok(())
    }
}
