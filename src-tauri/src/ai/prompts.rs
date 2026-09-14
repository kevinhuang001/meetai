//! 提示词构造。
//!
//! 这里只做字符串拼装（不依赖其它业务类型），方便单独测试与迭代。
//!
//! 纪要的核心思路是「滚动增量」：每次都把**已有纪要**和**本次新增转写**一起送进去，
//! 让模型输出更新后的完整纪要。这样上下文长度恒定，长会议也不会越来越贵、越来越慢。
//!
//! **纪要总结的是「已经说过去的那段时间」**，不是实时字幕：转写负责实时，
//! 纪要负责回头看。所以提示词里给的是时间区间（from → to），不是「正在说」。
//!
//! 会议与讲座要抓的东西完全不同，因此有两套 system 提示词（见 [`SummaryMode`]）。

use crate::ai::client::ChatMessage;
use crate::util::{format_clock, truncate_chars};

/// 传给提示词的已有分段（避免提示词层依赖业务类型）
#[derive(Debug, Clone)]
pub struct SectionLine {
    pub title: String,
    pub points: Vec<String>,
}

/// 纪要模式：同一份 JSON 结构，两种场景下字段含义不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryMode {
    /// 会议：要决定、要待办、要负责人
    Meeting,
    /// 讲座 / 课程：要知识点、要概念脉络、要复习项
    Lecture,
}

impl SummaryMode {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "lecture" | "talk" | "course" | "讲座" | "课程" => SummaryMode::Lecture,
            _ => SummaryMode::Meeting,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SummaryMode::Meeting => "meeting",
            SummaryMode::Lecture => "lecture",
        }
    }

    fn schema(&self) -> &'static str {
        match self {
            SummaryMode::Meeting => SUMMARY_SCHEMA,
            SummaryMode::Lecture => SUMMARY_SCHEMA_LECTURE,
        }
    }

    fn system_prompt(&self) -> &'static str {
        match self {
            SummaryMode::Meeting => SUMMARY_SYSTEM_MEETING,
            SummaryMode::Lecture => SUMMARY_SYSTEM_LECTURE,
        }
    }

    /// 「已有纪要」在 user 消息里的字段名，两种模式用词不同
    fn labels(&self) -> (&'static str, &'static str, &'static str, &'static str, &'static str) {
        match self {
            SummaryMode::Meeting => ("会议总览", "已分段纪要", "关键结论", "已达成的决定", "待办事项"),
            SummaryMode::Lecture => ("内容脉络", "已分模块笔记", "核心概念", "讲者强调的结论", "课后要做的事"),
        }
    }
}

/// 讲座模式的 JSON 说明：键名与会议模式完全一致（前端契约不变），
/// 但每个键**描述的含义**按讲座场景来写，免得模型把会议话术套到课堂上。
const SUMMARY_SCHEMA_LECTURE: &str = r#"{
  "live": "字符串。最近这一段在讲什么，1~2 句话，要具体到术语与结论；没有新内容则给空字符串",
  "overview": "字符串。这场讲座到目前为止的 2~4 句脉络总览",
  "sections": [{"title": "知识模块标题", "points": ["该模块讲到的知识点"], "untilMs": 0}],
  "keyPoints": ["字符串数组。核心概念、定义、定理、公式、重要数据"],
  "decisions": ["字符串数组。讲者明确强调的结论、易错点、重要提醒"],
  "actionItems": [{"text": "课后需要复习、练习或查阅的点", "owner": "留空字符串", "due": "留空字符串"}],
  "topics": ["字符串数组。当前讲解的知识模块关键词，最多 6 个"]
}"#;

/// 纪要 JSON 的字段说明，system / user 两处共用
const SUMMARY_SCHEMA: &str = r#"{
  "live": "字符串。最近几句在说什么，1~2 句话，要具体到人名、数字、结论；没有新内容则给空字符串",
  "overview": "字符串。整场会议到目前为止的 2~4 句总览",
  "sections": [{"title": "分段标题", "points": ["该段的要点"], "untilMs": 0}],
  "keyPoints": ["字符串数组。关键结论、重要事实、关键数字"],
  "decisions": ["字符串数组。已经明确达成的决定"],
  "actionItems": [{"text": "待办事项", "owner": "负责人，不知道就填空字符串", "due": "截止时间，不知道就填空字符串"}],
  "topics": ["字符串数组。当前正在讨论的主题关键词，最多 6 个"]
}"#;

/// 会议模式：抓决定、待办、结论
pub const SUMMARY_SYSTEM_MEETING: &str = r#"你是一名资深会议秘书，负责在会议进行中**滚动维护**一份会议纪要。

