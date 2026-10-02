use thiserror::Error;

#[derive(Debug, Error)]
pub enum SysError {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("执行 `{program}` 失败: {message}")]
    Command { program: String, message: String },

    #[error("找不到可执行文件 `{0}`")]
    MissingProgram(String),

    #[error("解析 `{what}` 失败: {message}")]
    Parse { what: String, message: String },

    #[error("找不到初始基线备份（{0}）：拒绝写入，请先成功建立基线")]
    BaseLineMissing(String),

    #[error("core 错误: {0}")]
    Core(#[from] wifisync_core::CoreError),
}

pub type SysResult<T> = Result<T, SysError>;
