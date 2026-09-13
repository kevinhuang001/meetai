//! 统一错误类型。所有 Tauri 命令都返回 `Result<T, AppError>`，
//! 前端拿到的是中文可读信息。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("音频设备错误：{0}")]
    Audio(String),

    #[error("模型错误：{0}")]
    Model(String),

    #[error("语音识别错误：{0}")]
    Asr(String),

    #[error("AI 接口错误：{0}")]
    Ai(String),

    #[error("会话错误：{0}")]
    Session(String),

    #[error("配置错误：{0}")]
    Config(String),

    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),

    #[error("数据格式错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn audio(msg: impl Into<String>) -> Self {
        Self::Audio(msg.into())
    }
    pub fn model(msg: impl Into<String>) -> Self {
        Self::Model(msg.into())
    }
    pub fn asr(msg: impl Into<String>) -> Self {
        Self::Asr(msg.into())
    }
    pub fn ai(msg: impl Into<String>) -> Self {
        Self::Ai(msg.into())
    }
    pub fn session(msg: impl Into<String>) -> Self {
        Self::Session(msg.into())
    }
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(value: anyhow::Error) -> Self {
        AppError::Other(value.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            AppError::Ai("请求超时，请检查网络或调大超时时间".into())
        } else if value.is_connect() {
            AppError::Ai(format!("无法连接到 AI 服务：{value}"))
        } else {
            AppError::Ai(value.to_string())
        }
    }
}

// Tauri 要求命令的错误类型可序列化，直接转成字符串，前端好展示。
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
