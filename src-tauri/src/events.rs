//! 后端 → 前端的事件。
//!
//! 事件名与前端 `src/lib/contract.ts` 的 `EV` 常量一一对应。
//! 为了便于无界面（headless）测试，这里定义了 [`Emitter`] 抽象：
//! 生产环境用 [`TauriEmitter`] 发到 WebView，测试里用收集器即可完整断言整条链路。

use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::audio::capture::SourceKind;
use crate::session::{SessionInfo, SessionStats, SummaryState, TranscriptSegment};

/* ==========================================================================
 * 事件名
 * ========================================================================== */

pub mod event {
    pub const SEGMENT: &str = "asr:segment";
    pub const PARTIAL: &str = "asr:partial";
    pub const STATE: &str = "asr:state";
    pub const LEVEL: &str = "asr:level";
    pub const AI_STATUS: &str = "ai:status";
    pub const SUMMARY: &str = "ai:summary";
    pub const SESSION_UPDATED: &str = "session:updated";
    pub const ERROR: &str = "app:error";
}

/* ==========================================================================
 * 载荷
 * ========================================================================== */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AsrStateKind {
    Idle,
    Starting,
    Listening,
    Speech,
    Paused,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrStateEvent {
    pub session_id: String,
    pub state: AsrStateKind,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSegmentEvent {
    pub session_id: String,
    pub segment: TranscriptSegment,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrPartialEvent {
    pub session_id: String,
    /// 已被两次识别共同确认的部分
    pub committed: String,
    /// 仍未确认的尾巴
    pub tentative: String,
    pub start_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelItem {
    pub source_id: String,
    pub kind: SourceKind,
    /// 0~1，已做感知映射，可直接当 UI 比例用
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrLevelEvent {
    pub session_id: String,
    pub levels: Vec<LevelItem>,
    pub stats: SessionStats,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiStateKind {
    Idle,
    Thinking,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatusEvent {
    pub session_id: String,
    pub state: AiStateKind,
    pub message: Option<String>,
    pub calls: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSummaryEvent {
    pub session_id: String,
    pub summary: SummaryState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdatedEvent {
    pub session: SessionInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorEvent {
    pub scope: String,
    pub message: String,
}

/* ==========================================================================
 * Emitter
 * ========================================================================== */

pub trait Emitter: Send + Sync + 'static {
    fn emit_value(&self, event: &str, payload: Value);
}

impl<T: Emitter + ?Sized> Emitter for Arc<T> {
    fn emit_value(&self, event: &str, payload: Value) {
        (**self).emit_value(event, payload);
    }
}

/// 统一发送入口：序列化失败只记日志，绝不因为一个事件把录音打断
pub fn emit<E: Emitter, P: Serialize>(emitter: &E, event: &str, payload: &P) {
    match serde_json::to_value(payload) {
        Ok(v) => emitter.emit_value(event, v),
        Err(e) => tracing::error!("事件 {event} 序列化失败：{e}"),
    }
}

/// Tauri 实现
pub struct TauriEmitter {
    app: tauri::AppHandle,
}

impl TauriEmitter {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl Emitter for TauriEmitter {
    fn emit_value(&self, event: &str, payload: Value) {
        use tauri::Emitter as _;
        if let Err(e) = self.app.emit(event, payload) {
            tracing::warn!("发送事件 {event} 失败：{e}");
        }
    }
}

/// 测试用：把事件收集到内存里
#[derive(Clone, Default)]
pub struct CollectingEmitter {
    pub events: Arc<Mutex<Vec<(String, Value)>>>,
}

impl CollectingEmitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.events.lock().expect("事件锁被污染"))
    }

    /// 取出某个事件名的全部载荷
    pub fn payloads(&self, event: &str) -> Vec<Value> {
        self.events
            .lock()
            .expect("事件锁被污染")
            .iter()
            .filter(|(name, _)| name == event)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn has(&self, event: &str) -> bool {
        self.events
            .lock()
            .expect("事件锁被污染")
            .iter()
            .any(|(name, _)| name == event)
    }

    pub fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("事件锁被污染")
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

impl Emitter for CollectingEmitter {
    fn emit_value(&self, event: &str, payload: Value) {
        self.events
            .lock()
            .expect("事件锁被污染")
            .push((event.to_string(), payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Speaker;

    #[test]
    fn event_names_match_frontend_contract() {
        // 这些字符串必须与 src/lib/contract.ts 的 EV 完全一致
        assert_eq!(event::SEGMENT, "asr:segment");
        assert_eq!(event::PARTIAL, "asr:partial");
        assert_eq!(event::STATE, "asr:state");
        assert_eq!(event::LEVEL, "asr:level");
        assert_eq!(event::AI_STATUS, "ai:status");
        assert_eq!(event::SUMMARY, "ai:summary");
        assert_eq!(event::SESSION_UPDATED, "session:updated");
        assert_eq!(event::ERROR, "app:error");
    }

    #[test]
    fn asr_state_serializes_lowercase() {
        let e = AsrStateEvent {
            session_id: "s1".into(),
            state: AsrStateKind::Speech,
            message: None,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["state"], "speech");
        assert_eq!(v["sessionId"], "s1");
    }

    #[test]
    fn collector_records_events() {
        let c = CollectingEmitter::new();
        emit(
            &c,
            event::PARTIAL,
            &AsrPartialEvent {
                session_id: "s1".into(),
                committed: "你好".into(),
                tentative: "世界".into(),
                start_ms: 0,
            },
        );
        assert!(c.has(event::PARTIAL));
        let p = &c.payloads(event::PARTIAL)[0];
        assert_eq!(p["committed"], "你好");
        assert_eq!(p["tentative"], "世界");
    }

    #[test]
    fn segment_payload_shape_matches_contract() {
        let c = CollectingEmitter::new();
        emit(
            &c,
            event::SEGMENT,
            &AsrSegmentEvent {
                session_id: "s1".into(),
                segment: TranscriptSegment {
                    id: 7,
                    text: "测试".into(),
                    start_ms: 100,
                    end_ms: 900,
                    speaker: Speaker::Others,
                    suspect: None,
                    confidence: Some(0.9),
                },
            },
        );
        let p = &c.payloads(event::SEGMENT)[0];
        assert_eq!(p["segment"]["id"], 7);
        assert_eq!(p["segment"]["speaker"], "others");
        assert_eq!(p["segment"]["startMs"], 100);
        assert_eq!(p["segment"]["endMs"], 900);
    }
}
