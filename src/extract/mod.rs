//! Archive extraction (specification section 4.3).
//!
//! [`ExtractorRegistry`] dispatches to a format-specific [`Extractor`] based
//! on the archive's file extension. `.zip`, `.7z`, `.tar` and `.tar.gz` are
//! fully supported in this version; other formats listed in the
//! specification (`.tar.bz2`, `.tar.xz`, `.lzma`, `.lzh`, `.rar`, `.msi`)
//! report a clear "unsupported format" error so they can be added later
//! without changing the call sites in `install.rs`.

mod sevenzip;
mod tar;
pub mod zip;

use std::path::Path;

use anyhow::Result;

use crate::error::LastError;

/// A single archive format extractor.
pub trait Extractor {
    /// Whether this extractor can handle a file with the given (lowercase)
    /// name.
    fn supports(&self, file_name: &str) -> bool;

    /// Extracts `archive` into `dest` (which must already exist).
    fn extract(&self, archive: &Path, dest: &Path) -> Result<()>;
}

/// Dispatches extraction to the appropriate [`Extractor`] based on file
/// extension.
pub struct ExtractorRegistry {
    extractors: Vec<Box<dyn Extractor>>,
}

impl ExtractorRegistry {
    pub fn new() -> Self {
        Self {
            extractors: vec![
                Box::new(zip::ZipExtractor),
                Box::new(sevenzip::SevenZipExtractor),
                Box::new(tar::TarExtractor),
            ],
        }
    }

    /// Extracts `archive` into `dest`, creating `dest` if necessary.
    pub fn extract(&self, archive: &Path, dest: &Path) -> Result<()> {
        std::fs::create_dir_all(dest)?;
        let file_name = archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        for extractor in &self.extractors {
            if extractor.supports(&file_name) {
                return extractor.extract(archive, dest);
            }
        }

        Err(LastError::UnsupportedArchiveFormat(file_name).into())
    }

    /// Whether `file_name` is a recognized archive that LAST can extract in
    /// this version.
    pub fn is_supported(&self, file_name: &str) -> bool {
        let file_name = file_name.to_ascii_lowercase();
        self.extractors.iter().any(|e| e.supports(&file_name))
    }
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
