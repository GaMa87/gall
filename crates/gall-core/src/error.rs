use std::path::PathBuf;

/// Errors surfaced by the core. Each variant carries enough context to show
/// a meaningful notification to the user.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{op} failed for '{path}': {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{0}' already exists")]
    AlreadyExists(PathBuf),
    #[error("'{0}' does not exist")]
    NotFound(PathBuf),
    #[error("invalid name '{0}'")]
    InvalidName(String),
    #[error("cannot {op}: '{dest}' is inside '{src}'")]
    IntoItself {
        op: &'static str,
        src: PathBuf,
        dest: PathBuf,
    },
    #[error("trash: {0}")]
    Trash(String),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("watcher: {0}")]
    Watch(#[from] notify::Error),
    #[error("operation cancelled")]
    Cancelled,
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    pub fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io {
            op,
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Extension trait to attach an operation name and path to `io::Result`.
pub trait IoContext<T> {
    fn ctx(self, op: &'static str, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn ctx(self, op: &'static str, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|e| CoreError::io(op, path, e))
    }
}
