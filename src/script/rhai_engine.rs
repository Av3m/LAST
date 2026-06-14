//! Sandboxed Rhai script engine (specification sections 5.1-5.4).
//!
//! Only the API functions listed in the specification are exposed to
//! scripts. There is no general filesystem or process access - every
//! capability a script has goes through one of the functions registered
//! here, each of which respects `--dry-run`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Scope};

use super::{ScriptContext, ScriptEngine};
use crate::extract::ExtractorRegistry;
use crate::ui::Ui;

pub struct RhaiEngine {
    engine: Engine,
    ui: Arc<Ui>,
}

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

fn rhai_err(message: impl std::fmt::Display) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.to_string().into(),
        rhai::Position::NONE,
    ))
}

fn to_rhai<T>(result: anyhow::Result<T>) -> RhaiResult<T> {
    result.map_err(rhai_err)
}

impl RhaiEngine {
    pub fn new(ui: Arc<Ui>) -> Self {
        let mut engine = Engine::new();
        register_api(&mut engine, ui.clone());
        Self { engine, ui }
    }
}

impl ScriptEngine for RhaiEngine {
    fn run(&self, source: &str, context: &ScriptContext) -> Result<()> {
        let mut scope = Scope::new();
        scope.push("dir", context.dir.display().to_string());
        scope.push("persist_dir", context.persist_dir.display().to_string());
        scope.push("version", context.version.clone());
        scope.push("app", context.app.clone());
        scope.push("architecture", context.architecture.clone());
        scope.push("scoopdir", context.last_root.display().to_string());
        scope.push("last_root", context.last_root.display().to_string());
        scope.push("global", context.global);

        self.engine
            .run_with_scope(&mut scope, source)
            .map_err(|e| anyhow::anyhow!("script error: {e}"))?;
        Ok(())
    }
}

