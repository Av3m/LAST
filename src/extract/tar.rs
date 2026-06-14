//! `.tar` and `.tar.gz` extraction.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;

use super::Extractor;

pub struct TarExtractor;

impl Extractor for TarExtractor {
    fn supports(&self, file_name: &str) -> bool {
        file_name.ends_with(".tar") || file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz")
    }

    fn extract(&self, archive: &Path, dest: &Path) -> Result<()> {
        let file = File::open(archive)
            .with_context(|| format!("failed to open {}", archive.display()))?;

        let file_name = archive.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            let decoder = GzDecoder::new(file);
            let mut archive = ::tar::Archive::new(decoder);
            archive
                .unpack(dest)
                .with_context(|| format!("failed to extract {file_name}"))
        } else {
            let mut archive = ::tar::Archive::new(file);
            archive
                .unpack(dest)
                .with_context(|| format!("failed to extract {file_name}"))
        }
    }
}
