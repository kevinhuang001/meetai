//! 识别结果的后处理：清洗文本 + 丢弃幻听与退化输出。
//!
//! 这些检查不依赖任何识别引擎的实现细节，只针对「输出文本本身」，
//! 因此在换成任意 HTTP 识别服务后依然有效。

/// 已知的 Whisper 系模型在静音/纯音乐上会稳定输出的短语
const HALLUCINATION_PHRASES: &[&str] = &[
    "谢谢观看",
    "谢谢大家观看",
    "感谢观看",
    "谢谢收看",
    "请不吝点赞",
    "字幕由",
    "字幕组",
    "字幕志愿者",
    "明镜与点点栏目",
    "訂閱",
    "订阅频道",
    "点赞订阅",
    "下次再见",
    "我们下期再见",
    "thank you for watching",
    "thanks for watching",
    "please subscribe",
    "subtitles by",
    "subtitle by",
    "amara.org",
    "ming pao",
    "transcription by",
];

/// 清洗一段识别结果：
///  * 去掉首尾空白
///  * 去掉 `[BLANK_AUDIO]`、`(silence)` 这类非语音标记
///  * 去掉中日韩字符之间被模型插入的空格（`你 好` → `你好`）
pub fn clean_text(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return s;
    }
    s = strip_non_speech_markers(&s);
    s = remove_cjk_spaces(&s);
    s.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

/// 判断一段结果是否应当丢弃。
///
/// 判据（任一命中即丢弃）：
///  1. 有效字符太少（空文本、只有标点）
///  2. 全是已知幻听短语（短音频里）
///  3. 退化重复（整串是同一小段的重复，或句尾出现长片段重复）
pub fn is_likely_hallucination(text: &str, audio_ms: i64) -> bool {
    let stripped: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || is_cjk(*c))
        .collect();
    if stripped.chars().count() < 2 {
        return true;
    }

    let lower = text.to_lowercase();
    if audio_ms < 6_000 {
        let phrase_chars: usize = HALLUCINATION_PHRASES
            .iter()
            .filter(|p| lower.contains(*p))
            .map(|p| p.chars().count())
            .sum();
        if phrase_chars > 0 && phrase_chars * 2 >= stripped.chars().count() {
            return true;
        }
    }

    is_degenerate_repetition(&stripped)
}

/// 检出「同一小段不停重复」的退化输出。
///
/// Whisper 系模型有两种典型退化形态，都要挡：
///  1. 整句就是一个短单元的重复（"哈哈哈哈哈哈哈哈"、"一二三一二三一二三"）
///  2. **尾部循环**：长句结尾把前半句又重复一遍
fn is_degenerate_repetition(stripped: &str) -> bool {
    let chars: Vec<char> = stripped.chars().collect();
    let n = chars.len();
    if n < 8 {
        return false;
    }

    // 形态 1：整串由同一个短单元整除重复
    for period in 1..=(n / 3).max(1) {
        if n % period != 0 || n / period < 4 {
            continue;
        }
        let unit: String = chars[..period].iter().collect();
        if chars.chunks(period).all(|c| c.iter().collect::<String>() == unit) {
            return true;
        }
    }

    // 形态 2：尾部出现了足够长的连续两段重复
    let min_chunk = 8usize.max(n / 4);
    let mut chunk = min_chunk;
    while chunk * 2 <= n {
        let a: String = chars[n - chunk * 2..n - chunk].iter().collect();
        let b: String = chars[n - chunk..].iter().collect();
        if a == b {
            return true;
        }
        chunk += 1;
    }

    // 形态 3：某个 ≥5 字的片段反复出现且占比超过一半
    // （中文没有词边界，"百分之十五百分之十五百分之十五"这种循环靠形态 2 抓不到）
    const NGRAM: usize = 5;
    if n >= NGRAM * 3 {
        let mut counts: std::collections::HashMap<&[char], usize> = std::collections::HashMap::new();
        for i in 0..=n - NGRAM {
            *counts.entry(&chars[i..i + NGRAM]).or_insert(0) += 1;
        }
        let max_repeat = counts.values().copied().max().unwrap_or(0);
        if max_repeat >= 3 && max_repeat * NGRAM * 2 > n {
            return true;
        }
    }

    false
}

