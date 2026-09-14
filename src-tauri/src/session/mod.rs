//! 会话：转写结果、纪要、统计的容器，以及持久化与导出。
//!
//! 存储形式是「一会话一 JSON 文件」（`sessions/{id}.json`）。这样：
//!   * 崩溃最多丢最后一次写入，不会破坏整个库
//!   * 直接用文本编辑器就能看/修
//!   * 导出、备份、迁移都很简单

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::ai::summarizer::SummaryState;
use crate::error::{AppError, AppResult};
use crate::util::{format_clock, now_ms, sanitize_filename};

/* ==========================================================================
 * 数据模型（与 src/lib/contract.ts 一一对应）
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub id: u64,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub confidence: Option<f32>,
    /// 这段结果为什么可疑（幻听/重复/服务没返回内容）。
    ///
    /// **有值不代表文本被丢弃**：文本照常显示，只是界面上会标灰提示，
    /// 并且不会进入纪要输入，避免污染摘要。
    pub suspect: Option<String>,
}

impl TranscriptSegment {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionStats {
    /// 已识别语音的累计时长
    pub speech_ms: i64,
    /// 已采集音频的累计时长
    pub audio_ms: i64,
    pub chars: i64,
    /// 实时率：识别耗时 / 音频时长
    pub rtf: f32,
    /// 从说完到出字的中位延迟
    pub latency_ms: i64,
    pub segments: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionConfig {
    pub model_id: String,
    pub enable_mic: bool,
    pub enable_loopback: bool,
    pub mic_label: Option<String>,
    pub loopback_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Recording,
    Paused,
    Finished,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub created_at: i64,
    pub duration_ms: i64,
    pub config: SessionConfig,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub created_at: i64,
    pub duration_ms: i64,
    pub config: SessionConfig,
    pub error: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    pub summary: SummaryState,
    pub report_md: Option<String>,
    pub audio_path: Option<String>,
    pub stats: SessionStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub created_at: i64,
    pub duration_ms: i64,
    pub segments: usize,
    pub chars: i64,
    pub has_summary: bool,
    pub audio_path: Option<String>,
}

/* ==========================================================================
 * Session
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub status: SessionStatus,
    pub created_at: i64,
    /// 累计录音时长（毫秒，不含暂停时间）
    pub duration_ms: i64,
    pub config: SessionConfig,
    pub error: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    pub summary: SummaryState,
    pub report_md: Option<String>,
    pub audio_path: Option<String>,
    pub stats: SessionStats,
    /// 最近一次恢复录音的墙钟时间；仅运行期有意义，不持久化
    #[serde(skip)]
    pub resumed_at: Option<i64>,
    #[serde(skip)]
    next_segment_id: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            status: SessionStatus::Finished,
            created_at: now_ms(),
            duration_ms: 0,
            config: SessionConfig::default(),
            error: None,
            segments: Vec::new(),
            summary: SummaryState::default(),
            report_md: None,
            audio_path: None,
            stats: SessionStats::default(),
            resumed_at: None,
            next_segment_id: 1,
        }
    }
}

impl Session {
    pub fn new(title: String, config: SessionConfig) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            status: SessionStatus::Recording,
            created_at: now_ms(),
            resumed_at: Some(now_ms()),
            ..Default::default()
        }
        .with_config(config)
    }

    fn with_config(mut self, config: SessionConfig) -> Self {
        self.config = config;
        self
    }

    /// 包含当前正在进行的这一段录音的时长
    pub fn live_duration_ms(&self, now: i64) -> i64 {
        match (self.status, self.resumed_at) {
            (SessionStatus::Recording, Some(t)) => self.duration_ms + (now - t).max(0),
            _ => self.duration_ms,
        }
    }

    /// 当前会话内所有转写的结束位置（毫秒）
    pub fn last_segment_end_ms(&self) -> i64 {
        self.segments.last().map(|s| s.end_ms).unwrap_or(0)
    }

    pub fn next_id(&mut self) -> u64 {
        if self.next_segment_id == 0 {
            self.next_segment_id = self
                .segments
                .iter()
                .map(|s| s.id)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
        }
        let id = self.next_segment_id;
        self.next_segment_id = id.saturating_add(1);
        id
    }

    /// 追加一段转写。
    ///
    /// 注意：`stats.speech_ms` 不在这里累加 —— 它由断句线程按 VAD 判定的语音区间
    /// 统一维护（见 `pipeline::run_segmenter`）。两边都写会导致重复累加，
    /// 出现「语音总时长是音频总长两倍」这种不可能的数字。
    pub fn push_segment(&mut self, mut segment: TranscriptSegment) -> u64 {
        if segment.id == 0 {
            segment.id = self.next_id();
        }
        self.stats.segments = self.segments.len() as i64 + 1;
        self.stats.chars += segment.text.chars().count() as i64;
        let id = segment.id;
        self.segments.push(segment);
        id
    }

    /// 把最近一段时间的原文拼成提示词里用的「最近对话」
    pub fn recent_transcript(&self, now_ms: i64, window_secs: u32) -> String {
        let from = (now_ms - window_secs as i64 * 1000).max(0);
        let end = self.last_segment_end_ms();
        let from = from.min(end);
        self.transcript_between(from, end)
    }

    /// 取指定时间区间的转写文本（带时间戳与说话人）
    pub fn transcript_between(&self, from_ms: i64, to_ms: i64) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            if seg.end_ms <= from_ms || seg.start_ms > to_ms {
                continue;
            }
            out.push_str(&format!("[{}] {}\n", format_clock(seg.start_ms), seg.text));
        }
        out
    }

    /// 给大模型看的转写文本：**排除可疑段**。
    ///
    /// 可疑段（幻听/重复/服务没返回内容）照常显示给用户看，但不该喂给大模型 ——
    /// 一段英文幻觉足以把整份纪要带偏。如果全都是可疑段，返回空串，
    /// 让纪要循环知道「这轮没有可用的新内容」。
    pub fn transcript_between_trusted(&self, from_ms: i64, to_ms: i64) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            if seg.suspect.is_some() || seg.end_ms <= from_ms || seg.start_ms > to_ms {
                continue;
            }
            out.push_str(&format!("[{}] {}\n", format_clock(seg.start_ms), seg.text));
        }
        out
    }

    /// 完整转写（用于生成最终纪要），过长时保留头尾。
    /// 同样跳过可疑段，理由见 [`Self::transcript_between_trusted`]。
    pub fn full_transcript(&self, max_chars: usize) -> String {
        let mut out = String::new();
        for seg in self.segments.iter().filter(|s| s.suspect.is_none()) {
            out.push_str(&format!("[{}] {}\n", format_clock(seg.start_ms), seg.text));
        }
        crate::util::truncate_middle(out.trim(), max_chars)
    }

    pub fn info(&self, now: i64) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            status: self.status,
            created_at: self.created_at,
            duration_ms: self.live_duration_ms(now),
            config: self.config.clone(),
            error: self.error.clone(),
        }
    }

    pub fn detail(&self, now: i64) -> SessionDetail {
        SessionDetail {
            id: self.id.clone(),
            title: self.title.clone(),
            status: self.status,
            created_at: self.created_at,
            duration_ms: self.live_duration_ms(now),
            config: self.config.clone(),
            error: self.error.clone(),
            segments: self.segments.clone(),
            summary: self.summary.clone(),
            report_md: self.report_md.clone(),
            audio_path: self.audio_path.clone(),
            stats: self.stats,
        }
    }

    pub fn list_item(&self) -> SessionListItem {
        SessionListItem {
            id: self.id.clone(),
            title: self.title.clone(),
            status: self.status,
            created_at: self.created_at,
            duration_ms: self.duration_ms,
            segments: self.segments.len(),
            chars: self.stats.chars,
            has_summary: !self.summary.is_empty(),
            audio_path: self.audio_path.clone(),
        }
    }
}

/* ==========================================================================
 * 持久化
 * ========================================================================== */

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 只允许字母数字与连字符，防止 `../` 之类的路径穿越
    fn path_of(&self, id: &str) -> AppResult<PathBuf> {
        if id.is_empty()
            || id.len() > 64
            || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AppError::session(format!("非法的会话 id：{id}")));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn save(&self, session: &Session) -> AppResult<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path_of(&session.id)?;
        let text = serde_json::to_string(session)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> AppResult<Session> {
        let path = self.path_of(id)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|_| AppError::session(format!("会话不存在：{id}")))?;
        let mut session: Session =
            serde_json::from_str(&text).map_err(|e| AppError::session(format!("会话文件损坏：{e}")))?;
        session.status = match session.status {
            // 上次异常退出时状态可能停留在 recording，加载时纠正
            SessionStatus::Recording | SessionStatus::Paused => SessionStatus::Finished,
            other => other,
        };
        session.resumed_at = None;
        session.next_segment_id = 0;
        Ok(session)
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path_of(id).map(|p| p.is_file()).unwrap_or(false)
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        let path = self.path_of(id)?;
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// 按创建时间倒序列出全部会话
    pub fn list(&self) -> AppResult<Vec<SessionListItem>> {
        let mut items = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return Ok(items), // 目录还不存在 = 没有会话
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            match self.load(id) {
                Ok(session) => items.push(session.list_item()),
                Err(e) => tracing::warn!("跳过无法读取的会话 {id}：{e}"),
            }
        }
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(items)
    }
}

