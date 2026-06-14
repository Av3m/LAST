mod bucket;
mod cli;
mod config;
mod download;
mod error;
mod export;
mod extract;
mod hash;
mod install;
mod log;
mod manifest;
mod migrate;
mod mirror;
mod path;
mod persist;
mod script;
mod shim;
mod ui;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("\x1b[31m[error]\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
