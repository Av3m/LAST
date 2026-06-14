//! Scoop-compatible manifest parsing, validation and architecture resolution.
//!
//! Scoop manifests are JSON documents with a number of fields that can
//! appear either as a single value or as an array (and, for `bin`, as a
//! mixture of strings and `[exe, alias, ...args]` arrays). The types in this
//! module mirror that flexibility while giving the rest of LAST a single,
//! architecture-resolved view via [`Manifest::resolved`].

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// A value that may appear as a single item or as an array in JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T: Clone> OneOrMany<T> {
    pub fn to_vec(&self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

/// A `bin` entry: either a bare executable name, or `[exe, alias, ...args]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BinEntry {
    Simple(String),
    WithArgs(Vec<String>),
}

impl BinEntry {
    /// The path to the executable, relative to the install directory.
    pub fn exe(&self) -> &str {
        match self {
            BinEntry::Simple(s) => s,
            BinEntry::WithArgs(v) => v.first().map(String::as_str).unwrap_or_default(),
        }
    }

    /// The shim name (defaults to the executable's file stem).
    pub fn alias(&self) -> Option<&str> {
        match self {
            BinEntry::Simple(_) => None,
            BinEntry::WithArgs(v) => v.get(1).map(String::as_str),
        }
    }

    /// Additional fixed arguments always passed to the shim.
    pub fn extra_args(&self) -> &[String] {
        match self {
            BinEntry::Simple(_) => &[],
            BinEntry::WithArgs(v) if v.len() > 2 => &v[2..],
            BinEntry::WithArgs(_) => &[],
        }
    }
}

/// `license` field: either a plain SPDX identifier string, or
/// `{ "identifier": ..., "url": ... }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum License {
    Simple(String),
    Detailed {
        identifier: String,
        #[serde(default)]
        url: Option<String>,
    },
}

/// A script field: inline source (string or array of lines), or a reference
/// to an external script file (LAST extension).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptField {
    Inline(OneOrMany<String>),
    File { script_file: String },
}

impl ScriptField {
    /// Resolves the script to its source text. External script files are
    /// read relative to `base_dir` (the bucket's manifest directory).
    pub fn resolve(&self, base_dir: &Path) -> Result<String> {
        match self {
            ScriptField::Inline(lines) => Ok(lines.to_vec().join("\n")),
            ScriptField::File { script_file } => {
                let path = base_dir.join(script_file);
                std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read script file {}", path.display()))
            }
        }
    }
}

/// `installer` / `uninstaller` field.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallerField {
    pub file: Option<String>,
    pub script: Option<ScriptField>,
    pub args: Option<OneOrMany<String>>,
    #[serde(default)]
    pub keep: bool,
}

/// Fields that can be overridden per architecture inside the
/// `"architecture"` block.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArchManifest {
    pub url: Option<OneOrMany<String>>,
    pub hash: Option<OneOrMany<String>>,
    pub bin: Option<OneOrMany<BinEntry>>,
    pub extract_dir: Option<OneOrMany<String>>,
    pub extract_to: Option<OneOrMany<String>>,
    pub env_add_path: Option<OneOrMany<String>>,
    pub env_set: Option<HashMap<String, String>>,
    pub pre_install: Option<ScriptField>,
    pub post_install: Option<ScriptField>,
    pub pre_uninstall: Option<ScriptField>,
    pub post_uninstall: Option<ScriptField>,
    pub installer: Option<InstallerField>,
    pub uninstaller: Option<InstallerField>,
}

/// A Scoop-compatible package manifest.
///
/// Unknown/unsupported Scoop-specific fields (`autoupdate`, `checkver`) are
/// accepted but ignored - see [`Manifest::ignored_fields`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    /// App name, derived from the manifest filename (not part of the JSON).
    #[serde(skip)]
    pub name: String,

    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<License>,

    pub url: Option<OneOrMany<String>>,
    pub hash: Option<OneOrMany<String>>,
    pub extract_dir: Option<OneOrMany<String>>,
    pub extract_to: Option<OneOrMany<String>>,

    pub bin: Option<OneOrMany<BinEntry>>,
    pub shortcuts: Option<Vec<Vec<String>>>,
    pub persist: Option<OneOrMany<String>>,
    pub env_set: Option<HashMap<String, String>>,
    pub env_add_path: Option<OneOrMany<String>>,

    pub depends: Option<OneOrMany<String>>,
    pub suggest: Option<HashMap<String, Vec<String>>>,

    pub architecture: Option<HashMap<String, ArchManifest>>,

    pub installer: Option<InstallerField>,
    pub uninstaller: Option<InstallerField>,
    pub pre_install: Option<ScriptField>,
    pub post_install: Option<ScriptField>,
    pub pre_uninstall: Option<ScriptField>,
    pub post_uninstall: Option<ScriptField>,

    pub notes: Option<OneOrMany<String>>,

    /// Scoop fields that LAST intentionally ignores (`autoupdate`,
    /// `checkver`). Captured so round-tripping (e.g. for `last mirror`)
    /// can report what was dropped, without failing on unknown fields.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Target CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Architecture {
    X64,
    X86,
    Arm64,
}

