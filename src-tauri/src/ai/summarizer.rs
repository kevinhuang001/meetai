//! 滚动纪要状态机。
//!
//! 状态是「可累积、可序列化」的：会话持久化后重启还能继续增量更新。
//! 模型返回的 JSON 这里的解析做了三层容错：
//!   1. 去掉 ```json 代码块包裹 / 前后多余的解释文字
//!   2. 模型偶尔会套一层 {"result": {...}}，自动下钻
//!   3. 字段缺失、类型不对（例如待办只给了字符串）都能兜住

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::client::TokenUsage;
use crate::error::{AppError, AppResult};
use crate::util::{now_ms, truncate_chars};

/// 纪要正文与列表的上限，防止长时间会议把状态撑爆
/// 把分段渲染成 markdown 无序列表（导出、完整纪要都复用它）
pub fn sections_to_markdown(sections: &[SummarySection]) -> String {
    let mut out: Vec<String> = Vec::new();
    for sec in sections {
        if sections.len() > 1 {
            out.push(format!("**{}**", sec.title));
        }
        for p in &sec.points {
            out.push(format!("- {p}"));
        }
    }
    out.join("\n")
}

/// 分段数量上限（够覆盖两三个小时的会议，又不至于让面板失控）
const MAX_SECTIONS: usize = 40;
/// 每段要点上限
const MAX_SECTION_POINTS: usize = 12;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_LIST_ITEMS: usize = 20;
const MAX_TOPICS: usize = 8;

/* ==========================================================================
 * 数据模型
 * ========================================================================== */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub text: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub due: String,
}

impl ActionItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), owner: String::new(), due: String::new() }
    }
}

/// 待办事项允许模型返回纯字符串或对象两种形状
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ActionItemRaw {
    Text(String),
    Full(ActionItem),
}

impl ActionItemRaw {
    fn into_item(self) -> ActionItem {
        match self {
            ActionItemRaw::Text(t) => ActionItem::new(t.trim()),
            ActionItemRaw::Full(mut a) => {
                a.text = a.text.trim().to_string();
                a.owner = a.owner.trim().to_string();
                a.due = a.due.trim().to_string();
                a
            }
        }
    }
}

/// 纪要的一个分段：模型按议题/阶段自己切，并随着会议推进**重新分段**。
///
/// 为什么要有分段：纪要要展示的是**从会议开始到现在的全部内容**。
/// 平铺成一长串要点没法看，而按「谁在讲什么」切段之后，
/// 用户扫一眼就知道整场会议都覆盖了哪些块、讲到哪了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySection {
    /// 分段标题，例如「三季度复盘」「注意力机制」
    pub title: String,
    pub points: Vec<String>,
    /// 这一段覆盖到的时间位置（毫秒），用于显示「讲到哪」
    #[serde(default)]
    pub until_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SummaryState {
    /// 刚刚说了什么
    pub live: String,
    /// 整场总览
    pub overview: String,
    /// markdown 无序列表形式的完整纪要
    pub summary: String,
    /// 按议题分好的完整纪要（覆盖会议开始到现在的全部内容）
    pub sections: Vec<SummarySection>,
    pub key_points: Vec<String>,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
    pub topics: Vec<String>,
    pub updated_at: i64,
    /// 已经纳入纪要的转写位置（毫秒）
    pub covered_until_ms: i64,
    pub revision: u32,
    pub model: Option<String>,
    pub error: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub calls: u32,
}

impl Default for SummaryState {
    fn default() -> Self {
        Self {
            live: String::new(),
            overview: String::new(),
            summary: String::new(),
            sections: Vec::new(),
            key_points: Vec::new(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            topics: Vec::new(),
            updated_at: 0,
            covered_until_ms: 0,
            revision: 0,
            model: None,
            error: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            calls: 0,
        }
    }
}

impl SummaryState {
    pub fn is_empty(&self) -> bool {
        self.revision == 0
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }

