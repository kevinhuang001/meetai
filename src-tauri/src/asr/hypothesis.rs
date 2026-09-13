//! 文本假设合并（LocalAgreement 策略）。
//!
//! Whisper 不是流式模型：同一段话音每识别一次，结果都可能略有不同。
//! 如果直接把每次结果都显示出来，用户会看到文字不停跳动。
//!
//! 解决办法（来自 whisper_streaming 的 LocalAgreement-2 思路）：
//! 连续两次识别结果中**完全一致的前缀**才认为是可靠的，可以「定稿」；
//! 其余部分作为「未确认的尾巴」用灰色显示，下次识别可能被修正。
//!
//! 分词方式：中日韩字符按单字切分，拉丁字母/数字按单词切分（避免把半个英文单词定稿），
//! 标点和空白不参与比较（否则模型多加一个逗号就会打断定稿）。

/// 一次合并的结果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Agreement {
    /// 已被两次识别共同确认的部分（相对本次输入文本的切片）
    pub committed: String,
    /// 仍未确认的尾巴
    pub tentative: String,
}

impl Agreement {
    pub fn full(&self) -> String {
        format!("{}{}", self.committed, self.tentative)
    }

    pub fn is_empty(&self) -> bool {
        self.committed.is_empty() && self.tentative.is_empty()
    }
}

/// 一个参与比较的 token：在原文里的字符区间 + 归一化后的文本
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    char_start: usize,
    char_end: usize,
    norm: String,
}

#[derive(Debug, Default)]
pub struct HypothesisBuffer {
    prev_tokens: Vec<Token>,
}

impl HypothesisBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 换一句话时清空历史（避免上一句的假设影响新句子）
    pub fn reset(&mut self) {
        self.prev_tokens.clear();
    }

    pub fn has_history(&self) -> bool {
        !self.prev_tokens.is_empty()
    }

    /// 用新的识别结果更新缓冲区，返回定稿前缀与未确认尾巴。
    pub fn update(&mut self, new_text: &str) -> Agreement {
        let tokens = tokenize(new_text);
        if tokens.is_empty() {
            self.prev_tokens = tokens;
            return Agreement::default();
        }

        // 最长公共 token 前缀
        let mut common = 0usize;
        while common < tokens.len()
            && common < self.prev_tokens.len()
            && tokens[common].norm == self.prev_tokens[common].norm
        {
            common += 1;
        }

        // 保留最后一个「已达成一致」的 token 不定稿。
        //
        // 原因：两次识别结果完全相同时，最长公共前缀就是全文，尾巴为空 ——
        // 用户会看到整句一下子变黑，没有任何"正在识别"的反馈。
        // 留一个 token 的余量，UI 上就始终有一段灰色尾巴在跳动。
        let commit_tokens = common.saturating_sub(1);

        // 公共前缀的字符边界（取最后一个公共 token 的结束位置）
        let boundary = if commit_tokens == 0 {
            0
        } else {
            tokens[commit_tokens - 1].char_end
        };
        // 把边界之后紧邻的标点与空白一并算进定稿：
        // 标点不参与比较，所以「18」定稿时后面的「%」也应该一起定稿，
        // 否则灰色尾巴会以标点开头，观感很怪。
        let boundary = extend_over_non_word(new_text, boundary);

        self.prev_tokens = tokens;

        let chars: Vec<char> = new_text.chars().collect();
        let committed: String = chars[..boundary.min(chars.len())].iter().collect();
        let tentative: String = chars[boundary.min(chars.len())..].iter().collect();

        Agreement {
            committed: committed.trim_end().to_string(),
            tentative: tentative.trim_start().to_string(),
        }
    }
}

/// 从 `from` 开始，吞掉后续所有「非词字符」（标点与空白），遇到下一个词字符停止
fn extend_over_non_word(text: &str, from: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = from;
    while i < chars.len() && !is_word_char(chars[i]) {
        i += 1;
    }
    i
}