impl Architecture {
    /// The architecture key as used in manifest `architecture` blocks and
    /// the `--arch` flag.
    pub fn key(&self) -> &'static str {
        match self {
            Architecture::X64 => "64bit",
            Architecture::X86 => "32bit",
            Architecture::Arm64 => "arm64",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "64bit" => Ok(Architecture::X64),
            "32bit" => Ok(Architecture::X86),
            "arm64" => Ok(Architecture::Arm64),
            other => bail!("unknown architecture '{other}', expected 64bit, 32bit or arm64"),
        }
    }

    /// Detects the architecture of the host running LAST.
    pub fn detect() -> Self {
        match std::env::consts::ARCH {
            "x86_64" => Architecture::X64,
            "x86" => Architecture::X86,
            "aarch64" => Architecture::Arm64,
            _ => Architecture::X64,
        }
    }
}

/// One `(url, override_filename)` download entry, after resolving the Scoop
/// rename trick (`https://host/file.exe#/renamed.7z`).
#[derive(Debug, Clone)]
pub struct DownloadUrl {
    pub url: String,
    pub rename_to: Option<String>,
}

impl DownloadUrl {
    fn parse(raw: &str) -> Self {
        match raw.split_once('#') {
            Some((url, fragment)) if fragment.starts_with('/') => DownloadUrl {
                url: url.to_string(),
                rename_to: Some(fragment.trim_start_matches('/').to_string()),
            },
            _ => DownloadUrl {
                url: raw.to_string(),
                rename_to: None,
            },
        }
    }

    /// The file name to use for the downloaded artifact: the override from
    /// the rename trick, or the last path segment of the URL.
    pub fn file_name(&self) -> String {
        if let Some(name) = &self.rename_to {
            return name.clone();
        }
        self.url
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("download")
            .to_string()
    }
}

/// One download to perform: URL plus the expected hash, if any.
#[derive(Debug, Clone)]
pub struct ResolvedDownload {
    pub download: DownloadUrl,
    pub hash: Option<String>,
}

/// A manifest with all fields resolved for a specific architecture, merging
/// the architecture-specific overrides over the root-level defaults.
#[derive(Debug, Clone, Default)]
pub struct ResolvedManifest {
    pub downloads: Vec<ResolvedDownload>,
    pub extract_dir: Option<String>,
    pub extract_to: Option<String>,
    pub bin: Vec<BinEntry>,
    pub env_add_path: Vec<String>,
    pub env_set: HashMap<String, String>,
    pub pre_install: Option<ScriptField>,
    pub post_install: Option<ScriptField>,
    pub pre_uninstall: Option<ScriptField>,
    pub post_uninstall: Option<ScriptField>,
    pub installer: Option<InstallerField>,
    pub uninstaller: Option<InstallerField>,
}

