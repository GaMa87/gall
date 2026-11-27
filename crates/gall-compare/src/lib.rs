//! The comparison and merge models that do not need a file system or an operating system, so that the
//! same code runs in the desktop application and in a browser worker (`wasm32-unknown-unknown`).
//! `gall-core` re-exports these modules and keeps its own API unchanged.

pub mod adapter;
pub mod lines;
pub mod merge;

/// Why portable work stopped. `gall-core` converts it into its own error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The caller asked to stop.
    Cancelled,
    /// A refusal with its reason.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => f.write_str("operation cancelled"),
            Error::Other(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
