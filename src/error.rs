//! Domain-specific error types.
//!
//! Most errors are propagated as [`anyhow::Error`] with added context, but
//! a few well-known conditions are modeled explicitly so callers (and the
//! CLI's exit-code logic) can match on them.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LastError {
    #[error("package '{0}' not found in any registered bucket")]
    PackageNotFound(String),

    #[error("bucket '{0}' is not registered")]
    BucketNotFound(String),

    #[error("bucket '{0}' is already registered")]
    BucketAlreadyExists(String),

    #[error("app '{0}' is not installed")]
    AppNotInstalled(String),

    #[error("app '{0}' is already installed (version {1})")]
    AppAlreadyInstalled(String, String),

    #[error("no download URL available for architecture '{0}'")]
    NoUrlForArchitecture(String),

    #[error("unsupported archive format: {0}")]
    UnsupportedArchiveFormat(String),

    #[error("unknown configuration key: {0}")]
    UnknownConfigKey(String),
}
