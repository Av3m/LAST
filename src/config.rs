//! Configuration management (`%LAST_ROOT%\config.json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::LastError;

/// Source of a registered bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketSource {
    /// A local directory containing a `bucket\` subdirectory with manifests.
    Local { path: String },
    /// A ZIP archive, either on a local/UNC path or an HTTP(S) URL. The
    /// archive is extracted into the bucket cache on registration and on
    /// `last bucket update`.
    Zip { url: String },
}

/// A registered bucket entry, as stored in `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketEntry {
    pub source: BucketSource,
}

/// Mirror-specific configuration (see specification section 7.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorConfig {
    #[serde(default)]
    pub binary_share: String,
    #[serde(default = "default_architectures")]
    pub architectures: Vec<String>,
    #[serde(default)]
    pub download_proxy: String,
    #[serde(default)]
    pub bucket_path: String,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            binary_share: String::new(),
            architectures: default_architectures(),
            download_proxy: String::new(),
            bucket_path: String::new(),
        }
    }
}

fn default_architectures() -> Vec<String> {
    vec!["64bit".to_string()]
}

fn default_true() -> bool {
    true
}

/// Top-level configuration, persisted as `%LAST_ROOT%\config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_architectures")]
    pub architectures: Vec<String>,

    #[serde(default)]
    pub download_proxy: String,

    #[serde(default = "default_true")]
    pub powershell_compat: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<String>,

    #[serde(default)]
    pub mirror: MirrorConfig,

    /// Registered buckets, in registration order (insertion order is
    /// preserved by `BTreeMap`'s `IndexMap`-like behaviour is *not*
    /// guaranteed, so registration order is tracked explicitly via
    /// `bucket_order`).
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketEntry>,

    /// Order in which buckets were registered; used for package lookup
    /// priority (see specification section 6.6).
    #[serde(default)]
    pub bucket_order: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            architectures: default_architectures(),
            download_proxy: String::new(),
            powershell_compat: true,
            last_update: None,
            mirror: MirrorConfig::default(),
            buckets: BTreeMap::new(),
            bucket_order: Vec::new(),
        }
    }
}

impl Config {
    /// Loads the configuration from `path`. If the file does not exist, a
    /// default configuration is returned (it is not written to disk until
    /// [`Config::save`] is called).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Config = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        Ok(config)
    }

    /// Writes the configuration to `path` as pretty-printed JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config directory {}", parent.display()))?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write config file {}", path.display()))
    }

    /// Registers a bucket. Fails if a bucket with the same name already
    /// exists.
    pub fn add_bucket(&mut self, name: &str, source: BucketSource) -> Result<()> {
        if self.buckets.contains_key(name) {
            bail!(LastError::BucketAlreadyExists(name.to_string()));
        }
        self.buckets.insert(name.to_string(), BucketEntry { source });
        self.bucket_order.push(name.to_string());
        Ok(())
    }

    /// Removes a registered bucket.
    pub fn remove_bucket(&mut self, name: &str) -> Result<()> {
        if self.buckets.remove(name).is_none() {
            bail!(LastError::BucketNotFound(name.to_string()));
        }
        self.bucket_order.retain(|n| n != name);
        Ok(())
    }

    /// Returns registered buckets in registration order.
    pub fn ordered_buckets(&self) -> Vec<(&String, &BucketEntry)> {
        self.bucket_order
            .iter()
            .filter_map(|name| self.buckets.get(name).map(|entry| (name, entry)))
            .collect()
    }

    /// Gets a configuration value by dotted key path (e.g.
    /// `mirror.binary_share`).
    pub fn get(&self, key: &str) -> Result<Value> {
        let root = serde_json::to_value(self)?;
        get_path(&root, key)
            .cloned()
            .ok_or_else(|| LastError::UnknownConfigKey(key.to_string()).into())
    }

    /// Sets a configuration value by dotted key path. The value is parsed as
    /// JSON if possible, otherwise treated as a plain string.
    pub fn set(&mut self, key: &str, raw_value: &str) -> Result<()> {
        let value: Value = serde_json::from_str(raw_value)
            .unwrap_or_else(|_| Value::String(raw_value.to_string()));

        let mut root = serde_json::to_value(&*self)?;
        set_path(&mut root, key, value)?;
        *self = serde_json::from_value(root)
            .with_context(|| format!("invalid value for configuration key '{key}'"))?;
        Ok(())
    }

    /// Returns the configuration as a flattened list of `(key, value)`
    /// pairs, suitable for `last config list`.
    pub fn list(&self) -> Vec<(String, Value)> {
        let root = serde_json::to_value(self).unwrap_or(Value::Null);
        let mut out = Vec::new();
        flatten("", &root, &mut out);
        out
    }
}

fn get_path<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in key.split('.') {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}

fn set_path(value: &mut Value, key: &str, new_value: Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = value;
    for (i, part) in parts.iter().enumerate() {
        if !current.is_object() {
            *current = Value::Object(serde_json::Map::new());
        }
        let map = current.as_object_mut().expect("just ensured object");
        if i == parts.len() - 1 {
            map.insert(part.to_string(), new_value);
            return Ok(());
        }
        current = map
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    Ok(())
}

fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        other => out.push((prefix.to_string(), other.clone())),
    }
}

/// Returns the path to `config.json` within `root`.
pub fn config_path(root: &Path) -> PathBuf {
    root.join("config.json")
}
