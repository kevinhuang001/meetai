//! AI 层：OpenAI 兼容接口客户端 + 提示词 + 滚动纪要状态机。

pub mod client;
pub mod presets;
pub mod prompts;
pub mod summarizer;

pub use client::{AiClient, ChatMessage, ChatOutcome, ConnectionTest, TokenUsage};
pub use presets::{presets, provider_from_preset, AiPreset};
pub use summarizer::{ActionItem, SummaryPatch, SummaryState};
