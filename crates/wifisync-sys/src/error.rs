use thiserror::Error;

#[derive(Debug, Error)]
pub enum SysError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Running `{program}` failed: {message}")]
    Command { program: String, message: String },

    #[error("Executable `{0}` not found")]
    MissingProgram(String),

    #[error("Failed to parse `{what}`: {message}")]
    Parse { what: String, message: String },

    #[error(
        "Initial baseline backup not found ({0}): refusing to write, create the baseline first"
    )]
    BaseLineMissing(String),

    #[error("core error: {0}")]
    Core(#[from] wifisync_core::CoreError),
}

pub type SysResult<T> = Result<T, SysError>;
