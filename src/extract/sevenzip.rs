//! `.7z` extraction via the pure-Rust `sevenz-rust` crate.

use std::path::Path;

use anyhow::{Context, Result};

use super::Extractor;

pub struct SevenZipExtractor;

impl Extractor for SevenZipExtractor {
    fn supports(&self, file_name: &str) -> bool {
        file_name.ends_with(".7z")
    }

    fn extract(&self, archive: &Path, dest: &Path) -> Result<()> {
        sevenz_rust::decompress_file(archive, dest)
            .with_context(|| format!("failed to extract 7z archive {}", archive.display()))
    }
}
