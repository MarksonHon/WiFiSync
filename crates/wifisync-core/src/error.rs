use thiserror::Error;

/// Core layer errors. All of them are meant to be shown to the user; the daemon
/// returns them to LuCI and the CLI prints them as-is (English only).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("No wireless module detected on this device, the AP role is unavailable")]
    ApRequiresWifi,

    #[error("Unknown role identifier: {0}")]
    UnknownRole(String),

    #[error("Wi-Fi source `{source_name}` is unavailable: {reason}")]
    SourceUnavailable { source_name: String, reason: String },

    #[error(
        "Using custom Wi-Fi information while the Controller and the local AP are the same device would modify the local wireless configuration; explicit confirmation is required"
    )]
    LocalWifiChangeNotConfirmed,

    #[error("Device `{0}` has not passed admission; refusing distribution and writes")]
    NotAdmitted(String),

    #[error("Invalid configuration: {0}")]
    Invalid(String),

    #[error("Backup verification failed, refusing to restore: {0}")]
    BackupVerificationFailed(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
