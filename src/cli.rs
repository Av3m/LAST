//! Command-line interface: argument parsing and command dispatch.
//!
//! [`run`] is the single entry point called from `main`. It resolves the
//! LAST root and configuration, builds the shared [`Ui`], and dispatches to
//! the appropriate manager (`InstallManager`, `Exporter`, `Mirror`,
//! `Migrator`, ...) for each subcommand.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use crate::bucket::{BucketManager, ZipBucket};
use crate::config::{self, BucketSource, Config};
use crate::export::{ExportOptions, Exporter};
use crate::install::{parse_package_spec, InstallManager};
use crate::manifest::Architecture;
use crate::migrate::Migrator;
use crate::mirror::{Mirror, MirrorOptions, MirrorSource};
use crate::path as last_path;
use crate::ui::Ui;

#[derive(Parser)]
#[command(name = "last", version, about = "LAST - The last package manager you'll ever need.")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Show what would be done without making any changes.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Print additional debug information.
    #[arg(short = 'v', long = "verbose", global = true)]
    verbose: bool,

    /// LAST root directory (overrides the `LAST` environment variable).
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Path to an alternative `config.json`.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Target architecture(s): `64bit`, `32bit` or `arm64`. The first value
    /// is used as an override for `install`/`update`/`remove`; `mirror`
    /// uses all given values.
    #[arg(long, global = true, num_args = 1..)]
    arch: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Install a package (`<package>` or `<bucket>/<package>`, optionally
    /// `@<version>`).
    Install { package: String },

    /// Update a package, or all installed packages if omitted or `*`.
    Update { package: Option<String> },

    /// Remove an installed package.
    Remove {
        package: String,
        /// Also delete persisted user data.
        #[arg(long)]
        purge: bool,
    },

    /// Search registered buckets for packages matching `query`.
    Search { query: String },

    /// Show information about a package.
    Info { package: String },

    /// List installed packages.
    List,

    /// Export an installed package to a portable directory.
    Export {
        package: String,
        dest: PathBuf,
        /// Also copy persisted user data.
        #[arg(long)]
        include_persist: bool,
        /// Overwrite existing files at the destination instead of backing them up.
        #[arg(long)]
        overwrite: bool,
        /// Suffix used for backups of existing files (default: `.bak.<timestamp>`).
        #[arg(long)]
        backup_suffix: Option<String>,
    },

    /// Mirror a package from a public Scoop bucket into the local bucket and binary share.
    Mirror {
        package: String,
        /// Source bucket to mirror from (`main` or `extras`).
        #[arg(long, default_value = "extras")]
        source: String,
        /// Override the detected vendor directory name.
        #[arg(long)]
        vendor: Option<String>,
        /// Override the app name used in the binary share path.
        #[arg(long = "app-name")]
        app_name: Option<String>,
        /// Print the mirror plan without downloading or writing anything.
        #[arg(long)]
        list_only: bool,
        /// Rewrite the manifest without (re-)downloading the binaries.
        #[arg(long)]
        skip_download: bool,
    },

    /// Manage registered buckets.
    Bucket {
        #[command(subcommand)]
        command: BucketCommand,
    },

    /// Manage configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Verify the LAST installation and report any issues.
    Checkup,

    /// Migrate an existing installation.
    Migrate {
        /// Register apps from an existing Scoop installation without re-downloading them.
        #[arg(long = "from-scoop")]
        from_scoop: bool,
        /// Path to the Scoop installation (default: `%USERPROFILE%\scoop`).
        #[arg(long = "scoop-root")]
        scoop_root: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BucketCommand {
    /// Register a bucket. `source` is a local/UNC directory, or a path/URL to a `.zip` archive.
    Add { name: String, source: String },
    /// Unregister a bucket.
    Rm { name: String },
    /// List registered buckets.
    List,
    /// Re-fetch all ZIP buckets.
    Update,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set a configuration value (dotted key path, e.g. `mirror.binary_share`).
    Set { key: String, value: String },
    /// Get a configuration value.
    Get { key: String },
    /// List all configuration values.
    List,
}

/// Parses arguments and dispatches to the requested command.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let ui = Arc::new(Ui::new(cli.verbose, cli.dry_run));

    let root = cli
        .root
        .clone()
        .unwrap_or_else(last_path::env_or_default_root);

    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(|| config::config_path(&root));
    let mut config = Config::load(&config_path)?;

    let arch_override = match cli.arch.first() {
        Some(a) => Some(Architecture::parse(a)?),
        None => None,
    };

    match &cli.command {
        Command::Install { package } => {
            let buckets = BucketManager::load(&root, &config);
            let manager = InstallManager::new(root.clone(), &config, &buckets, ui.clone())?;
            let (query, version) = parse_package_spec(package);
            manager.install(query, version, arch_override)?;
        }

        Command::Update { package } => {
            let buckets = BucketManager::load(&root, &config);
            let manager = InstallManager::new(root.clone(), &config, &buckets, ui.clone())?;
            let app = match package.as_deref() {
                None | Some("*") => None,
                Some(app) => Some(app),
            };
            manager.update(app, arch_override)?;
        }

        Command::Remove { package, purge } => {
            let buckets = BucketManager::load(&root, &config);
            let manager = InstallManager::new(root.clone(), &config, &buckets, ui.clone())?;
            manager.remove(package, *purge)?;
        }

        Command::Search { query } => {
            let buckets = BucketManager::load(&root, &config);
            let results = buckets.search(query)?;
            if results.is_empty() {
                ui.info(format!("no packages found matching '{query}'"));
            }
            for (bucket_name, manifest) in results {
                let description = manifest.description.as_deref().unwrap_or("");
                ui.info(format!(
                    "{bucket_name}/{} {} - {description}",
                    manifest.name, manifest.version
                ));
            }
        }

        Command::Info { package } => {
            let buckets = BucketManager::load(&root, &config);
            let manager = InstallManager::new(root.clone(), &config, &buckets, ui.clone())?;
            let (manifest, state) = manager.info(package)?;

            ui.info(format!("Name:          {}", manifest.name));
            ui.info(format!("Version:       {}", manifest.version));
            if let Some(description) = &manifest.description {
                ui.info(format!("Description:   {description}"));
            }
            if let Some(homepage) = &manifest.homepage {
                ui.info(format!("Homepage:      {homepage}"));
            }
            ui.info(format!(
                "Architectures: {}",
                manifest.available_architectures().join(", ")
            ));
            let deps = manifest.depends();
            if !deps.is_empty() {
                ui.info(format!("Depends on:    {}", deps.join(", ")));
            }
            for field in manifest.ignored_fields() {
                ui.debug(format!("ignoring unsupported field '{field}'"));
            }

            match state {
                Some(state) => {
                    ui.info(format!(
                        "Installed:     {} ({}, {})",
                        state.version, state.bucket, state.architecture
                    ));
                    ui.info(format!(
                        "Location:      {}",
                        manager.current_dir_for(&manifest.name).display()
                    ));
                    if !state.persist.is_empty() {
                        ui.info(format!(
                            "Persisted at:  {}",
                            manager.persist_dir_for(&manifest.name).display()
                        ));
                    }
                }
                None => ui.info("Installed:     no"),
            }
        }

        Command::List => {
            let buckets = BucketManager::load(&root, &config);
            let manager = InstallManager::new(root.clone(), &config, &buckets, ui.clone())?;
            let apps = manager.list()?;
            if apps.is_empty() {
                ui.info("no packages installed");
            }
            for app in apps {
                ui.info(format!(
                    "{} {} ({}, {})",
                    app.name, app.version, app.bucket, app.architecture
                ));
            }
        }

        Command::Export {
            package,
            dest,
            include_persist,
            overwrite,
            backup_suffix,
        } => {
            let exporter = Exporter::new(root.clone(), ui.clone());
            let options = ExportOptions {
                include_persist: *include_persist,
                overwrite: *overwrite,
                backup_suffix: backup_suffix.clone(),
            };
            exporter.export(package, dest, &options)?;
        }

        Command::Mirror {
            package,
            source,
            vendor,
            app_name,
            list_only,
            skip_download,
        } => {
            let source = MirrorSource::parse(source)?;
            let architectures: Result<Vec<Architecture>> =
                cli.arch.iter().map(|a| Architecture::parse(a)).collect();
            let options = MirrorOptions {
                source,
                architectures: architectures?,
                vendor: vendor.clone(),
                app_name: app_name.clone(),
                list_only: *list_only,
                skip_download: *skip_download,
            };
            let mirror = Mirror::new(&config.mirror, ui.clone())?;
            mirror.run(package, &options)?;
        }

        Command::Bucket { command } => match command {
            BucketCommand::Add { name, source } => {
                let bucket_source = detect_bucket_source(source);
                config.add_bucket(name, bucket_source.clone())?;

                if let BucketSource::Zip { url } = &bucket_source {
                    let buckets_root = root.join("buckets");
                    let zip = ZipBucket::new(name.clone(), url.clone(), &buckets_root);
                    ui.step(format!("fetching bucket '{name}'"));
                    if !ui.is_dry_run() {
                        zip.fetch()?;
                    }
                }

                if !ui.is_dry_run() {
                    config.save(&config_path)?;
                }
                ui.success(format!("added bucket '{name}'"));
            }
            BucketCommand::Rm { name } => {
                config.remove_bucket(name)?;
                if !ui.is_dry_run() {
                    config.save(&config_path)?;
                }
                ui.success(format!("removed bucket '{name}'"));
            }
            BucketCommand::List => {
                if config.bucket_order.is_empty() {
                    ui.info("no buckets registered");
                }
                for (name, entry) in config.ordered_buckets() {
                    match &entry.source {
                        BucketSource::Local { path } => ui.info(format!("{name}  local  {path}")),
                        BucketSource::Zip { url } => ui.info(format!("{name}  zip    {url}")),
                    }
                }
            }
            BucketCommand::Update => {
                let buckets = BucketManager::load(&root, &config);
                let updated = buckets.update_all()?;
                if updated.is_empty() {
                    ui.info("no buckets registered");
                } else {
                    ui.success(format!("updated {} bucket(s): {}", updated.len(), updated.join(", ")));
                }
            }
        },

        Command::Config { command } => match command {
            ConfigCommand::Set { key, value } => {
                config.set(key, value)?;
                if !ui.is_dry_run() {
                    config.save(&config_path)?;
                }
                ui.success(format!("{key} = {value}"));
            }
            ConfigCommand::Get { key } => {
                let value = config.get(key)?;
                ui.info(format!("{key} = {value}"));
            }
            ConfigCommand::List => {
                for (key, value) in config.list() {
                    ui.info(format!("{key} = {value}"));
                }
            }
        },

        Command::Checkup => {
            let buckets = BucketManager::load(&root, &config);
            run_checkup(&root, &config, &buckets, ui.clone())?;
        }

        Command::Migrate { from_scoop, scoop_root } => {
            if !*from_scoop {
                bail!("only 'last migrate --from-scoop' is currently supported");
            }
            let scoop_root = scoop_root.clone().unwrap_or_else(|| {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("scoop")
            });
            let migrator = Migrator::new(root.clone(), scoop_root, ui.clone());
            let migrated = migrator.migrate()?;
            if migrated.is_empty() {
                ui.info("no apps migrated");
            } else {
                ui.success(format!("migrated {} app(s): {}", migrated.len(), migrated.join(", ")));
            }
        }
    }

    Ok(())
}

/// Determines whether a bucket registration `source` refers to a ZIP archive
/// (HTTP(S)/UNC/local path ending in `.zip`) or a local directory.
fn detect_bucket_source(source: &str) -> BucketSource {
    if source.to_ascii_lowercase().ends_with(".zip") {
        BucketSource::Zip { url: source.to_string() }
    } else {
        BucketSource::Local { path: source.to_string() }
    }
}

/// `last checkup` - verifies the LAST root layout, registered buckets and
/// installed apps, reporting any issues found.
fn run_checkup(root: &Path, config: &Config, buckets: &BucketManager, ui: Arc<Ui>) -> Result<()> {
    ui.info(format!("LAST root: {}", root.display()));

    for dir in ["apps", "buckets", "cache", "persist", "shims", "log"] {
        let path = root.join(dir);
        if path.is_dir() {
            ui.success(format!("  ok      {dir}\\"));
        } else {
            ui.warn(format!("  missing {dir}\\"));
        }
    }

    if config.bucket_order.is_empty() {
        ui.warn("no buckets registered");
    }
    for bucket in buckets.buckets() {
        let dir = bucket.manifest_dir();
        if dir.is_dir() {
            let count = bucket.list_apps().map(|a| a.len()).unwrap_or(0);
            ui.success(format!(
                "  ok      bucket '{}' ({count} packages) -> {}",
                bucket.name(),
                dir.display()
            ));
        } else {
            ui.warn(format!(
                "  missing bucket '{}' manifest directory: {}",
                bucket.name(),
                dir.display()
            ));
        }
    }

    let manager = InstallManager::new(root.to_path_buf(), config, buckets, ui.clone())?;
    let apps = manager.list()?;
    if apps.is_empty() {
        ui.info("no packages installed");
    }
    for app in &apps {
        let current = manager.current_dir_for(&app.name);
        if std::fs::canonicalize(&current).is_ok() {
            ui.success(format!(
                "  ok      {} {} ({}, {})",
                app.name, app.version, app.bucket, app.architecture
            ));
        } else {
            ui.warn(format!(
                "  broken  {} {}: 'current' link is invalid",
                app.name, app.version
            ));
        }
    }

    Ok(())
}
