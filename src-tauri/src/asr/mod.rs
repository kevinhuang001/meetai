//! 语音识别：可插拔的 HTTP 服务 + 流式文本合并。
//!
//! 本模块**不包含任何识别模型**，App 通过 HTTP 调用识别服务：
//!   * OpenAI 兼容 `/audio/transcriptions`（OpenAI / Groq / 硅基流动 / faster-whisper-server）
//!   * whisper.cpp server `/inference`
//!
//! 想完全离线也很简单：本地起一个 whisper.cpp server 或 faster-whisper-server，
//! 把 Base URL 指向 `http://localhost:端口` 即可，App 侧代码完全一样。

pub mod client;
pub mod hypothesis;
pub mod provider;

pub use client::{AsrClient, AsrConnectionTest, AsrOutcome, TranscribeRequest};
pub use hypothesis::{Agreement, HypothesisBuffer};
pub use provider::{presets, provider_from_preset, AsrPreset};

/// 文本层的幻听/退化输出过滤（与具体识别服务无关）
pub mod filter;
pub use filter::{is_suspect, suspect_reason};