fn register_api(engine: &mut Engine, ui: Arc<Ui>) {
    // --- Logging ---------------------------------------------------------
    {
        let ui = ui.clone();
        engine.register_fn("log", move |msg: &str| ui.info(msg));
    }
    {
        let ui = ui.clone();
        engine.register_fn("warn", move |msg: &str| ui.warn(msg));
    }
    {
        let ui = ui.clone();
        engine.register_fn("error", move |msg: &str| ui.error(msg));
    }
    {
        let ui = ui.clone();
        engine.register_fn("success", move |msg: &str| ui.success(msg));
    }

    // --- Filesystem --------------------------------------------------------
    {
        let ui = ui.clone();
        engine.register_fn("copy", move |src: &str, dst: &str| -> RhaiResult<()> {
            ui.step(format!("copy {src} -> {dst}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(copy_path(Path::new(src), Path::new(dst)))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn("copy_dir", move |src: &str, dst: &str| -> RhaiResult<()> {
            ui.step(format!("copy_dir {src} -> {dst}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(copy_path(Path::new(src), Path::new(dst)))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn("move_file", move |src: &str, dst: &str| -> RhaiResult<()> {
            ui.step(format!("move {src} -> {dst}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(move_path(Path::new(src), Path::new(dst)))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn("delete", move |path: &str| -> RhaiResult<()> {
            ui.step(format!("delete {path}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(delete_path(Path::new(path)))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn("mkdir", move |path: &str| -> RhaiResult<()> {
            ui.step(format!("mkdir {path}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(std::fs::create_dir_all(path).map_err(anyhow::Error::from))
        });
    }
    engine.register_fn("exists", |path: &str| Path::new(path).exists());
    engine.register_fn("read_file", |path: &str| -> RhaiResult<String> {
        to_rhai(std::fs::read_to_string(path).map_err(anyhow::Error::from))
    });
    {
        let ui = ui.clone();
        engine.register_fn("write_file", move |path: &str, content: &str| -> RhaiResult<()> {
            ui.step(format!("write {path}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(std::fs::write(path, content).map_err(anyhow::Error::from))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn("rename", move |src: &str, dst: &str| -> RhaiResult<()> {
            ui.step(format!("rename {src} -> {dst}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(std::fs::rename(src, dst).map_err(anyhow::Error::from))
        });
    }
    engine.register_fn("glob", |pattern: &str| -> RhaiResult<Array> {
        to_rhai(glob_paths(pattern)).map(|paths| paths.into_iter().map(Dynamic::from).collect())
    });

    // --- Environment -------------------------------------------------------
    {
        let ui = ui.clone();
        engine.register_fn("set_env", move |name: &str, value: &str| -> RhaiResult<()> {
            ui.step(format!("set_env {name}={value}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(crate::path::set_user_env(name, value))
        });
    }
    engine.register_fn("get_env", |name: &str| -> String {
        std::env::var(name).unwrap_or_default()
    });
    {
        let ui = ui.clone();
        engine.register_fn("unset_env", move |name: &str| -> RhaiResult<()> {
            ui.step(format!("unset_env {name}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            to_rhai(crate::path::unset_user_env(name))
        });
    }

    // --- JSON ----------------------------------------------------------------
    engine.register_fn("json_parse", |text: &str| -> RhaiResult<Dynamic> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| rhai_err(format!("invalid JSON: {e}")))?;
        rhai::serde::to_dynamic(value).map_err(|e| rhai_err(e.to_string()))
    });
    engine.register_fn("json_stringify", |value: Dynamic| -> RhaiResult<String> {
        let json: serde_json::Value =
            rhai::serde::from_dynamic(&value).map_err(|e| rhai_err(e.to_string()))?;
        serde_json::to_string(&json).map_err(|e| rhai_err(e.to_string()))
    });
    engine.register_fn("json_merge", |base: Dynamic, overlay: Dynamic| -> RhaiResult<Dynamic> {
        let mut base: serde_json::Value =
            rhai::serde::from_dynamic(&base).map_err(|e| rhai_err(e.to_string()))?;
        let overlay: serde_json::Value =
            rhai::serde::from_dynamic(&overlay).map_err(|e| rhai_err(e.to_string()))?;
        json_merge_values(&mut base, overlay);
        rhai::serde::to_dynamic(base).map_err(|e| rhai_err(e.to_string()))
    });

    // --- Archive --------------------------------------------------------------
    {
        let ui = ui.clone();
        engine.register_fn("extract", move |archive: &str, dest: &str| -> RhaiResult<()> {
            ui.step(format!("extract {archive} -> {dest}"));
            if ui.is_dry_run() {
                return Ok(());
            }
            let registry = ExtractorRegistry::new();
            to_rhai(registry.extract(Path::new(archive), Path::new(dest)))
        });
    }

    // --- Windows-specific -------------------------------------------------------
    {
        let ui = ui.clone();
        engine.register_fn("create_shortcut", move |target: &str, link_path: &str| -> RhaiResult<()> {
            ui.step(format!("create_shortcut {target} -> {link_path}"));
            Err(rhai_err(
                "create_shortcut() is not yet implemented in this version of LAST",
            ))
        });
    }
    {
        let ui = ui.clone();
        engine.register_fn(
            "create_shortcut_ex",
            move |target: &str, link_path: &str, _opts: Map| -> RhaiResult<()> {
                ui.step(format!("create_shortcut_ex {target} -> {link_path}"));
                Err(rhai_err(
                    "create_shortcut_ex() is not yet implemented in this version of LAST",
                ))
            },
        );
    }
    {
        let ui = ui.clone();
        engine.register_fn("register_filetype", move |ext: &str, _handler: &str| -> RhaiResult<()> {
            ui.step(format!("register_filetype {ext}"));
            Err(rhai_err(
                "register_filetype() is not yet implemented in this version of LAST",
            ))
        });
    }
}

fn copy_path(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn move_path(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_path(src, dst)?;
            delete_path(src)
        }
    }
}

fn delete_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// A minimal glob implementation supporting `*` and `?` wildcards in the
/// final path component, optionally recursive via a `**` directory segment.
fn glob_paths(pattern: &str) -> Result<Vec<String>> {
    let pattern = pattern.replace('/', "\\");
    let path = Path::new(&pattern);
    let file_pattern = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let recursive = parent.components().any(|c| c.as_os_str() == "**");
    let base: PathBuf = if recursive {
        parent
            .components()
            .take_while(|c| c.as_os_str() != "**")
            .collect()
    } else {
        parent.to_path_buf()
    };

    let mut results = Vec::new();
    if recursive {
        for entry in walkdir::WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if wildcard_match(&file_pattern, &name) {
                results.push(entry.path().display().to_string());
            }
        }
    } else if base.is_dir() {
        for entry in std::fs::read_dir(&base)?.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if wildcard_match(&file_pattern, &name) {
                results.push(entry.path().display().to_string());
            }
        }
    }
    results.sort();
    Ok(results)
}

/// Matches `name` against a pattern containing `*` (any sequence) and `?`
/// (any single character).
fn wildcard_match(pattern: &str, name: &str) -> bool {
    fn inner(pattern: &[char], name: &[char]) -> bool {
        match (pattern.first(), name.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                inner(&pattern[1..], name) || (!name.is_empty() && inner(pattern, &name[1..]))
            }
            (Some('?'), Some(_)) => inner(&pattern[1..], &name[1..]),
            (Some(p), Some(n)) if p.eq_ignore_ascii_case(n) => inner(&pattern[1..], &name[1..]),
            _ => false,
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    inner(&pattern, &name)
}

fn json_merge_values(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(existing) => json_merge_values(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}
