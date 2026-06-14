//! Hash computation and verification (sha256/sha512/sha1/md5).

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Result};
use sha2::{Digest, Sha256, Sha512};

/// Supported hash algorithms, as used in Scoop/LAST manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha512,
    Sha1,
    Md5,
}

impl HashAlgorithm {
    fn from_prefix(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            "sha1" => Some(Self::Sha1),
            "md5" => Some(Self::Md5),
            _ => None,
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Sha1 => "sha1",
            Self::Md5 => "md5",
        };
        f.write_str(name)
    }
}

/// A parsed hash specification, e.g. `sha256:<hex>` or a bare hex digest
/// (which is assumed to be sha256, the default).
#[derive(Debug, Clone)]
pub struct HashSpec {
    pub algorithm: HashAlgorithm,
    pub digest: String,
}

impl HashSpec {
    /// Parses `<algo>:<hexdigest>` or a bare hex digest (defaults to sha256).
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if let Some((algo, digest)) = spec.split_once(':') {
            let algorithm = HashAlgorithm::from_prefix(algo)
                .ok_or_else(|| anyhow::anyhow!("unsupported hash algorithm: {algo}"))?;
            Ok(Self {
                algorithm,
                digest: digest.to_ascii_lowercase(),
            })
        } else {
            if !spec.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("invalid hash specification: {spec}");
            }
            Ok(Self {
                algorithm: HashAlgorithm::Sha256,
                digest: spec.to_ascii_lowercase(),
            })
        }
    }
}

impl fmt::Display for HashSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.digest)
    }
}

/// Computes the hex digest of a file for the given algorithm.
pub fn compute_hash(path: &Path, algorithm: HashAlgorithm) -> Result<String> {
    let mut file = File::open(path)?;
    let digest = match algorithm {
        HashAlgorithm::Sha256 => hash_with(&mut file, Sha256::new())?,
        HashAlgorithm::Sha512 => hash_with(&mut file, Sha512::new())?,
        HashAlgorithm::Sha1 => hash_with(&mut file, sha1::Sha1::new())?,
        HashAlgorithm::Md5 => hash_with(&mut file, md5::Md5::new())?,
    };
    Ok(digest)
}

fn hash_with<D: Digest>(file: &mut File, mut hasher: D) -> Result<String> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verifies that `path` matches `expected` (`<algo>:<hexdigest>` or bare
/// hex). Returns an error describing the mismatch if verification fails.
pub fn verify_file(path: &Path, expected: &str) -> Result<()> {
    let spec = HashSpec::parse(expected)?;
    let actual = compute_hash(path, spec.algorithm)?;
    if actual.eq_ignore_ascii_case(&spec.digest) {
        Ok(())
    } else {
        bail!(
            "hash mismatch for {}: expected {} {}, got {}",
            path.display(),
            spec.algorithm,
            spec.digest,
            actual
        )
    }
}

/// Minimal hex encoding helper so we don't need an extra dependency just for
/// `Vec<u8> -> String`.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let bytes = bytes.as_ref();
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}