/// 去掉 `[...]` / `(...)` 形式的纯标记
fn strip_non_speech_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' || c == '(' || c == '（' {
            let close = match c {
                '[' => ']',
                '(' => ')',
                _ => '）',
            };
            if let Some(offset) = chars[i + 1..].iter().position(|&x| x == close) {
                let inner: String = chars[i + 1..i + 1 + offset].iter().collect();
                let inner_trim = inner.trim();
                let is_marker = !inner_trim.is_empty()
                    && (inner_trim
                        .chars()
                        .all(|x| x.is_ascii_uppercase() || x.is_ascii_digit() || x == '_' || x == ' ')
                        || matches!(
                            inner_trim.to_lowercase().as_str(),
                            "silence" | "music" | "blank_audio" | "inaudible" | "背景音乐" | "音乐"
                        ));
                if is_marker {
                    i += offset + 2;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 中日韩字符之间的空格通常来自模型，直接去掉；拉丁词之间保留。
///
/// 要按「连续空格」整体处理：删掉 `[BLANK_AUDIO]` 这类标记后会在汉字之间留下两个空格。
fn remove_cjk_spaces(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == ' ' {
            let start = i;
            while i < chars.len() && chars[i] == ' ' {
                i += 1;
            }
            let prev = if start > 0 { Some(chars[start - 1]) } else { None };
            let next = chars.get(i).copied();
            if let (Some(p), Some(n)) = (prev, next) {
                if is_cjk(p) && is_cjk(n) {
                    continue;
                }
            }
            out.push(' ');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F |   // CJK 标点
        0x3040..=0x30FF |   // 假名
        0x3400..=0x4DBF |   // 扩展 A
        0x4E00..=0x9FFF |   // 基本区
        0xF900..=0xFAFF |   // 兼容表意
        0xFF00..=0xFFEF     // 全角
    )
}

/// 两段文本拼接时是否需要补空格（中文之间不补，英文之间补）
pub fn needs_space_between(prev: &str, next: &str) -> bool {
    match (prev.chars().last(), next.chars().next()) {
        (Some(a), Some(b)) => !is_cjk(a) && !is_cjk(b) && a.is_alphanumeric() && b.is_alphanumeric(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_cjk_spaces() {
        assert_eq!(clean_text("你 好 ， 世 界"), "你好，世界");
        assert_eq!(clean_text("we  should  review"), "we should review");
        assert_eq!(clean_text("我们用 Tauri 开发"), "我们用 Tauri 开发");
    }

    #[test]
    fn clean_strips_non_speech_markers() {
        assert_eq!(clean_text("[BLANK_AUDIO]"), "");
        assert_eq!(clean_text("(silence)"), "");
        assert_eq!(clean_text("你好 [BLANK_AUDIO] 世界"), "你好世界");
        assert_eq!(clean_text("大家好（音乐）"), "大家好");
        assert_eq!(clean_text("这就是方案（第二个）"), "这就是方案（第二个）");
    }

    #[test]
    fn clean_trims_and_collapses() {
        assert_eq!(clean_text("   你好   世界  "), "你好世界");
        assert_eq!(clean_text("  we   should  review  "), "we should review");
        assert_eq!(clean_text(""), "");
    }

    #[test]
    fn hallucination_detected_on_short_audio() {
        assert!(is_likely_hallucination("谢谢观看", 1_500));
        assert!(is_likely_hallucination("谢谢大家观看，请不吝点赞订阅", 2_000));
        assert!(is_likely_hallucination("Thank you for watching!", 1_000));
        assert!(is_likely_hallucination("字幕由 Amara.org 社群提供", 2_500));
    }

    #[test]
    fn real_speech_is_not_flagged() {
        assert!(!is_likely_hallucination("我们下季度目标定在环比增长百分之十五", 3_000));
        assert!(!is_likely_hallucination("谢谢大家的支持是我们前进的动力", 4_000));
    }

    #[test]
    fn long_audio_mentioning_thanks_is_kept() {
        assert!(!is_likely_hallucination("谢谢观看", 20_000));
    }

    #[test]
    fn empty_and_punctuation_only_are_hallucinations() {
        assert!(is_likely_hallucination("", 5_000));
        assert!(is_likely_hallucination("。", 5_000));
        assert!(is_likely_hallucination("   ", 5_000));
        assert!(is_likely_hallucination("♪♪♪", 5_000));
    }

    #[test]
    fn degenerate_repetition_is_rejected() {
        assert!(is_likely_hallucination("哈哈哈哈哈哈哈哈", 5_000));
        assert!(is_likely_hallucination("一二三一二三一二三一二三", 5_000));
    }

    #[test]
    fn tail_loop_is_rejected() {
        let text = "aswhat you can do for your country as what you can do for your country";
        assert!(is_likely_hallucination(text, 8_000), "尾部循环没有被识别：{text}");
        let text2 = "季度目标定在百分之十五百分之十五百分之十五";
        assert!(is_likely_hallucination(text2, 8_000), "中文循环没有被识别：{text2}");
    }

    #[test]
    fn normal_long_sentence_is_not_flagged_as_loop() {
        let text = "我们今天主要过三件事情第一是上季度的增长复盘第二是下季度的目标第三是新版本的上线节奏";
        assert!(!is_likely_hallucination(text, 20_000));
        assert!(!is_likely_hallucination("谢谢大家谢谢大家", 20_000));
        // 同一个数字短语出现两次是正常表达
        assert!(!is_likely_hallucination("季度目标定在环比增长百分之十五同比增长百分之十五", 20_000));
    }

    #[test]
    fn needs_space_between_only_for_latin() {
        assert!(needs_space_between("hello", "world"));
        assert!(!needs_space_between("你好", "世界"));
        assert!(!needs_space_between("你好", "world"));
        assert!(!needs_space_between("hello", "世界"));
        assert!(!needs_space_between("18", "%"));
    }
}
