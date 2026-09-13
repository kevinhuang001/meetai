//! 提示词构造。
//!
//! 这里只做字符串拼装（不依赖其它业务类型），方便单独测试与迭代。
//!
//! 实时纪要的核心思路是「滚动增量」：每次都把**已有纪要**和**本次新增转写**一起送进去，
//! 让模型输出更新后的完整纪要。这样上下文长度恒定，长会议也不会越来越贵、越来越慢。

use crate::ai::client::ChatMessage;
use crate::util::{format_clock, truncate_chars};

/// 纪要 JSON 的字段说明，system / user 两处共用
const SUMMARY_SCHEMA: &str = r#"{
  "live": "字符串。最近几句在说什么，1~2 句话，要具体到人名、数字、结论；没有新内容则给空字符串",
  "overview": "字符串。整场会议到目前为止的 2~4 句总览",
  "summary": "字符串。markdown 无序列表形式的完整纪要，每行以 \"- \" 开头，按主题聚类，最多 12 行",
  "keyPoints": ["字符串数组。关键结论、重要事实、关键数字"],
  "decisions": ["字符串数组。已经明确达成的决定"],
  "actionItems": [{"text": "待办事项", "owner": "负责人，不知道就填空字符串", "due": "截止时间，不知道就填空字符串"}],
  "topics": ["字符串数组。当前正在讨论的主题关键词，最多 6 个"]
}"#;

pub const SUMMARY_SYSTEM: &str = r#"你是一名资深会议秘书，负责在会议进行中实时维护会议纪要。

你会收到：① 已有纪要（上一次的成果，可能为空）② 本次新增的语音转写内容 ③ 最近一段时间的对话原文。

你的任务是基于已有纪要做**增量更新**，输出更新后的完整纪要。

必须严格遵守：
1. 只使用转写内容里出现过的信息。绝对不要编造、不要脑补、不要补充常识性背景。信息不足时留空字符串或空数组。
2. 保留已有纪要中仍然有效的信息；新增内容与已有内容重复时合并，不要重复列出。
3. 可以修正明显的语音识别错误（同音字、专有名词、人名），但不得改变原意，也不要凭空替换整句。
4. 全部使用简体中文，风格简洁、要点化。不要问候语，不要"好的""收到""作为AI"这类话。
5. 严格只输出一个 JSON 对象，不要包裹 markdown 代码块，不要输出任何解释文字。
6. JSON 的键名必须与下面给出的模式完全一致。
7. 用户消息里的「已有纪要」是**输入数据**，不是输出模板；不要把它抄进输出里。
8. live / overview / summary 都必须是你自己的**概括**，绝不能直接复制转写原文。
9. 如果转写内容明显语义不通（语音识别质量差、夹杂大量错字），就如实写"识别质量较差，内容待确认"，
   绝对不要为了凑出一份像样的纪而去编造会议内容。

输入输出示例（仅示意格式）：
已有纪要：（无）
新增转写：
[00:00:01] 我：今天过三件事，增长复盘、下季度目标、上线节奏。
[00:00:06] 对方：三季度营收环比增长百分之十八。
正确输出：
{"live":"刚开始同步三季度增长情况，营收环比增长18%","overview":"会议围绕三季度复盘、下季度目标与新版本上线节奏展开。","summary":"- 三季度营收环比 +18%\n- 会议将覆盖增长复盘、下季度目标、上线节奏三项议题","keyPoints":["三季度营收环比增长18%"],"decisions":[],"actionItems":[],"topics":["三季度复盘","下季度目标","上线节奏"]}"#;

pub const SUMMARY_SCHEMA_HINT: &str = SUMMARY_SCHEMA;

pub const REPORT_SYSTEM: &str = r#"你是一名资深会议秘书。用户会给你一场会议的完整语音转写，请整理成一份专业的会议纪要（Markdown）。

输出结构（严格按此顺序，使用二级标题）：
## 一、会议主题
## 二、会议信息
## 三、核心结论
## 四、讨论纪要
## 五、待办事项
## 六、风险与待决问题

