//! Filesystem path helpers: LAST_ROOT resolution, junctions and hard links.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolves the LAST root directory according to the precedence rules from
/// the specification: CLI flag > config file > `LAST` env var > default
/// (`%USERPROFILE%\last`).
///
/// This function only handles the env-var and default cases; the CLI flag
/// and config file are resolved by the caller (which has access to both).
pub fn env_or_default_root() -> PathBuf {
    if let Ok(root) = std::env::var("LAST") {
        if !root.trim().is_empty() {
            return PathBuf::from(root);
        }
    }
    default_root()
}

/// `%USERPROFILE%\last`
pub fn default_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("last")
}

/// Creates a directory junction (Windows) pointing `link` -> `target`.
///
/// If `link` already exists (as a junction, directory or file) it is removed
/// first.
pub fn create_junction(target: &Path, link: &Path) -> Result<()> {
    if link.exists() || is_dangling_reparse_point(link) {
        remove_link(link)?;
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    junction::create(target, link)
        .with_context(|| format!("failed to create junction {} -> {}", link.display(), target.display()))
}

/// Removes a junction or directory symlink without touching its target.
pub fn remove_junction(link: &Path) -> Result<()> {
    remove_link(link)
}

/// Creates a hard link `link` -> `target` for a single file. If `link`
/// already exists it is removed first. Falls back to a plain copy if hard
/// linking is not possible (e.g. across volumes).
pub fn create_hardlink(target: &Path, link: &Path) -> Result<()> {
    if link.exists() {
        std::fs::remove_file(link)?;
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::hard_link(target, link) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(target, link).map(|_| ()).with_context(|| {
            format!(
                "failed to hard-link or copy {} -> {}",
                target.display(),
                link.display()
            )
        }),
    }
}

/// Removes a link (junction, symlink or plain file/dir) at `path` without
/// recursing into / deleting the target it points to.
pub fn remove_link(path: &Path) -> Result<()> {
    if !path.exists() && !is_dangling_reparse_point(path) {
        return Ok(());
    }
    // Junctions and directory symlinks must be removed with `remove_dir`,
    // not `remove_dir_all`, to avoid touching the target. `FileType::is_dir`
    // is unreliable for junctions on Windows, so try `remove_dir` first
    // (which handles directory reparse points, including dangling ones)
    // and fall back to `remove_file` for hard-linked files.
    if std::fs::remove_dir(path).is_ok() {
        return Ok(());
    }
    std::fs::remove_file(path)
        .with_context(|| format!("failed to remove link {}", path.display()))
}

/// Checks whether the path is a dangling reparse point (junction whose
/// target no longer exists) - `Path::exists` returns `false` for these.
fn is_dangling_reparse_point(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Returns true if `url` looks like a UNC path (`\\server\share\...`).
pub fn is_unc_path(url: &str) -> bool {
    url.starts_with("\\\\")
}

/// Returns true if `url` looks like a local filesystem path rather than a
/// remote URL (drive letter, UNC path, or relative/absolute path without a
/// scheme).
pub fn is_local_path(url: &str) -> bool {
    if is_unc_path(url) {
        return true;
    }
    // Drive letter, e.g. "C:\..." or "C:/..."
    let bytes = url.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return true;
    }
    !url.contains("://")
}

/// Sets a persistent per-user environment variable via `setx`.
///
/// Note: `setx` truncates values longer than 1024 characters - acceptable
/// for the environment variables manifests typically set, but a known
/// limitation of this implementation.
pub fn set_user_env(name: &str, value: &str) -> Result<()> {
    let status = std::process::Command::new("setx").args([name, value]).status()
        .with_context(|| format!("failed to run setx {name}"))?;
    if !status.success() {
        anyhow::bail!("setx {name} failed with status {status}");
    }
    Ok(())
}

/// Removes a persistent per-user environment variable from the registry.
/// Not an error if the variable does not exist.
pub fn unset_user_env(name: &str) -> Result<()> {
    let _ = std::process::Command::new("reg")
        .args(["delete", "HKCU\\Environment", "/v", name, "/f"])
        .status();
    Ok(())
}

/// Reads a persistent per-user environment variable directly from the
/// registry (so it reflects values set by `setx` in the current session,
/// which `std::env::var` would not see).
pub fn get_user_env(name: &str) -> Result<Option<String>> {
    let output = std::process::Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", name])
        .output()
        .with_context(|| format!("failed to run reg query for {name}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        for token in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(idx) = line.find(token) {
                let value = line[idx + token.len()..].trim();
                return Ok(Some(value.to_string()));
            }
        }
    }
    Ok(None)
}

/// Appends `entry` to the user `PATH` if it is not already present.
pub fn add_user_path_entry(entry: &str) -> Result<()> {
    let current = get_user_env("Path")?.unwrap_or_default();
    if current.split(';').any(|p| p.eq_ignore_ascii_case(entry)) {
        return Ok(());
    }
    let new_value = if current.trim().is_empty() {
        entry.to_string()
    } else {
        format!("{current};{entry}")
    };
    set_user_env("Path", &new_value)
}

/// Removes `entry` from the user `PATH`, if present.
pub fn remove_user_path_entry(entry: &str) -> Result<()> {
    let Some(current) = get_user_env("Path")? else {
        return Ok(());
    };
    let new_value: Vec<&str> = current
        .split(';')
        .filter(|p| !p.eq_ignore_ascii_case(entry))
        .collect();
    set_user_env("Path", &new_value.join(";"))
}