/* ==========================================================================
 * 导出
 * ========================================================================== */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Text,
    Srt,
    Json,
}

impl ExportFormat {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Ok(Self::Markdown),
            "txt" | "text" => Ok(Self::Text),
            "srt" => Ok(Self::Srt),
            "json" => Ok(Self::Json),
            other => Err(AppError::session(format!(
                "不支持的导出格式：{other}（可选 md / txt / srt / json）"
            ))),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Srt => "srt",
            Self::Json => "json",
        }
    }

    pub fn default_file_name(&self, session: &Session) -> String {
        let stamp = chrono::DateTime::from_timestamp_millis(session.created_at)
            .map(|d| d.format("%Y%m%d-%H%M").to_string())
            .unwrap_or_else(|| "meeting".into());
        let title = sanitize_filename(&session.title, "会议");
        format!("{stamp}-{title}.{}", self.extension())
    }
}

pub fn render_export(session: &Session, format: ExportFormat) -> AppResult<String> {
    Ok(match format {
        ExportFormat::Markdown => render_markdown(session),
        ExportFormat::Text => render_text(session),
        ExportFormat::Srt => render_srt(session),
        ExportFormat::Json => serde_json::to_string_pretty(session)?,
    })
}

fn format_created_at(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".into())
}

fn render_markdown(s: &Session) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", s.title));

    let mut sources: Vec<String> = Vec::new();
    if s.config.enable_mic {
        sources.push(match &s.config.mic_label {
            Some(l) => format!("麦克风（{l}）"),
            None => "麦克风".into(),
        });
    }
    if s.config.enable_loopback {
        sources.push(match &s.config.loopback_label {
            Some(l) => format!("系统声音（{l}）"),
            None => "系统声音".into(),
        });
    }
    if sources.is_empty() {
        sources.push("音频文件".into());
    }

    out.push_str("| 项目 | 内容 |\n| --- | --- |\n");
    out.push_str(&format!("| 开始时间 | {} |\n", format_created_at(s.created_at)));
    out.push_str(&format!("| 时长 | {} |\n", format_clock(s.duration_ms)));
    out.push_str(&format!("| 识别服务 | {} |\n", s.config.model_id));
    out.push_str(&format!("| 音频来源 | {} |\n", sources.join(" + ")));
    out.push_str(&format!(
        "| 语音时长 | {} |\n",
        format_clock(s.stats.speech_ms)
    ));
    out.push_str(&format!("| 字数 | {} |\n", s.stats.chars));
    out.push('\n');

    if let Some(report) = &s.report_md {
        out.push_str("## 会议纪要\n\n");
        out.push_str(report.trim());
        out.push_str("\n\n");
    }

    let summary = &s.summary;
    if !summary.overview.trim().is_empty() {
        out.push_str("## 总览\n\n");
        out.push_str(summary.overview.trim());
        out.push_str("\n\n");
    }
    if !summary.summary.trim().is_empty() {
        out.push_str("## 纪要要点\n\n");
        out.push_str(summary.summary.trim());
        out.push_str("\n\n");
    }
    if !summary.key_points.is_empty() {
        out.push_str("## 关键结论\n\n");
        for p in &summary.key_points {
            out.push_str(&format!("- {p}\n"));
        }
        out.push('\n');
    }
    if !summary.decisions.is_empty() {
        out.push_str("## 已达成的决定\n\n");
        for d in &summary.decisions {
            out.push_str(&format!("- {d}\n"));
        }
        out.push('\n');
    }
    if !summary.action_items.is_empty() {
        out.push_str("## 待办事项\n\n");
        out.push_str("| 事项 | 负责人 | 截止时间 |\n| --- | --- | --- |\n");
        for item in &summary.action_items {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                item.text,
                if item.owner.is_empty() { "待确认" } else { &item.owner },
                if item.due.is_empty() { "待确认" } else { &item.due }
            ));
        }
        out.push('\n');
    }
    if !summary.topics.is_empty() {
        out.push_str(&format!("**讨论主题**：{}\n\n", summary.topics.join(" · ")));
    }

    out.push_str("## 完整转写\n\n");
    if s.segments.is_empty() {
        out.push_str("_（没有识别到内容）_\n");
    } else {
        for seg in &s.segments {
            out.push_str(&format!(
                "**[{}]** {}\n\n",
                format_clock(seg.start_ms),
                seg.text
            ));
        }
    }
    out
}