impl Manifest {
    /// Loads and parses a manifest from `path`. The app name is derived from
    /// the file stem (e.g. `bucket/vscodium.json` -> `vscodium`).
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        Self::load_from_str(&content, path)
    }

    /// Parses a manifest from a JSON string. `path` is only used to derive
    /// the app name and for error messages.
    pub fn load_from_str(content: &str, path: &Path) -> Result<Self> {
        let mut manifest: Manifest = serde_json::from_str(content)
            .with_context(|| format!("failed to parse manifest {}", path.display()))?;
        manifest.name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        manifest.validate()?;
        Ok(manifest)
    }

    /// Performs basic structural validation.
    pub fn validate(&self) -> Result<()> {
        if self.version.trim().is_empty() {
            bail!("manifest '{}' is missing a 'version' field", self.name);
        }
        if self.url.is_none() && self.architecture.is_none() && self.installer.is_none() {
            bail!(
                "manifest '{}' has no 'url', 'architecture' or 'installer' field",
                self.name
            );
        }
        Ok(())
    }

    /// Returns the architecture-specific override block, if present.
    fn arch_block(&self, arch: Architecture) -> Option<&ArchManifest> {
        self.architecture.as_ref()?.get(arch.key())
    }

    /// Resolves the manifest for the given architecture. Returns an error if
    /// no download URL is available for that architecture.
    pub fn resolved(&self, arch: Architecture) -> Result<ResolvedManifest> {
        let arch_block = self.arch_block(arch);

        let urls: Vec<String> = arch_block
            .and_then(|a| a.url.as_ref())
            .or(self.url.as_ref())
            .map(OneOrMany::to_vec)
            .ok_or_else(|| crate::error::LastError::NoUrlForArchitecture(arch.key().to_string()))?;

        let hashes: Vec<String> = arch_block
            .and_then(|a| a.hash.as_ref())
            .or(self.hash.as_ref())
            .map(OneOrMany::to_vec)
            .unwrap_or_default();

        let downloads = urls
            .into_iter()
            .enumerate()
            .map(|(i, raw)| ResolvedDownload {
                download: DownloadUrl::parse(&raw),
                hash: hashes.get(i).cloned(),
            })
            .collect();

        let extract_dir = arch_block
            .and_then(|a| a.extract_dir.as_ref())
            .or(self.extract_dir.as_ref())
            .map(|v| v.to_vec().into_iter().next().unwrap_or_default());

        let extract_to = arch_block
            .and_then(|a| a.extract_to.as_ref())
            .or(self.extract_to.as_ref())
            .map(|v| v.to_vec().into_iter().next().unwrap_or_default());

        let bin = arch_block
            .and_then(|a| a.bin.as_ref())
            .or(self.bin.as_ref())
            .map(OneOrMany::to_vec)
            .unwrap_or_default();

        let env_add_path = arch_block
            .and_then(|a| a.env_add_path.as_ref())
            .or(self.env_add_path.as_ref())
            .map(OneOrMany::to_vec)
            .unwrap_or_default();

        let env_set = arch_block
            .and_then(|a| a.env_set.clone())
            .or_else(|| self.env_set.clone())
            .unwrap_or_default();

        let pre_install = arch_block
            .and_then(|a| a.pre_install.clone())
            .or_else(|| self.pre_install.clone());
        let post_install = arch_block
            .and_then(|a| a.post_install.clone())
            .or_else(|| self.post_install.clone());
        let pre_uninstall = arch_block
            .and_then(|a| a.pre_uninstall.clone())
            .or_else(|| self.pre_uninstall.clone());
        let post_uninstall = arch_block
            .and_then(|a| a.post_uninstall.clone())
            .or_else(|| self.post_uninstall.clone());
        let installer = arch_block
            .and_then(|a| a.installer.clone())
            .or_else(|| self.installer.clone());
        let uninstaller = arch_block
            .and_then(|a| a.uninstaller.clone())
            .or_else(|| self.uninstaller.clone());

        Ok(ResolvedManifest {
            downloads,
            extract_dir,
            extract_to,
            bin,
            env_add_path,
            env_set,
            pre_install,
            post_install,
            pre_uninstall,
            post_uninstall,
            installer,
            uninstaller,
        })
    }

    /// `persist` entries (paths relative to the install directory).
    pub fn persist_entries(&self) -> Vec<String> {
        self.persist
            .as_ref()
            .map(OneOrMany::to_vec)
            .unwrap_or_default()
    }

    /// Dependency app names (`<bucket>/<app>` or `<app>`).
    pub fn depends(&self) -> Vec<String> {
        self.depends
            .as_ref()
            .map(OneOrMany::to_vec)
            .unwrap_or_default()
    }

    /// Fields present in the manifest that LAST ignores entirely.
    pub fn ignored_fields(&self) -> Vec<&str> {
        ["autoupdate", "checkver"]
            .into_iter()
            .filter(|f| self.extra.contains_key(*f))
            .collect()
    }

    /// List of architecture keys for which this manifest provides a
    /// download URL.
    pub fn available_architectures(&self) -> Vec<&'static str> {
        let mut result = Vec::new();
        for arch in [Architecture::X64, Architecture::X86, Architecture::Arm64] {
            if self.resolved(arch).is_ok() {
                result.push(arch.key());
            }
        }
        result
    }
}