/// 分词：CJK 表意文字/假名按单字切分，拉丁字母与数字按词切分，标点与空白不参与比较。
///
/// 注意：不能直接用 `char::is_alphanumeric()` 判断 CJK —— 它对汉字也返回 true，
/// 会把「今天我们讨论」整串当成一个词，导致任何细微改动都无法定稿。
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if is_cjk_ideograph(c) {
            tokens.push(Token {
                char_start: i,
                char_end: i + 1,
                norm: c.to_lowercase().to_string(),
            });
            i += 1;
        } else if is_latin_word_char(c) {
            let start = i;
            while i < chars.len() && is_latin_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(Token {
                char_start: start,
                char_end: i,
                norm: word.to_lowercase(),
            });
        } else {
            i += 1;
        }
    }
    tokens
}

/// 参与比较的字符（CJK 单字或拉丁词字符）
fn is_word_char(c: char) -> bool {
    is_cjk_ideograph(c) || is_latin_word_char(c)
}

/// 汉字、假名等「无空格分词」的表意文字
fn is_cjk_ideograph(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF |   // 平假名 / 片假名
        0x3400..=0x4DBF |   // 扩展 A
        0x4E00..=0x9FFF |   // 基本区
        0xF900..=0xFAFF     // 兼容表意
    )
}

