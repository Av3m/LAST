//! `.zip` extraction.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

use super::Extractor;

pub struct ZipExtractor;

impl Extractor for ZipExtractor {
    fn supports(&self, file_name: &str) -> bool {
        file_name.ends_with(".zip")
    }

    fn extract(&self, archive: &Path, dest: &Path) -> Result<()> {
        extract_zip(archive, dest)
    }
}

/// Extracts all entries of a ZIP archive into `dest`. Also used by
/// [`crate::bucket::zip::ZipBucket`] to unpack bucket archives.
pub fn extract_zip(archive_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("failed to open {}", archive_path.display()))?;
    let mut archive = ::zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read ZIP archive {}", archive_path.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel_path) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let out_path = dest.join(&rel_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(&out_path)
            .with_context(|| format!("failed to create {}", out_path.display()))?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        std::io::Write::write_all(&mut out_file, &buf)?;
    }
    Ok(())
}