注意：你不是在写实时字幕。转写负责实时出字，你负责**回头看已经说过的那一段**，
把它整理成结论性内容。所以你收到的永远是「一段时间区间内的新增内容」。

你会收到：① 已有纪要（上一次的成果，可能为空）② 本次新增的语音转写内容 ③ 最近一段时间的对话原文。

你的任务是基于已有纪要做**增量更新**，输出更新后的完整纪要。

必须严格遵守：
0. **绝对不要照抄解释文字或任何示例**。输出里的每一个词都必须来自用户消息里的转写内容。
   本提示词只用来说明规则，里面出现的任何具体说法都不是给你的素材。
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
10. **绝对不要重复**：同一个要点只能出现一次。如果信息很少，就少写几条，
    宁可比要求的更短，也不要靠复读凑长度。整个 JSON 控制在 800 字以内。

分段规则（sections，最重要）：
- 纪要要呈现的是**从会议开始到现在的全部内容**，不是只有最近一段。
- 按议题/阶段把内容切成若干段，每段一个短标题（4~12 字，如「三季度复盘」「上线节奏」）。
- **每次都必须输出完整的分段列表**：包含之前已有的所有段 + 这次新增或变化的内容。
  「已有纪要」里的段要原样带上（可以改标题用词，但不能丢内容）。
- 内容变多时**重新分段**：同一议题的内容要合进同一段；如果一段里混进了明显不同的话题，
  就把它拆成两段。分段的粒度以「打开纪要的人能快速找到他关心的那块」为准。
- 一段要点 2~6 条，写结论和关键数字，不要写流水账。
- 已结束的旧话题保持简短，把要点留给当前正在讨论的内容。
- untilMs 填这一段最后一条内容对应的会议时间（毫秒）；不确定就填 0。

字段含义（会议场景）：
- live：最近这一段（已过去的几十秒到几分钟）在说什么，1~2 句，要具体到人名、数字、结论
- overview：整场会议到目前为止的 2~4 句总览
- sections：**最重要**，见下面「分段规则」。整份笔记的内容都在这里。。整份纪要的内容都在这里。
- keyPoints：关键结论、重要事实、关键数字
- decisions：已经明确达成的决定（没达成就是空数组）
- actionItems：待办，尽量带负责人与截止时间；不确定就留空字符串
- topics：当前讨论的主题关键词，最多 6 个

**不要照抄任何示例文本**：本提示词里出现的任何举例内容都与你的输入无关。
你只能依据用户消息里给出的转写内容来写，一个词都不许来自提示词本身。"#;

/// 讲座 / 课程模式：抓知识点、概念脉络、复习项
pub const SUMMARY_SYSTEM_LECTURE: &str = r#"你是一名专业的学习笔记整理者，负责在一场讲座 / 课程进行中**滚动维护**一份知识笔记。

注意：你不是在写实时字幕。转写负责实时出字，你负责**回头看已经讲过的那一段**，
把它整理成结构化的知识内容。所以你收到的永远是「一段时间区间内的新增内容」。

你会收到：① 已有笔记（上一次的成果，可能为空）② 本次新增的语音转写内容 ③ 最近一段时间的讲解原文。

你的任务是基于已有笔记做**增量更新**，输出更新后的完整笔记。

必须严格遵守：
0. **绝对不要照抄解释文字或任何示例**。输出里的每一个词都必须来自用户消息里的转写内容。
   本提示词只用来说明规则，里面出现的任何具体说法都不是给你的素材。
1. 只使用转写内容里出现过的信息。绝对不要补充讲者没讲过的知识、不要脑补教材内容、不要自行推导结论。
2. 保留已有笔记中仍然有效的内容；重复讲的合并，不要重复列出。
3. 修正明显的语音识别错误（同音字、专业术语、人名、数字），但不得改变原意。
4. 全部使用简体中文，风格像一份可以直接复习的笔记：概念清楚、层次分明。
5. 严格只输出一个 JSON 对象，不要包裹 markdown 代码块，不要输出任何解释文字。
6. JSON 的键名必须与下面给出的模式完全一致。
7. 用户消息里的「已有笔记」是**输入数据**，不是输出模板；不要把它抄进输出里。
8. live / overview / summary 都必须是概括，绝不能直接复制讲稿原文。
9. 讲者没讲完、语义不完整的地方，就如实略过或写"此处内容不完整"，
   绝对不要为了讲得通而编造内容。
10. **绝对不要重复**：同一个知识点只出现一次。信息少就少写，不要靠复读凑长度。
    整个 JSON 控制在 800 字以内。