/// 拉丁字母、数字，以及词内连接符（韩文按空格分词，归到这里）
fn is_latin_word_char(c: char) -> bool {
    (c.is_alphanumeric() && !is_cjk_ideograph(c)) || c == '\'' || c == '-' || c == '_'
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_hypothesis_commits_nothing() {
        let mut hb = HypothesisBuffer::new();
        let a = hb.update("今天我们讨论一下");
        assert_eq!(a.committed, "");
        assert_eq!(a.tentative, "今天我们讨论一下");
        assert!(hb.has_history());
    }

    #[test]
    fn identical_hypothesis_keeps_a_visible_tail() {
        // 两次结果完全一致时，最长公共前缀就是全文；但我们故意少定稿一个 token，
        // 这样 UI 上始终有一段灰色尾巴，用户能看到"正在识别"而不是文字忽然变黑。
        let mut hb = HypothesisBuffer::new();
        hb.update("今天我们讨论一下");
        let a = hb.update("今天我们讨论一下");
        assert_eq!(a.committed, "今天我们讨论一");
        assert_eq!(a.tentative, "下");
        assert_eq!(a.full(), "今天我们讨论一下");
    }

    #[test]
    fn tail_is_never_empty_when_there_is_text() {
        // 这是「边说边出字」的观感保证：只要识别出了内容，就必须有未确认尾巴
        let mut hb = HypothesisBuffer::new();
        hb.update("季度复盘会议");
        for text in [
            "季度复盘会议",
            "季度复盘会议开始",
            "季度复盘会议开始了",
            "季度复盘会议开始了我们",
        ] {
            let a = hb.update(text);
            assert!(
                !a.tentative.is_empty(),
                "「{text}」的尾巴不应为空：{a:?}"
            );
            assert_eq!(a.full(), text);
        }
    }

    #[test]
    fn growing_hypothesis_commits_only_the_stable_prefix() {
        let mut hb = HypothesisBuffer::new();
        hb.update("今天我们讨论三个议题");
        let a = hb.update("今天我们讨论三个议题第一个是增长");
        // 最后一个一致 token（"题"）保留在尾巴里
        assert_eq!(a.committed, "今天我们讨论三个议");
        assert_eq!(a.tentative, "题第一个是增长");
        assert_eq!(a.full(), "今天我们讨论三个议题第一个是增长");
    }

    #[test]
    fn corrected_tail_is_not_committed() {
        let mut hb = HypothesisBuffer::new();
        hb.update("我们这季度增长了百分之十八");
        let a = hb.update("我们这季度增长了百分之八十");
        // 「百分之」之前一致，之后的分歧不能定稿（"之"作为最后一个一致 token 留在尾巴里）
        assert_eq!(a.committed, "我们这季度增长了百分");
        assert_eq!(a.tentative, "之八十");
    }

    #[test]
    fn punctuation_differences_do_not_break_agreement() {
        let mut hb = HypothesisBuffer::new();
        hb.update("我们先看复盘");
        let a = hb.update("我们先看复盘，然后是目标");
        assert_eq!(a.committed, "我们先看复");
        assert!(a.tentative.contains("盘，然后是目标"), "实际：{:?}", a);
    }

    #[test]
    fn english_words_are_not_committed_mid_word() {
        let mut hb = HypothesisBuffer::new();
        hb.update("we should review the quart");
        let a = hb.update("we should review the quarterly report");
        // 不能把半个单词 "quart" 定稿成 "quart" 再接上 "erly"
        assert_eq!(a.committed, "we should review");
        assert_eq!(a.tentative, "the quarterly report");
    }

    #[test]
    fn english_word_agreement_works_across_calls() {
        let mut hb = HypothesisBuffer::new();
        hb.update("we should review the quarterly report");
        let a = hb.update("we should review the quarterly report and goals");
        assert_eq!(a.committed, "we should review the quarterly");
        assert_eq!(a.tentative, "report and goals");
    }

    #[test]
    fn empty_input_is_handled() {
        let mut hb = HypothesisBuffer::new();
        hb.update("有内容");
        let a = hb.update("");
        assert!(a.is_empty());
        let a = hb.update("   ");
        assert!(a.is_empty());
    }

    #[test]
    fn reset_clears_history() {
        let mut hb = HypothesisBuffer::new();
        hb.update("第一句话");
        hb.reset();
        assert!(!hb.has_history());
        let a = hb.update("第一句话");
        assert_eq!(a.committed, "");
    }

    #[test]
    fn committed_never_starts_with_space() {
        let mut hb = HypothesisBuffer::new();
        hb.update("hello world");
        let a = hb.update("hello world again");
        assert_eq!(a.committed, "hello");
        assert_eq!(a.tentative, "world again");
        assert!(!a.tentative.starts_with(' '), "尾巴不应以空格开头：{:?}", a.tentative);
    }

    #[test]
    fn mixed_chinese_english_and_numbers() {
        let mut hb = HypothesisBuffer::new();
        hb.update("我们的 Q3 营收增长 18%");
        let a = hb.update("我们的 Q3 营收增长 18%，主要来自企业版");
        // 最后一个一致的 token（"18"）留在尾巴里，逗号不参与比较
        assert_eq!(a.committed, "我们的 Q3 营收增长");
        assert!(a.tentative.starts_with("18%"), "实际：{:?}", a.tentative);
        assert!(a.tentative.contains("主要来自企业版"));
    }

    #[test]
    fn punctuation_right_after_a_committed_token_is_included() {
        let mut hb = HypothesisBuffer::new();
        hb.update("营收增长 18%");
        let a = hb.update("营收增长 18%主要来自企业版");
        assert_eq!(a.committed, "营收增长");
        assert_eq!(a.tentative, "18%主要来自企业版");
    }

    #[test]
    fn case_insensitive_for_latin() {
        let mut hb = HypothesisBuffer::new();
        hb.update("we use Tauri");
        let a = hb.update("We use Tauri for desktop");
        assert_eq!(a.committed, "We use");
    }

    #[test]
    fn tokenizer_skips_punctuation_only() {
        let t = tokenize("你好，world 123！");
        let norms: Vec<&str> = t.iter().map(|x| x.norm.as_str()).collect();
        assert_eq!(norms, vec!["你", "好", "world", "123"]);
        // 字符区间要能正确切回原文
        let chars: Vec<char> = "你好，world 123！".chars().collect();
        let last = t.last().unwrap();
        let last_word: String = chars[last.char_start..last.char_end].iter().collect();
        assert_eq!(last_word, "123");
    }
}