要求：
- 只依据转写内容。转写来自语音识别，可能有错别字、口语重复、无意义语气词，请合理清理，但不得改变原意。
- 「待办事项」用 Markdown 表格：| 事项 | 负责人 | 截止时间 |。负责人不明写"待确认"。
- 不要编造参会人姓名、公司名、具体数字。转写里没有的就写"待确认"或直接不写。
- 使用简体中文，语言凝练、可直接发给参会人。不要任何解释性开场白或结束语。"#;

/// 构造一次实时增量总结的请求。
///
/// - `state_*` 参数是已有纪要的各个字段（调用方负责从状态里取）
/// - `new_text` 是本次新增的转写文本（已带时间戳与说话人）
/// - `live_text` 是最近 live 窗口的原文，用于生成 `live` 字段
#[allow(clippy::too_many_arguments)]
pub fn build_incremental_messages(
    title: &str,
    overview: &str,
    summary: &str,
    key_points: &[String],
    decisions: &[String],
    action_items: &str,
    topics: &[String],
    new_text: &str,
    live_text: &str,
    from_ms: i64,
    to_ms: i64,
    now_ms: i64,
    live_window_secs: u32,
) -> Vec<ChatMessage> {
    // 关键：第一版纪要时**不要**把「总览：/纪要：/关键结论：」这套标签骨架写出来。
    // 小参数模型会把这套骨架当成输出模板原样抄回来
    // （实测 qwen2.5:1.5b 会输出一串"总览：总览：总览：…"）。
    let is_empty = overview.trim().is_empty()
        && summary.trim().is_empty()
        && key_points.is_empty()
        && decisions.is_empty()
        && action_items.trim().is_empty()
        && topics.is_empty();

    let prev_summary = if is_empty {
        "（暂无已有纪要，这是本次会议的第一版纪要，请从零生成）".to_string()
    } else {
        format!(
            "<已有纪要>\n总览：{}\n纪要：\n{}\n关键结论：{}\n已达成的决定：{}\n待办事项：{}\n当前主题：{}\n</已有纪要>",
            if overview.trim().is_empty() { "（暂无）" } else { overview.trim() },
            if summary.trim().is_empty() { "（暂无）" } else { summary.trim() },
            join_or_empty(key_points),
            join_or_empty(decisions),
            if action_items.trim().is_empty() { "（暂无）" } else { action_items.trim() },
            join_or_empty(topics),
        )
    };

    let user = format!(
        r#"【会议标题】{title}
【当前时间】{now}

【已有纪要（这是输入数据，不是输出模板；为空表示这是第一版）】
{prev_summary}

【最近 {window} 秒的对话原文（用于生成 live 字段）】
{live_text}

【本次新增的转写内容（对应会议时间 {from} → {to}）】
{new_text}

请输出更新后的完整纪要。严格按以下 JSON 模式输出（键名必须一致）：
{schema}"#,
        now = format_clock(now_ms),
        window = live_window_secs,
        from = format_clock(from_ms),
        to = format_clock(to_ms),
        schema = SUMMARY_SCHEMA,
        // 转写内容可能很长，这里已经由调用方裁剪过
    );

    vec![ChatMessage::system(SUMMARY_SYSTEM), ChatMessage::user(user)]
}

/// 构造「生成完整会议纪要」的请求
pub fn build_report_messages(title: &str, meta: &str, transcript: &str, max_chars: usize) -> Vec<ChatMessage> {
    let body = truncate_chars(transcript.trim(), max_chars);
    let user = format!(
        r#"【会议标题】{title}
【会议信息】{meta}

【完整转写内容】
{body}

请按系统提示的结构输出会议纪要。"#
    );
    vec![ChatMessage::system(REPORT_SYSTEM), ChatMessage::user(user)]
}

