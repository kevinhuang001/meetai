//! 小工具函数集合。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 Unix 毫秒时间戳
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把毫秒格式化成 `HH:MM:SS`，用于转写时间戳
pub fn format_clock(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    format!("{:02}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
}

/// 把毫秒格式化成 `MM:SS`（不足一小时时更清爽）
pub fn format_short(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    if total >= 3600 {
        format_clock(ms)
    } else {
        format!("{:02}:{:02}", total / 60, total % 60)
    }
}

/// 文件名里不能出现的字符统统替换掉
pub fn sanitize_filename(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// 按「字符数」截断，中文不会被切坏
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// 保留头尾的截断：长转写送给模型时，头尾都比中间重要
pub fn truncate_middle(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let head = max_chars * 2 / 3;
    let tail = max_chars - head;
    let head_str: String = s.chars().take(head).collect();
    let tail_str: String = s.chars().skip(total - tail).collect();
    format!("{head_str}\n……（中间省略 {total} 字中的 {} 字）……\n{tail_str}", total - max_chars)
}

/// 声音振幅转 dBFS，0 为满量程
pub fn amplitude_db(rms: f32) -> f32 {
    20.0 * (rms.max(1e-9)).log10()
}

/// 把 0~1 的线性音量映射成更好看的 UI 比例
pub fn level_to_display(rms: f32) -> f32 {
    // -60dB..0dB 映射到 0..1
    let db = amplitude_db(rms).clamp(-60.0, 0.0);
    ((db + 60.0) / 60.0).powf(1.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_format() {
        assert_eq!(format_clock(0), "00:00:00");
        assert_eq!(format_clock(3_661_000), "01:01:01");
        assert_eq!(format_short(65_000), "01:05");
        assert_eq!(format_short(3_661_000), "01:01:01");
    }

    #[test]
    fn truncation_is_char_safe() {
        let s = "会议纪要内容";
        assert_eq!(truncate_chars(s, 3), "会议纪…");
        assert_eq!(truncate_chars(s, 99), s);
    }

    #[test]
    fn middle_truncation_keeps_both_ends() {
        let s: String = (0..100).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let out = truncate_middle(&s, 20);
        assert!(out.starts_with(&s[..1]));
        assert!(out.contains("省略"));
        assert!(out.ends_with(&s[s.len() - 1..]));
    }

    #[test]
    fn filename_sanitize() {
        assert_eq!(sanitize_filename("2024/03/01 周会:复盘", "会议"), "2024_03_01 周会_复盘");
        assert_eq!(sanitize_filename("   ", "会议"), "会议");
    }

    #[test]
    fn db_math() {
        assert!((amplitude_db(1.0) - 0.0).abs() < 1e-4);
        assert!((amplitude_db(0.001) + 60.0).abs() < 0.01);
        assert!(level_to_display(1.0) > 0.99);
        assert!(level_to_display(0.0) < 0.01);
    }
}
