# LAST – Package Manager Specification
## "The last package manager you'll ever need. Because once is enough."

Version: 0.1.0-draft  
Status: Draft

---

## 1. Overview

LAST is a portable, self-hosted-first package manager for Windows written in Rust.
It is fully compatible with the Scoop manifest format (JSON) and designed
for environments where full control over package sources and deployment is required.

### Design Principles

- **Self-hosted-first** – no internet connectivity assumed or required
- **Portable** – runs from any path without installation
- **Scoop-compatible** – existing Scoop manifests work without modification
- **Single binary** – no runtime dependencies, no installer required
- **Secure scripting** – sandboxed Rhai scripting instead of unrestricted PowerShell
- **Predictable** – no silent failures, no automatic state changes without explicit user action

---

## 2. CLI Interface

```
last install <package>           Install a package
last install <package>@<version> Install a specific version
last update *                    Update all installed packages
last update <package>            Update a specific package
last remove <package>            Remove a package
last search <query>              Search available packages
last info <package>              Show package details and target paths
last list                        List installed packages
last export <package> <dest>     Copy installed package to destination path
last mirror <package>            Mirror a public package into the local bucket
last bucket add <name> <url>     Register a bucket (ZIP or local path)
last bucket rm <name>            Remove a bucket
last bucket list                 List registered buckets
last bucket update               Update all registered buckets
last config set <key> <value>    Set a configuration value
last config get <key>            Get a configuration value
last config list                 Show all configuration values
last checkup                     Verify installation integrity
last migrate --from-scoop        Import existing Scoop installation
```

### Global Flags

```
--dry-run         Show what would be done without making changes
--verbose / -v    Enable debug output
--config <path>   Use alternative config file
--arch <arch>     Override architecture (64bit | 32bit | arm64)
```

---

## 3. Architecture

### 3.1 Directory Layout

LAST follows the same directory layout as Scoop to ensure manifest compatibility:

```
%LAST_ROOT%\                        # configurable via LAST env var
  apps\
    <appname>\
      <version>\                    # actual installation
      current\                      # junction link → <version>
  buckets\
    <bucketname>\
      bucket\
        <app>.json
  cache\                            # downloaded binaries
  persist\
    <appname>\                      # user data surviving updates
  shims\                            # executable stubs
  config.json                       # LAST configuration
```

LAST_ROOT defaults to `%USERPROFILE%\last` but can be set via:
- Environment variable `LAST`
- Config file
- CLI flag `--root <path>`

This allows LAST to run from any path without prior installation.

### 3.2 PATH Management

LAST adds only a single entry to the user PATH: `%LAST_ROOT%\shims`.
All tool shims are placed in this directory.

For session-scoped usage (e.g. switching between environments):

```powershell
$env:LAST = "D:\last"
$env:PATH = "D:\last\shims;$env:PATH"
```

---

## 4. Manifest Format

### 4.1 Scoop Compatibility

LAST is 100% compatible with the Scoop manifest JSON format. All existing
Scoop manifests can be used without modification.

Supported Scoop fields:

```
version         description     homepage        license
url             hash            extract_dir     bin
shortcuts       persist         env_set         env_add_path
depends         architecture    installer       pre_install
post_install    uninstaller     pre_uninstall   post_uninstall
notes           suggest
```

Fields silently ignored (Scoop-specific, not applicable to LAST):
```
autoupdate      checkver
```

### 4.2 URL Support

All URL schemes supported by Scoop are supported by LAST:

- `https://` and `http://`
- `ftp://`
- UNC paths: `\\server\share\file.zip`
- Local paths: `C:\software\file.zip`
- Scoop rename trick: `https://host/file.exe#/renamed.7z`

### 4.3 Supported Archive Formats

```
.zip    .7z     .tar    .tar.gz     .tar.bz2
.tar.xz .gz     .lzma   .lzh        .rar
.msi    (extracted without running the installer)
.exe    (portable, not installed – use installer.script for actual installers)
```

### 4.4 Architecture Support

Supported architecture keys: `64bit`, `32bit`, `arm64`

LAST detects the current architecture automatically and selects the
appropriate URL from the `architecture` block. Can be overridden with
`--arch`.

### 4.5 Hash Verification

All downloads are verified before installation. Supported algorithms:

```
sha256 (default)    sha512    sha1    md5
```