分段规则（sections，最重要）：
- 笔记要呈现的是**从讲座开始到现在的全部内容**，不是只有最近一段。
- 按**知识模块**切段，每段一个短标题（4~12 字，如「注意力机制」「梯度消失」）。
- **每次都必须输出完整的分段列表**：包含之前已有的所有段 + 这次新增的知识点。
  「已有内容」里的段要原样带上，不能丢。
- 讲到新模块时新增一段；同一模块的补充内容追加到对应段里；模块划分不合理时重新分段。
- 一段 2~6 条，写知识点本身（定义、结论、公式、数字），不要写「讲者说了什么」这类元描述。
- untilMs 填这一段最后一条内容对应的讲解时间（毫秒）；不确定就填 0。

字段含义（讲座场景，注意与会议不同）：
- live：最近这一段（已过去的几十秒到几分钟）在讲什么，1~2 句，要具体到术语与结论
- overview：这场讲座到目前为止讲了什么、脉络是什么，2~4 句
- summary：markdown 无序列表，每行以 "- " 开头，**按知识模块聚类**的知识笔记，最多 12 行
- keyPoints：核心概念、定义、定理、公式、重要数据等需要记住的东西
- decisions：讲者明确强调过的结论、易错点、重要提醒（没有就是空数组）
- actionItems：课后需要复习、练习或查阅的点（没有就是空数组）
- topics：当前讲解的知识模块关键词，最多 6 个

**不要照抄任何示例文本**：本提示词里出现的任何举例内容都与你的输入无关。
你只能依据用户消息里给出的内容来写，一个词都不许来自提示词本身。"#;

pub const SUMMARY_SCHEMA_HINT: &str = SUMMARY_SCHEMA;