    /// 已达成/关键点等拼成一行，用于塞进下一轮提示词
    pub fn action_items_text(&self) -> String {
        if self.action_items.is_empty() {
            return String::new();
        }
        self.action_items
            .iter()
            .map(|a| {
                let mut s = format!("- {}", a.text);
                if !a.owner.is_empty() {
                    s.push_str(&format!("（负责人：{}）", a.owner));
                }
                if !a.due.is_empty() {
                    s.push_str(&format!("（截止：{}）", a.due));
                }
                s
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/* ==========================================================================
 * 模型返回的增量补丁
 * ========================================================================== */

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SummaryPatch {
    #[serde(alias = "live_summary", alias = "liveSummary", alias = "recent")]
    pub live: Option<String>,
    #[serde(alias = "abstract", alias = "summaryOverview")]
    pub overview: Option<String>,
    #[serde(alias = "summaryMarkdown", alias = "minutes", alias = "notes")]
    pub summary: Option<String>,
    /// 分段纪要：模型每次都要输出**完整**的分段列表（含之前的所有段）
    #[serde(alias = "outline", alias = "blocks", alias = "chapters")]
    pub sections: Option<Vec<SectionRaw>>,
    #[serde(alias = "key_points", alias = "keypoints", alias = "points", alias = "highlights")]
    pub key_points: Option<Vec<String>>,
    #[serde(alias = "decisions_made", alias = "conclusions")]
    pub decisions: Option<Vec<String>>,
    #[serde(alias = "action_items", alias = "todos", alias = "tasks", alias = "actionItemsList")]
    pub action_items: Option<Vec<ActionItemRaw>>,
    #[serde(alias = "topic", alias = "current_topics")]
    pub topics: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SectionRaw {
    #[serde(alias = "heading", alias = "name", alias = "topic", alias = "segment")]
    pub title: Option<String>,
    #[serde(alias = "items", alias = "bullets", alias = "content", alias = "details")]
    pub points: Option<Vec<String>>,
    #[serde(alias = "untilMs", alias = "endMs", alias = "at")]
    pub until_ms: Option<i64>,
}

/// 把模型返回的补丁合并进状态。返回是否发生了实质变化。
pub fn apply_patch(
    state: &mut SummaryState,
    patch: SummaryPatch,
    model: &str,
    covered_until_ms: i64,
    usage: &TokenUsage,
) -> bool {
    let mut changed = false;

    if let Some(v) = patch.live {
        let v = collapse_ws(&v);
        if !v.is_empty() && v != state.live {
            state.live = v;
            changed = true;
        }
    }
    if let Some(v) = patch.overview {
        let v = collapse_ws(&v);
        if !v.is_empty() && v != state.overview {
            state.overview = truncate_chars(&v, MAX_SUMMARY_CHARS / 2);
            changed = true;
        }
    }
    // 兼容：模型偶尔仍会返回 summary（旧提示词/别的模型）。没有分段时用它兜底，
    // 有分段时以分段为准（分段才是主内容）。
    if let Some(v) = patch.summary {
        let v = normalize_markdown_list(&v);
        if !v.is_empty() && v != state.summary && state.sections.is_empty() {
            state.summary = truncate_chars(&v, MAX_SUMMARY_CHARS);
            changed = true;
        }
    }

    // 分段纪要：非空就整体替换（模型每次都输出完整列表，直接换掉最省事也最不容易串味）。
    // 空列表保留旧值 —— 模型偶尔会漏输出，直接清空等于把整场纪要丢了。
    if let Some(raw) = patch.sections {
        let mut sections: Vec<SummarySection> = Vec::new();
        for r in raw {
            let title = collapse_ws(r.title.as_deref().unwrap_or_default());
            let points = clean_list(r.points.unwrap_or_default(), MAX_SECTION_POINTS);
            if title.is_empty() && points.is_empty() {
                continue;
            }
            let title = if title.is_empty() {
                "未命名段落".to_string()
            } else {
                truncate_chars(&title, 60)
            };
            // 标题相同的相邻段合并，避免模型把同一议题拆成好几段
            if let Some(last) = sections.last_mut() {
                if last.title == title {
                    last.points.extend(points);
                    last.points.truncate(MAX_SECTION_POINTS);
                    continue;
                }
            }
            sections.push(SummarySection {
                title,
                points,
                until_ms: r.until_ms.unwrap_or(0),
            });
        }
        if !sections.is_empty() {
            sections.truncate(MAX_SECTIONS);
            if sections != state.sections {
                state.sections = sections;
                // summary 由分段派生，不额外让模型维护一份重复内容
                state.summary = truncate_chars(&sections_to_markdown(&state.sections), MAX_SUMMARY_CHARS);
                changed = true;
            }
        }
    }

    // 列表类字段：模型给了非空就用新的，给了空数组则保留旧值
    // （模型偶尔会忘记重复输出已有条目，直接清空会造成信息丢失）
    if let Some(items) = patch.key_points {
        let cleaned = clean_list(items, MAX_LIST_ITEMS);
        if !cleaned.is_empty() && cleaned != state.key_points {
            state.key_points = cleaned;
            changed = true;
        }
    }
    if let Some(items) = patch.decisions {
        let cleaned = clean_list(items, MAX_LIST_ITEMS);
        if !cleaned.is_empty() && cleaned != state.decisions {
            state.decisions = cleaned;
            changed = true;
        }
    }
    if let Some(items) = patch.action_items {
        let cleaned: Vec<ActionItem> = items
            .into_iter()
            .map(ActionItemRaw::into_item)
            .filter(|a| !a.text.is_empty())
            .take(MAX_LIST_ITEMS)
            .collect();
        if !cleaned.is_empty() && cleaned != state.action_items {
            state.action_items = cleaned;
            changed = true;
        }
    }
    if let Some(items) = patch.topics {
        let cleaned = clean_list(items, MAX_TOPICS);
        if !cleaned.is_empty() && cleaned != state.topics {
            state.topics = cleaned;
            changed = true;
        }
    }

    // 元信息始终更新
    state.covered_until_ms = covered_until_ms.max(state.covered_until_ms);
    state.revision = state.revision.saturating_add(1);
    state.updated_at = now_ms();
    state.model = Some(model.to_string());
    state.calls = state.calls.saturating_add(1);
    state.prompt_tokens = state.prompt_tokens.saturating_add(usage.prompt_tokens);
    state.completion_tokens = state.completion_tokens.saturating_add(usage.completion_tokens);
    state.clear_error();

    changed
}

/* ==========================================================================
 * 解析
 * ========================================================================== */

/// 从模型输出里抠出 JSON 对象并解析成补丁。
pub fn parse_summary_patch(raw: &str) -> AppResult<SummaryPatch> {
    // 把模型原始输出带进错误信息：模型不听话是常态（尤其小参数模型），
    // 用户看到原文才能判断是提示词问题还是模型能力问题。
    let json_text = extract_json_object(raw).ok_or_else(|| {
        AppError::ai(format!(
            "模型没有按要求返回 JSON 纪要。原始输出：{}",
            crate::util::truncate_chars(raw.trim(), 300)
        ))
    })?;

    let value: Value = serde_json::from_str(&json_text)
        .map_err(|e| AppError::ai(format!("模型返回的 JSON 解析失败：{e}")))?;

    let value = unwrap_nested(value);
    serde_json::from_value::<SummaryPatch>(value)
        .map_err(|e| AppError::ai(format!("纪要字段与预期不符：{e}")))
}

/// 去掉 markdown 代码块、前后解释文字，返回第一个完整的 JSON 对象文本。
pub fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 快路径：整段就是 JSON
    if trimmed.starts_with('{') && serde_json::from_str::<Value>(trimmed).is_ok() {
        return Some(trimmed.to_string());
    }

    // 去掉 ```json ... ``` 包裹
    let unfenced = strip_code_fence(trimmed);
    let candidate = unfenced.trim();
    if candidate.starts_with('{') && serde_json::from_str::<Value>(candidate).is_ok() {
        return Some(candidate.to_string());
    }

    // 兜底：花括号配对扫描，取出第一个对象
    let bytes = candidate.as_bytes();
    let start = candidate.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let c = b as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(candidate[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_code_fence(s: &str) -> String {
    if !s.starts_with("```") {
        return s.to_string();
    }
    let mut lines = s.lines();
    lines.next(); // 丢掉 ```json 那行
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        body.push(line);
    }
    body.join("\n")
}

/// 模型有时会包一层 {"result": {...}} 或 {"summary": {...}}，下钻一层
fn unwrap_nested(value: Value) -> Value {
    const KNOWN: &[&str] = &[
        "live", "overview", "summary", "keyPoints", "key_points", "decisions", "actionItems",
        "action_items", "topics",
    ];
    let obj = match value.as_object() {
        Some(o) => o,
        None => return value,
    };
    if obj.keys().any(|k| KNOWN.contains(&k.as_str())) {
        return value;
    }
    // 只有一个键且值是对象 → 下钻
    if obj.len() == 1 {
        if let Some(inner) = obj.values().next() {
            if inner.is_object() {
                return inner.clone();
            }
        }
    }
    // 有 result / data 字段且是对象 → 下钻
    for key in ["result", "data", "output", "minutes"] {
        if let Some(inner) = obj.get(key) {
            if inner.is_object() {
                return inner.clone();
            }
        }
    }
    value
}

fn clean_list(items: Vec<String>, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let s = normalize_item(&item);
        if s.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == &s) {
            out.push(s);
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn normalize_item(s: &str) -> String {
    let t = s
        .trim()
        .trim_start_matches(['-', '*', '•', '·'])
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', ')', '、', ' '])
        .trim();
    collapse_ws(t)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 保证 summary 是规范的 markdown 列表
fn normalize_markdown_list(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let stripped = t.trim_start_matches(['-', '*', '•', '·']).trim();
        if stripped.is_empty() {
            continue;
        }
        // 已经是标题/引用就原样保留
        if t.starts_with('#') || t.starts_with('>') {
            lines.push(t.to_string());
        } else {
            lines.push(format!("- {stripped}"));
        }
    }
    lines.join("\n")
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    fn usage() -> TokenUsage {
        TokenUsage { prompt_tokens: 100, completion_tokens: 50, total_tokens: 150 }
    }

    #[test]
    fn parses_plain_json() {
        let raw = r#"{"live":"在讨论增长","overview":"整体进展","summary":"- 营收 +18%","keyPoints":["企业版驱动"],"decisions":["定目标"],"actionItems":[{"text":"出方案","owner":"张三","due":"周三"}],"topics":["增长"]}"#;
        let patch = parse_summary_patch(raw).unwrap();
        let mut st = SummaryState::default();
        assert!(apply_patch(&mut st, patch, "deepseek-chat", 12_000, &usage()));
        assert_eq!(st.live, "在讨论增长");
        assert_eq!(st.key_points, vec!["企业版驱动"]);
        assert_eq!(st.action_items[0].owner, "张三");
        assert_eq!(st.covered_until_ms, 12_000);
        assert_eq!(st.revision, 1);
        assert_eq!(st.calls, 1);
        assert_eq!(st.prompt_tokens, 100);
    }

    #[test]
    fn parses_fenced_json_with_prose() {
        let raw = "好的，这是更新后的纪要：\n```json\n{\"live\":\"在讲预算\",\"topics\":[\"预算\"]}\n```\n希望有帮助。";
        let patch = parse_summary_patch(raw).unwrap();
        assert_eq!(patch.live.unwrap(), "在讲预算");
    }

    #[test]
    fn unwraps_nested_result() {
        let raw = r#"{"result":{"live":"嵌套内容"}}"#;
        let patch = parse_summary_patch(raw).unwrap();
        assert_eq!(patch.live.unwrap(), "嵌套内容");
    }

    #[test]
    fn handles_unescaped_braces_inside_strings() {
        let raw = r#"{"live":"他说「{这个}要改」","topics":["改动"]}"#;
        let patch = parse_summary_patch(raw).unwrap();
        assert_eq!(patch.live.unwrap(), "他说「{这个}要改」");
    }

    #[test]
    fn action_items_accept_plain_strings() {
        let raw = r#"{"actionItems":["出埋点方案","找财务确认"]}"#;
        let patch = parse_summary_patch(raw).unwrap();
        let mut st = SummaryState::default();
        apply_patch(&mut st, patch, "m", 0, &usage());
        assert_eq!(st.action_items.len(), 2);
        assert_eq!(st.action_items[0].text, "出埋点方案");
        assert!(st.action_items[0].owner.is_empty());
    }

    #[test]
    fn snake_case_keys_are_accepted() {
        let raw = r#"{"key_points":["a"],"action_items":[{"text":"b"}],"live":"x"}"#;
        let patch = parse_summary_patch(raw).unwrap();
        assert_eq!(patch.key_points.unwrap(), vec!["a"]);
        assert_eq!(patch.action_items.unwrap().len(), 1);
    }

    #[test]
    fn empty_lists_do_not_wipe_existing_data() {
        let mut st = SummaryState::default();
        let full = parse_summary_patch(r#"{"keyPoints":["重要结论"],"decisions":["已决定"]}"#).unwrap();
        apply_patch(&mut st, full, "m", 0, &usage());
        let empty = parse_summary_patch(r#"{"keyPoints":[],"decisions":[]}"#).unwrap();
        apply_patch(&mut st, empty, "m", 5_000, &usage());
        assert_eq!(st.key_points, vec!["重要结论"]);
        assert_eq!(st.decisions, vec!["已决定"]);
        // 但时间戳与轮次仍然前进
        assert_eq!(st.covered_until_ms, 5_000);
        assert_eq!(st.revision, 2);
    }

    #[test]
    fn covered_until_never_goes_backwards() {
        let mut st = SummaryState::default();
        let p = parse_summary_patch(r#"{"live":"a"}"#).unwrap();
        apply_patch(&mut st, p, "m", 30_000, &usage());
        let p = parse_summary_patch(r#"{"live":"b"}"#).unwrap();
        apply_patch(&mut st, p, "m", 10_000, &usage());
        assert_eq!(st.covered_until_ms, 30_000);
    }

    #[test]
    fn summary_is_normalized_to_markdown_list() {
        let raw = r#"{"summary":"1. 营收增长\n* 留存下降\n已经达成的决定"}"#;
        let patch = parse_summary_patch(raw).unwrap();
        let mut st = SummaryState::default();
        apply_patch(&mut st, patch, "m", 0, &usage());
        for line in st.summary.lines() {
            assert!(line.starts_with("- "), "未规范化的行：{line}");
        }
        assert_eq!(st.summary.lines().count(), 3);
    }

    #[test]
    fn duplicate_items_are_deduped() {
        let raw = r#"{"keyPoints":["同一结论","同一结论","- 同一结论"]}"#;
        let patch = parse_summary_patch(raw).unwrap();
        let mut st = SummaryState::default();
        apply_patch(&mut st, patch, "m", 0, &usage());
        assert_eq!(st.key_points.len(), 1);
    }

    #[test]
    fn missing_json_reports_readable_error() {
        let err = parse_summary_patch("抱歉，我无法完成这个请求。").unwrap_err().to_string();
        assert!(err.contains("JSON"), "实际：{err}");
    }

    #[test]
    fn state_serializes_camel_case() {
        let mut st = SummaryState::default();
        st.key_points.push("要点".into());
        st.covered_until_ms = 1234;
        let v = serde_json::to_value(&st).unwrap();
        assert!(v.get("keyPoints").is_some());
        assert!(v.get("coveredUntilMs").is_some());
        assert!(v.get("key_points").is_none());
    }

    #[test]
    fn state_deserializes_partial_json_with_defaults() {
        let st: SummaryState = serde_json::from_str(r#"{"live":"只有这个"}"#).unwrap();
        assert_eq!(st.live, "只有这个");
        assert!(st.key_points.is_empty());
        assert_eq!(st.revision, 0);
    }

    #[test]
    fn action_items_text_includes_owner_and_due() {
        let mut st = SummaryState::default();
        st.action_items.push(ActionItem { text: "出方案".into(), owner: "张三".into(), due: "周三".into() });
        let t = st.action_items_text();
        assert!(t.contains("出方案") && t.contains("张三") && t.contains("周三"));
    }
}