fn render_text(s: &Session) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", s.title));
    out.push_str(&format!(
        "{} · 时长 {}\n\n",
        format_created_at(s.created_at),
        format_clock(s.duration_ms)
    ));
    for seg in &s.segments {
        out.push_str(&format!("[{}] {}\n", format_clock(seg.start_ms), seg.text));
    }
    out
}

fn srt_timestamp(ms: i64) -> String {
    let ms = ms.max(0);
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

fn render_srt(s: &Session) -> String {
    let mut out = String::new();
    for (i, seg) in s.segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            srt_timestamp(seg.start_ms),
            srt_timestamp(seg.end_ms.max(seg.start_ms + 200))
        ));
        out.push_str(&format!("{}\n\n", seg.text));
    }
    out
}

/// 生成默认标题
pub fn default_title(now: i64, prefix: &str) -> String {
    let stamp = chrono::DateTime::from_timestamp_millis(now)
        .map(|d| d.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "会议".into());
    format!("{prefix} {stamp}")
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::summarizer::ActionItem;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mh-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample_session() -> Session {
        let mut s = Session::new("产品周会".into(), SessionConfig {
            model_id: "small".into(),
            enable_mic: true,
            enable_loopback: true,
            mic_label: Some("MacBook 麦克风".into()),
            loopback_label: Some("扬声器".into()),
        });
        s.duration_ms = 754_000;
        s.status = SessionStatus::Finished;
        s.push_segment(TranscriptSegment {
            id: 0,
            text: "我们先过一下三季度的增长情况。".into(),
            start_ms: 1_200,
            end_ms: 4_500,
            confidence: Some(0.92),
            suspect: None,
        });
        s.push_segment(TranscriptSegment {
            id: 0,
            text: "流失率上升了两个百分点，主要在中小客户。".into(),
            start_ms: 5_000,
            end_ms: 9_800,
            confidence: Some(0.88),
            suspect: None,
        });
        s.summary.overview = "复盘三季度并确定下季度方向。".into();
        s.summary.summary = "- 三季度营收环比 +18%\n- 流失率上升 2pt".into();
        s.summary.key_points = vec!["企业版是增长主引擎".into()];
        s.summary.action_items = vec![ActionItem {
            text: "输出埋点方案".into(),
            owner: "张三".into(),
            due: "下周三".into(),
        }];
        s.summary.revision = 3;
        s
    }

    #[test]
    #[test]
    fn segment_ids_are_monotonic_and_stats_accumulate() {
        let mut s = Session::default();
        let a = s.push_segment(TranscriptSegment {
            id: 0,
            text: "一二三".into(),
            start_ms: 0,
            end_ms: 1_000,
            confidence: None,
            suspect: None,
        });
        let b = s.push_segment(TranscriptSegment {
            id: 0,
            text: "四五".into(),
            start_ms: 1_000,
            end_ms: 2_500,
            confidence: None,
            suspect: None,
        });
        assert_eq!((a, b), (1, 2));
        assert_eq!(s.stats.chars, 5);
        assert_eq!(s.stats.segments, 2);
        // speech_ms 由断句线程维护，这里不应被改动
        assert_eq!(s.stats.speech_ms, 0);
        assert_eq!(s.last_segment_end_ms(), 2_500);
    }

    #[test]
    fn live_duration_includes_running_time() {
        let mut s = Session::new("会议".into(), SessionConfig::default());
        s.duration_ms = 10_000;
        let now = now_ms();
        s.resumed_at = Some(now - 5_000);
        assert!((s.live_duration_ms(now) - 15_000).abs() < 50);
        s.status = SessionStatus::Paused;
        assert_eq!(s.live_duration_ms(now), 10_000);
    }

    #[test]
    fn store_roundtrip() {
        let dir = tmpdir("roundtrip");
        let store = SessionStore::new(dir.clone());
        let s = sample_session();
        store.save(&s).unwrap();

        let loaded = store.load(&s.id).unwrap();
        assert_eq!(loaded.title, "产品周会");
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.summary.action_items[0].owner, "张三");
        assert_eq!(loaded.stats.chars, s.stats.chars);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_rejects_path_traversal() {
        let dir = tmpdir("traversal");
        let store = SessionStore::new(dir.clone());
        assert!(store.load("../../etc/passwd").is_err());
        assert!(store.load("a/b").is_err());
        assert!(store.load("").is_err());
        assert!(store.delete("../evil").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_list_is_sorted_by_time_desc() {
        let dir = tmpdir("list");
        let store = SessionStore::new(dir.clone());
        for (i, title) in ["最早", "中间", "最新"].iter().enumerate() {
            let mut s = sample_session();
            s.id = format!("id{i}");
            s.title = (*title).into();
            s.created_at = 1_000 + i as i64 * 1_000;
            store.save(&s).unwrap();
        }
        let list = store.list().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].title, "最新");
        assert_eq!(list[2].title, "最早");
        assert!(list[0].has_summary);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_list_on_missing_dir_is_empty() {
        let store = SessionStore::new(PathBuf::from("/definitely/not/here/xyz"));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn load_fixes_stale_recording_status() {
        let dir = tmpdir("stale");
        let store = SessionStore::new(dir.clone());
        let mut s = sample_session();
        s.status = SessionStatus::Recording;
        store.save(&s).unwrap();
        let loaded = store.load(&s.id).unwrap();
        assert_eq!(loaded.status, SessionStatus::Finished, "异常退出的会话应被标记为已结束");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupted_session_file_reports_error() {
        let dir = tmpdir("corrupt");
        let store = SessionStore::new(dir.clone());
        std::fs::write(dir.join("bad.json"), "{ this is not json").unwrap();
        assert!(store.load("bad").is_err());
        // 列表接口不应该因为一个坏文件整体失败
        assert!(store.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn markdown_export_contains_all_sections() {
        let s = sample_session();
        let md = render_export(&s, ExportFormat::Markdown).unwrap();
        assert!(md.starts_with("# 产品周会"));
        assert!(md.contains("| 识别服务 | small |"));
        assert!(md.contains("## 总览"));
        assert!(md.contains("## 关键结论"));
        assert!(md.contains("## 待办事项"));
        assert!(md.contains("| 输出埋点方案 | 张三 | 下周三 |"));
        assert!(md.contains("## 完整转写"));
        assert!(md.contains("**[00:00:01]** 我们先过一下三季度的增长情况。"));
        assert!(md.contains("系统声音（扬声器）"));
    }

    #[test]
    fn srt_export_is_wellformed() {
        let s = sample_session();
        let srt = render_export(&s, ExportFormat::Srt).unwrap();
        assert!(srt.starts_with("1\n00:00:01,200 --> 00:00:04,500\n"));
        assert!(srt.contains("我们先过一下三季度的增长情况。"));
        assert!(srt.contains("2\n00:00:05,000 --> 00:00:09,800"));
    }

    #[test]
    fn srt_timestamp_formats_hours() {
        assert_eq!(srt_timestamp(0), "00:00:00,000");
        assert_eq!(srt_timestamp(3_661_005), "01:01:01,005");
    }

    #[test]
    fn text_export_is_plain() {
        let s = sample_session();
        let txt = render_export(&s, ExportFormat::Text).unwrap();
        assert!(txt.contains("[00:00:01] 我们先过一下三季度的增长情况。"));
        assert!(!txt.contains("##"));
    }

    #[test]
    fn json_export_roundtrips() {
        let s = sample_session();
        let json = render_export(&s, ExportFormat::Json).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, s.title);
        assert_eq!(back.segments.len(), 2);
    }

    #[test]
    fn export_format_parsing() {
        assert_eq!(ExportFormat::parse("md").unwrap(), ExportFormat::Markdown);
        assert_eq!(ExportFormat::parse("SRT").unwrap(), ExportFormat::Srt);
        assert_eq!(ExportFormat::parse("txt").unwrap(), ExportFormat::Text);
        assert_eq!(ExportFormat::parse("json").unwrap(), ExportFormat::Json);
        assert!(ExportFormat::parse("pdf").is_err());
        assert_eq!(ExportFormat::Markdown.extension(), "md");
    }

    #[test]
    fn default_file_name_is_safe() {
        let mut s = sample_session();
        s.title = "2024/03/01 周会:复盘?".into();
        let name = ExportFormat::Markdown.default_file_name(&s);
        assert!(name.ends_with(".md"));
        assert!(!name.contains('/'), "文件名不能含路径分隔符：{name}");
        assert!(!name.contains('?'));
        assert!(!name.contains(':'));
    }

    #[test]
    fn suspect_segments_are_kept_but_excluded_from_summary_input() {
        // 真实事故回归：模型/语言不匹配时服务返回英文幻觉，旧实现把它整段丢掉，
        // 界面上一个字都没有。现在的约定是 —— 照常展示，但不进纪要。
        let mut s = Session::default();
        s.push_segment(TranscriptSegment {
            id: 0,
            text: "我们先过一下三季度的增长情况。".into(),
            start_ms: 0,
            end_ms: 3_000,
            confidence: None,
            suspect: None,
        });
        s.push_segment(TranscriptSegment {
            id: 0,
            text: "Thank you for watching!".into(),
            start_ms: 3_000,
            end_ms: 5_000,
            confidence: None,
            suspect: Some("疑似模型幻听短语".into()),
        });

        // 1) 段落本身必须留在会话里（界面才能显示出来）
        assert_eq!(s.segments.len(), 2, "可疑段不得被删除");
        assert!(s.segments.iter().any(|x| x.text.contains("Thank you")));
        // 字数统计照旧，用户看到的确实是这些内容
        assert_eq!(s.stats.chars, s.segments.iter().map(|x| x.text.chars().count() as i64).sum::<i64>());

        // 2) 但对大模型要隐身：幻觉足以把整份纪要带偏
        let all = s.transcript_between(0, 10_000);
        assert!(all.contains("Thank you"), "给人看的转写要包含可疑段");
        let trusted = s.transcript_between_trusted(0, 10_000);
        assert!(trusted.contains("三季度"), "正常内容要进纪要");
        assert!(!trusted.contains("Thank you"), "可疑内容不得进纪要");
        assert!(!s.full_transcript(usize::MAX).contains("Thank you"));
    }

    #[test]
    fn transcript_between_includes_overlapping_segments() {
        let s = sample_session();
        // 区间 [4000,10000] 与第一段 [1200,4500] 有重叠，因此两段都在
        let window = s.transcript_between(4_000, 10_000);
        assert!(window.contains("我们先过一下"));
        assert!(window.contains("流失率上升"));

        // 完全落在区间之前的段不会被带进来
        let later = s.transcript_between(9_000, 10_000);
        assert!(!later.contains("我们先过一下"));
        assert!(later.contains("流失率上升"));
    }

    #[test]
    fn incremental_windows_do_not_duplicate_segments() {
        // 增量总结是「从上次覆盖到的位置继续」，相邻两轮之间不能重复取到同一段
        let s = sample_session();
        let first = s.transcript_between(0, 4_500);
        assert_eq!(first.matches("我们先过一下").count(), 1);
        let second = s.transcript_between(4_500, 9_800);
        assert!(!second.contains("我们先过一下"), "第二轮不应重复第一段");
        assert_eq!(second.matches("流失率上升").count(), 1);
    }

    #[test]
    fn full_transcript_includes_timestamps() {
        let s = sample_session();
        let t = s.full_transcript(10_000);
        assert!(t.contains("[00:00:01] 我们先过一下三季度的增长情况。"));
        assert!(t.contains("[00:00:05] 流失率上升了两个百分点"));
    }

    #[test]
    fn session_info_and_detail_are_consistent() {
        let s = sample_session();
        let now = now_ms();
        let info = s.info(now);
        let detail = s.detail(now);
        assert_eq!(info.id, detail.id);
        assert_eq!(info.duration_ms, detail.duration_ms);
        assert_eq!(detail.segments.len(), 2);
        assert_eq!(detail.summary.revision, 3);
        assert!(detail.report_md.is_none());
    }
}
