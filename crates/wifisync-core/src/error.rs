use thiserror::Error;

/// 核心层错误。全部为「可解释给用户看」的错误，daemon 会原样返回给 LuCI。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("本设备未检测到无线模块，AP 角色不可用")]
    ApRequiresWifi,

    #[error("未知角色标识: {0}")]
    UnknownRole(String),

    #[error("Wi-Fi 信息源 `{source_name}` 不可用: {reason}")]
    SourceUnavailable { source_name: String, reason: String },

    #[error(
        "在控制器与本机 AP 同设备的情况下使用自定义 Wi-Fi 信息会修改本机无线配置，需要显式确认"
    )]
    LocalWifiChangeNotConfirmed,

    #[error("设备 `{0}` 未通过准入，拒绝下发与写入")]
    NotAdmitted(String),

    #[error("配置无效: {0}")]
    Invalid(String),

    #[error("备份校验失败，拒绝恢复: {0}")]
    BackupVerificationFailed(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