Format: `<algo>:<hexdigest>` or bare hexdigest (assumes sha256).
Installation is aborted on hash mismatch with a clear error message.
Hash verification is mandatory and cannot be disabled.

---

## 5. Scripting

### 5.1 Script Engine: Rhai

LAST uses [Rhai](https://rhai.rs) as its embedded scripting engine.
Rhai scripts replace PowerShell in Scoop's `pre_install`, `post_install`,
`installer.script`, `uninstaller.script`, `pre_uninstall`, and
`post_uninstall` fields.

Rhai was chosen because:
- Written in Rust, zero additional runtime dependencies
- Sandboxed by design – no filesystem or network access unless explicitly exposed
- Clean, readable syntax similar to JavaScript/Rust
- Fast and lightweight

### 5.2 Sandboxing

Scripts have access only to the API explicitly exposed by LAST.
No arbitrary OS calls, no process spawning, no registry access outside
the provided API functions.

### 5.3 Available Script Variables

The following variables are available in all script contexts:

```
dir             # installation directory (current version)
persist_dir     # persist directory for this app
version         # current version string
app             # app name
architecture    # current architecture string (64bit/32bit/arm64)
scoopdir        # LAST root directory (alias for Scoop compatibility)
last_root       # LAST root directory
global          # bool: true if installed globally
```

### 5.4 Available Script Functions

```rhai
// Logging
log("message")              // info
warn("message")             // warning
error("message")            // error (does not abort)
success("message")          // success

// Filesystem
copy(src, dst)              // copy file or directory
copy_dir(src, dst)          // copy directory recursively
move_file(src, dst)         // move file
delete(path)                // delete file or directory
mkdir(path)                 // create directory (recursive)
exists(path)                // returns bool
read_file(path)             // returns string
write_file(path, content)   // write string to file
rename(src, dst)            // rename file or directory
glob(pattern)               // returns list of matching paths

// Environment
set_env(name, value)        // set user environment variable
get_env(name)               // get environment variable
unset_env(name)             // remove environment variable

// JSON
json_parse(string)          // parse JSON string → map
json_stringify(value)       // serialize value → JSON string
json_merge(base, overlay)   // deep merge two maps

// Archive
extract(archive, dest)      // extract archive to directory

// Windows-specific
create_shortcut(target, link_path)              // create .lnk file
create_shortcut_ex(target, link_path, opts)     // with icon, working dir, args
register_filetype(ext, handler)                 // register file association
```

### 5.5 Scoop PowerShell Compatibility

For backwards compatibility with existing Scoop manifests that use
PowerShell scripts, LAST provides a PowerShell execution mode.

If a script field contains PowerShell syntax (detected heuristically or
via explicit `#!powershell` marker), LAST passes it to PowerShell for
execution. This requires PowerShell to be available on the system.

PowerShell compatibility mode is opt-in and can be disabled in config:

```json
{ "powershell_compat": false }
```

### 5.6 Script Field Format

Scripts can be specified as:

**String** (single line or multiline):
```json
"post_install": "log(\"Installation complete\");"
```

**Array of strings** (Scoop-compatible, joined with newlines):
```json
"post_install": [
    "mkdir(persist_dir + \"\\\\config\");",
    "copy(dir + \"\\\\default.cfg\", persist_dir + \"\\\\config\\\\tool.cfg\");",
    "log(\"Done.\");"
]
```

**External script file** (LAST extension, not in Scoop):
```json
"post_install": { "script_file": "setup.rhai" }
```

---

## 6. Bucket Format

### 6.1 Overview

A LAST bucket is a collection of manifest JSON files. LAST supports two
distribution formats:

**Format A: ZIP archive** (recommended for self-hosted deployments)
**Format B: Local directory** (for development and testing)

Git repositories are intentionally NOT a first-class bucket format in LAST.
Git integration is a concern of the CI/CD pipeline that produces the bucket,
not of the package manager itself. This eliminates authentication complexity
in self-hosted environments.

### 6.2 ZIP Bucket Format

```
my-bucket.zip
  bucket\
    vscodium.json
    git.json
    mytool-1.0.json
    mytool-2.0.json
    ...
  last-bucket.json          # bucket metadata (optional)
```

`last-bucket.json` (optional metadata):
```json
{
    "name": "my-bucket",
    "description": "Internal tools bucket",
    "version": "2024.06.01",
    "maintainer": "team@org.intern"
}
```

### 6.3 Bucket Registration

```
last bucket add internal \\srv\buckets\my-bucket.zip
last bucket add internal C:\repos\my-bucket\bucket
last bucket add internal http://server.intern/buckets/my-bucket.zip
```

### 6.4 Bucket Updates

```
last bucket update
```

For ZIP buckets: re-downloads the ZIP from the registered URL and replaces
the local copy. The URL is stored at registration time.

For local directory buckets: no-op (directory is read directly).

### 6.5 Recommended CI/CD Pipeline

```
Developer edits manifest JSON
        ↓
git commit + push
        ↓
CI: zip bucket/ → my-bucket.zip
        ↓
Deploy ZIP to \\srv\buckets\my-bucket.zip
        ↓
last bucket update    ← on workstations
```

This separates the development workflow (Git) from the deployment artifact
(ZIP), without requiring Git authentication on target machines.

### 6.6 Package Lookup Priority

When multiple buckets are registered, LAST searches in registration order.
Explicit bucket prefix is supported:

```
last install internal/vscodium
last install extras/git
```

---

## 7. Mirror Command

### 7.1 Overview

`last mirror` imports a package from a public Scoop bucket (main or extras)
into the local bucket working copy and downloads the binaries to a
configured share. This is a first-class workflow for self-hosted environments
that need to replicate public packages internally.

### 7.2 Usage

```
last mirror <app>                        Mirror from extras (default)
last mirror <app> --source main          Mirror from main bucket
last mirror <app> --arch 64bit arm64     Specific architectures only
last mirror <app> --vendor "Microsoft"   Override vendor name
last mirror <app> --app-name "VSCode"    Override app name for share path
last mirror <app> --dry-run              Show what would happen
last mirror <app> --list-only            Show manifest info and target paths
last mirror <app> --skip-download        Only rewrite manifest, skip download
```

### 7.3 What mirror does

1. Fetches the public manifest from the Scoop GitHub bucket
2. Downloads all binaries for the requested architectures
3. Verifies hashes before copying to the share
4. Copies binaries to the configured share using the path layout:
   `<share>\Windows\<Vendor>\<AppName> <Version>\<arch>\<file>`
5. Rewrites all URLs in the manifest to point to the internal share
6. Removes `autoupdate` and `checkver` sections (not meaningful for internal mirrors)
7. Removes architecture blocks that were not mirrored
8. Writes the rewritten manifest to `bucket\<app>.json` in the local working copy

### 7.4 Share Path Layout

Binaries are stored on the share using this layout:

```
<share>\
  Windows\
    <Vendor>\
      <AppName> <Version>\
        x64\<file>
        x86\<file>
        arm64\<file>
```

Vendor is auto-detected from the manifest homepage URL and can be overridden
with `--vendor`. App name defaults to the Scoop package name and can be
overridden with `--app-name`.

### 7.5 Configuration

Mirror-specific configuration in `config.json`:

```json
{
    "mirror": {
        "binary_share": "\\\\srv\\software",
        "architectures": ["64bit", "arm64"],
        "download_proxy": "",
        "bucket_path": "C:\\repos\\my-bucket"
    }
}
```

`bucket_path` points to the local working copy of the bucket repository.
The `bucket\` subdirectory within it is where manifests are written.

### 7.6 Workflow

```
# One-time: configure mirror settings
last config set mirror.binary_share \\srv\software
last config set mirror.bucket_path C:\repos\my-bucket

# Preview without downloading
last mirror vscodium --source extras --list-only

# Mirror VSCodium (64bit + arm64)
last mirror vscodium --source extras --arch 64bit arm64 \
    --vendor "VSCodium" --app-name "VSCodium"

# Review and commit manually
git -C C:\repos\my-bucket diff bucket/
git -C C:\repos\my-bucket add bucket/vscodium.json
git -C C:\repos\my-bucket commit -m "mirror: vscodium 1.121.0"
```

Git operations are intentionally left to the user – LAST never commits or
pushes automatically.

---

## 8. Persist

The persist mechanism works identically to Scoop:

- Files and directories listed in `persist` are moved to `%LAST_ROOT%\persist\<app>\`
  on first install
- A junction (directory) or hard link (file) is created in the app directory
- On update, persist data is preserved and re-linked to the new version
- On uninstall, persist data is NOT deleted by default (use `--purge` to remove)

```json
"persist": [
    "config.json",
    "profiles",
    "data"
]
```

---

## 9. Shims

LAST generates shims for all entries in the `bin` field.
A shim is a small native executable (generated at install time) that
forwards execution to the actual binary.

Shims support:
- `.exe` binaries
- `.ps1` PowerShell scripts (via `powershell.exe -file`)
- `.cmd` / `.bat` scripts
- Alias names: `["real.exe", "alias-name"]`
- Additional arguments: `["real.exe", "alias", "--default-flag"]`

---

## 10. Export

The `export` command is a LAST-native feature not present in Scoop.
It copies an installed package (app directory + persist data) to an
arbitrary destination path, enabling deployment to portable locations.

```
last export mytool D:\portable\tools
last export vscodium E:\tools --include-persist
```

Options:
```
--include-persist   Also copy persist directory contents
--overwrite         Overwrite existing files (default: backup existing)
--backup-suffix     Suffix for backup copies (default: .bak.<timestamp>)
```

---

## 11. Configuration

Configuration is stored in `%LAST_ROOT%\config.json`.

```json
{
    "architectures": ["64bit", "arm64"],
    "download_proxy": "",
    "powershell_compat": true,
    "last_update": "2024-06-01T12:00:00",
    "mirror": {
        "binary_share": "\\\\srv\\software",
        "architectures": ["64bit", "arm64"],
        "download_proxy": "",
        "bucket_path": "C:\\repos\\my-bucket"
    }
}
```

| Key | Default | Description |
|---|---|---|
| `architectures` | `["64bit"]` | Default architectures to install |
| `download_proxy` | – | HTTP proxy for downloads |
| `powershell_compat` | `true` | Allow PowerShell script execution |
| `mirror.binary_share` | – | Share path for mirrored binaries |
| `mirror.bucket_path` | – | Local bucket working copy path |
| `mirror.architectures` | `["64bit"]` | Default architectures to mirror |

---

## 12. Environment Variables

| Variable | Description |
|---|---|
| `LAST` | LAST root directory (overrides default) |
| `LAST_ARCH` | Default architecture override |
| `LAST_PROXY` | Download proxy |

---

## 13. Logging

Every install, update, remove and mirror operation is logged with:
- Timestamp
- Operation type
- Package name and version
- Hash values of downloaded files
- Result (success / failure)

Log stored in `%LAST_ROOT%\log\last.log`.

---

## 14. Rust Crate Structure

```
last/
  Cargo.toml
  src/
    main.rs             # CLI entry point (clap)
    config.rs           # Configuration management
    manifest.rs         # Manifest parsing and validation
    bucket.rs           # Bucket management (ZIP / local)
    install.rs          # Install / update / remove logic
    download.rs         # Download with hash verification
    extract.rs          # Archive extraction
    persist.rs          # Persist link management
    shim.rs             # Shim generation
    export.rs           # Export to portable destination
    mirror.rs           # Mirror public packages to internal share
    script/
      mod.rs            # Script engine abstraction
      rhai.rs           # Rhai engine + API registration
      powershell.rs     # PowerShell compatibility layer
    hash.rs             # Hash verification (sha256/sha512/sha1/md5)
    path.rs             # Path helpers (UNC, junction, hardlink)
    ui.rs               # Output formatting (log/warn/error/success)
    log.rs              # Operation logging
```

### Key Dependencies

```toml
[dependencies]
clap        = { version = "4", features = ["derive"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
rhai        = "1"
reqwest     = { version = "0.12", features = ["blocking"] }
zip         = "2"
sevenz-rust = "0.6"         # 7z support
sha2        = "0.10"
sha1        = "0.10"
md5         = "0.7"
which       = "6"            # shim resolution
junction    = "1"            # Windows junctions
walkdir     = "2"
anyhow      = "1"
```

---

## 15. Scoop Migration

LAST can read an existing Scoop installation and import its state:

```
last migrate --from-scoop
last migrate --from-scoop --scoop-root C:\Users\user\scoop
```

This registers installed apps and their versions in LAST's state without
re-downloading or re-installing them.

---

## 16. Out of Scope (v1.0)

- GUI / TUI
- Package signing / GPG verification (planned for v2.0)
- Linux / macOS support
- Building packages from source (this is a deployment tool, not a build tool)
- Automatic update scheduling