pub const SUMMARY_SYSTEM: &str = SUMMARY_SYSTEM_MEETING;

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
    mode: SummaryMode,
    title: &str,
    overview: &str,
    sections: &[SectionLine],
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
        && sections.is_empty()
        && summary.trim().is_empty()
        && key_points.is_empty()
        && decisions.is_empty()
        && action_items.trim().is_empty()
        && topics.is_empty();

    let (l_overview, l_summary, l_points, l_decisions, l_actions) = mode.labels();
    let sections_text = if sections.is_empty() {
        "（暂无）".to_string()
    } else {
        sections
            .iter()
            .map(|s| {
                format!(
                    "■ {}\n{}",
                    s.title,
                    s.points
                        .iter()
                        .map(|p| format!("  - {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let prev_summary = if is_empty {
        "（暂无已有内容，这是第一版，请从零生成）".to_string()
    } else {
        format!(
            "<已有内容>\n{l_overview}：{}\n{l_summary}（必须完整带到输出里）：\n{sections_text}\n{l_points}：{}\n{l_decisions}：{}\n{l_actions}：{}\n当前主题：{}\n</已有内容>",
            if overview.trim().is_empty() { "（暂无）" } else { overview.trim() },
            join_or_empty(key_points),
            join_or_empty(decisions),
            if action_items.trim().is_empty() { "（暂无）" } else { action_items.trim() },
            join_or_empty(topics),
        )
    };

    let user = format!(
        r#"【标题】{title}
【当前时间】{now}

【已有内容（这是输入数据，不是输出模板；为空表示这是第一版）】
{prev_summary}

【最近 {window} 秒的原文（用于生成 live 字段）】
{live_text}

【本次新增的转写内容（对应时间 {from} → {to}，这是已经过去的区间）】
{new_text}

请输出更新后的完整内容。严格按以下 JSON 模式输出（键名必须一致）：
{schema}"#,
        now = format_clock(now_ms),
        window = live_window_secs,
        from = format_clock(from_ms),
        to = format_clock(to_ms),
        schema = mode.schema(),
        // 转写内容可能很长，这里已经由调用方裁剪过
    );

    vec![ChatMessage::system(mode.system_prompt()), ChatMessage::user(user)]
}

/// 构造「生成完整会议纪要」的请求
pub fn build_report_messages(title: &str, meta: &str, transcript: &str, max_chars: usize) -> Vec<ChatMessage> {
    let body = truncate_chars(transcript.trim(), max_chars);
    let user = format!(
        r#"【标题】{title}
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
            SummaryMode::Meeting,
            "产品周会",
            "",
            &[],
            "",
            &[],
            &[],
            "",
            &[],
            new_text,
            "[00:00:10] 我们先过一下进度",
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
        assert!(user.contains("这是第一版"));
        assert!(!user.contains("总览："), "空状态不应出现字段骨架：{user}");
        assert!(!user.contains("（空）"));
    }

    #[test]
    fn lecture_mode_gets_its_own_prompt_and_labels() {
        // 讲座与会议要抓的东西不同：讲座不该出现「待办/负责人」这种会议话术，
        // 会议也不该被要求整理「知识笔记」。（下面用非空状态，字段名才会渲染出来）
        let lecture = build_incremental_messages(
            SummaryMode::Lecture,
            "机器学习导论",
            "已讲注意力机制的动机",
            &[SectionLine {
                title: "注意力机制".into(),
                points: vec!["循环网络长序列梯度消失".into()],
            }],
            "",
            &["梯度消失".to_string()],
            &[],
            "",
            &[],
            "[00:00:03] 今天我们讲注意力机制。",
            "",
            0,
            3_000,
            3_000,
            90,
        );
        let sys = &lecture[0].content;
        assert!(sys.contains("学习笔记"), "讲座模式 system 提示词不对：{}", &sys[..60]);
        assert!(sys.contains("知识笔记"));
        assert!(sys.contains("核心概念"));
        assert!(!sys.contains("会议秘书"), "讲座不该用会议秘书人设");

        let user = &lecture[1].content;
        assert!(user.contains("已分模块笔记"), "讲座模式的字段用词应不同：{user}");
        assert!(user.contains("核心概念"), "讲座模式应使用「核心概念」而不是「关键结论」");
        assert!(!user.contains("已分段纪要"), "讲座模式不该出现会议用词");
        assert!(!user.contains("待办事项"), "讲座模式不该出现待办骨架");

        // 会议模式保持原样
        let meeting = build_incremental_messages(
            SummaryMode::Meeting,
            "产品周会",
            "",
            &[],
            "",
            &[],
            &[],
            "",
            &[],
            "开会",
            "",
            0,
            1_000,
            1_000,
            90,
        );
        assert!(meeting[0].content.contains("会议秘书"));
        assert_eq!(SummaryMode::parse("lecture"), SummaryMode::Lecture);
        assert_eq!(SummaryMode::parse("讲座"), SummaryMode::Lecture);
        assert_eq!(SummaryMode::parse("随便"), SummaryMode::Meeting);
        assert_eq!(SummaryMode::Meeting.as_str(), "meeting");
    }

    #[test]
    fn summary_is_framed_as_a_past_time_window() {
        // 纪要不是实时字幕：提示词必须把它说明成「已经过去的一段时间」
        let msgs = build("内容");
        let user = &msgs[1].content;
        assert!(user.contains("已经过去的区间"), "应说明总结的是过去的时间段：{user}");
        assert!(msgs[0].content.contains("实时字幕"), "system 提示词应说明它不是实时字幕");
    }

    #[test]
    fn chat_retry_backoff_grows_then_caps() {
        use crate::ai::client::backoff_delay;
        assert_eq!(backoff_delay(1).as_secs(), 1);
        assert_eq!(backoff_delay(2).as_secs(), 2);
        assert_eq!(backoff_delay(3).as_secs(), 4);
        // 不要无限增长，否则用户要等很久才看到报错
        assert_eq!(backoff_delay(4).as_secs(), 8);
        assert_eq!(backoff_delay(9).as_secs(), 8);
    }

    #[test]
    fn non_empty_state_is_delimited_as_data() {
        let msgs = build_incremental_messages(
            SummaryMode::Meeting,
            "周会",
            "已有总览",
            &[],
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
        assert!(user.contains("<已有内容>") && user.contains("</已有内容>"));
        assert!(user.contains("不是输出模板"));
    }

    #[test]
    fn system_prompt_forbids_copying_itself() {
        // 血泪教训：提示词里放 few-shot 示例，小模型会把示例**原样抄进结果**。
        // 用户看到的就是一段跟他毫无关系的「示例内容」，还以为是程序坏了。
        // 所以：不给示例，并且明确禁止照抄提示词。
        for mode in [SummaryMode::Meeting, SummaryMode::Lecture] {
            let sys = mode.system_prompt();
            assert!(
                sys.contains("不要照抄") || sys.contains("绝对不要照抄"),
                "{:?} 的提示词必须明确禁止照抄",
                mode
            );
            assert!(
                !sys.contains("正确输出"),
                "{:?} 的提示词不应再包含 few-shot 示例（会被原样抄走）",
                mode
            );
        }
    }

    #[test]
    fn previous_summary_is_included() {
        let msgs = build_incremental_messages(
            SummaryMode::Meeting,
            "周会",
            "讨论了增长",
            &[],
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