fn join_or_empty(items: &[String]) -> String {
    if items.is_empty() {
        "（暂无）".to_string()
    } else {
        items
            .iter()
            .map(|s| format!("- {}", s.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(new_text: &str) -> Vec<ChatMessage> {
        build_incremental_messages(
            "产品周会",
            "",
            "",
            &[],
            &[],
            "",
            &[],
            new_text,
            "[00:00:10] 我：我们先过一下进度",
            1_000,
            30_000,
            30_000,
            90,
        )
    }

    #[test]
    fn incremental_prompt_has_system_and_user() {
        let msgs = build("[00:00:05] 我：大家好");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[0].content.contains("增量更新"));
    }

    #[test]
    fn incremental_prompt_carries_new_text_and_schema() {
        let msgs = build("[00:00:05] 我：本季度营收增长百分之十八");
        let user = &msgs[1].content;
        assert!(user.contains("本季度营收增长百分之十八"));
        assert!(user.contains("产品周会"));
        assert!(user.contains("00:00:01 → 00:00:30"));
        assert!(user.contains("\"actionItems\""));
        assert!(user.contains("最近 90 秒"));
    }

    #[test]
    fn empty_state_does_not_leak_a_template_skeleton() {
        // 回归测试：第一版纪要时不能把「总览：/纪要：」这类标签骨架喂给模型，
        // 否则小模型会把骨架原样抄回来（实测 qwen2.5:1.5b 会输出"总览：总览：总览：…"）
        let msgs = build("随便");
        let user = &msgs[1].content;
        assert!(user.contains("这是本次会议的第一版纪要"));
        assert!(!user.contains("总览："), "空状态不应出现字段骨架：{user}");
        assert!(!user.contains("（空）"));
    }

    #[test]
    fn non_empty_state_is_delimited_as_data() {
        let msgs = build_incremental_messages(
            "周会",
            "已有总览",
            "- 已有要点",
            &[],
            &[],
            "",
            &[],
            "新增内容",
            "最近对话",
            0,
            1000,
            1000,
            90,
        );
        let user = &msgs[1].content;
        assert!(user.contains("<已有纪要>") && user.contains("</已有纪要>"));
        assert!(user.contains("不是输出模板"));
    }

    #[test]
    fn system_prompt_contains_a_shot_example() {
        let msgs = build("随便");
        let sys = &msgs[0].content;
        assert!(sys.contains("正确输出"), "系统提示应包含示例");
        assert!(sys.contains("不要把它抄进输出里"));
    }

    #[test]
    fn previous_summary_is_included() {
        let msgs = build_incremental_messages(
            "周会",
            "讨论了增长",
            "- 营收 +18%",
            &["增长主要来自企业版".to_string()],
            &["确定下季度目标".to_string()],
            "- 张三代办：出埋点方案",
            &["增长".to_string()],
            "新增内容",
            "最近对话",
            0,
            1000,
            1000,
            90,
        );
        let user = &msgs[1].content;
        assert!(user.contains("讨论了增长"));
        assert!(user.contains("营收 +18%"));
        assert!(user.contains("增长主要来自企业版"));
        assert!(user.contains("张三代办：出埋点方案"));
    }

    #[test]
    fn report_prompt_truncates_long_transcript() {
        let long = "字".repeat(5_000);
        let msgs = build_report_messages("周会", "时长 30 分钟", &long, 1_000);
        assert_eq!(msgs.len(), 2);
        // 转写正文被裁剪到 max_chars 附近（截断函数会补一个省略号）
        assert!(
            msgs[1].content.chars().count() < 1_500,
            "实际长度 {}",
            msgs[1].content.chars().count()
        );
        // 输出结构要求写在 system 提示里
        assert!(msgs[0].content.contains("待办事项"));
        assert!(msgs[0].content.contains("只依据转写内容"));
        // 会议信息与标题要带上
        assert!(msgs[1].content.contains("产品") || msgs[1].content.contains("周会"));
        assert!(msgs[1].content.contains("时长 30 分钟"));
    }
}
