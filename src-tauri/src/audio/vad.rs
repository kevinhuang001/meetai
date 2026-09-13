//! 语音活动检测（VAD）。
//!
//! 纯 Rust 实现，**不依赖任何模型文件**：在滚动窗口上用帧能量的低分位数估计噪声底，
//! 再做滞回与形态学合并，并由 [`EnergyState`] 跨调用维护噪声底
//! （说话中冻结、句间重估）。
//!
//! 断句策略（按句定稿）就建立在这里的输出之上：见 `pipeline::Segmenter`。

use crate::settings::VadSettings;
use crate::util::amplitude_db;

/// 目标采样率
pub const RATE: u32 = 16_000;
/// 能量 VAD 的帧长（20ms）
const FRAME: usize = 320;
/// 绝对静音门槛：低于此音量的帧一律不算语音，避免安静房间里把底噪当语音
const ABSOLUTE_SILENCE_DB: f32 = -55.0;

/// 一段语音区间，单位毫秒，相对传入窗口的起点
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechSpan {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl SpeechSpan {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }
}

/// 帧能量统计
#[derive(Debug, Clone, Copy)]
pub struct FrameEnergy {
    pub rms: f32,
    pub peak: f32,
    pub db: f32,
}

pub fn analyze(samples: &[f32]) -> FrameEnergy {
    if samples.is_empty() {
        return FrameEnergy { rms: 0.0, peak: 0.0, db: -120.0 };
    }
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    for &s in samples {
        sum += (s as f64) * (s as f64);
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    let rms = (sum / samples.len() as f64).sqrt() as f32;
    FrameEnergy { rms, peak, db: amplitude_db(rms) }
}

/* ==========================================================================
 * 电平表（带自然衰减，UI 用）
 * ========================================================================== */

#[derive(Debug, Clone, Copy, Default)]
pub struct PeakMeter {
    peak: f32,
    rms: f32,
}

impl PeakMeter {
    /// `decay` 是每帧的衰减系数（0.85 左右观感较好）
    pub fn push(&mut self, energy: FrameEnergy, decay: f32) {
        self.peak = if energy.peak > self.peak { energy.peak } else { self.peak * decay };
        self.rms = if energy.rms > self.rms { energy.rms } else { self.rms * decay };
        if self.peak < 1e-5 {
            self.peak = 0.0;
        }
        if self.rms < 1e-5 {
            self.rms = 0.0;
        }
    }

    pub fn peak(&self) -> f32 {
        self.peak
    }

    pub fn rms(&self) -> f32 {
        self.rms
    }

    pub fn db(&self) -> f32 {
        amplitude_db(self.rms)
    }

    pub fn reset(&mut self) {
        self.peak = 0.0;
        self.rms = 0.0;
    }
}

/* ==========================================================================
 * VAD 引擎
 * ========================================================================== */

/// 能量 VAD 的自适应噪声底。
///
/// 策略是「**说话中冻结、句间重估**」：
/// * 不在说话时，用当前窗口帧能量的低分位数重新估计噪声底 —— 这样能自然跟随环境
///   变吵或变安静；
/// * 一旦进入说话状态就冻结噪声底 —— 否则一段没有停顿的连续语音会把噪声底一起抬上去，
///   语音反而再也检不出来（整段录音都不会断句）。
#[derive(Debug, Clone, Copy, Default)]
pub struct EnergyState {
    noise_db: Option<f32>,
}

impl EnergyState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已经建立了噪声底
    pub fn is_ready(&self) -> bool {
        self.noise_db.is_some()
    }

    pub fn noise_db(&self) -> f32 {
        self.noise_db.unwrap_or(-70.0)
    }

    /// 用窗口的低分位数重估噪声底
    pub fn observe(&mut self, percentile_seed: f32) {
        self.noise_db = Some(percentile_seed.clamp(-70.0, -12.0));
    }
}

/// 语音活动检测器（自适应能量法）
pub struct VadEngine {
    cfg: VadSettings,
    state: EnergyState,
}

impl VadEngine {
    pub fn new(cfg: &VadSettings) -> Self {
        Self {
            cfg: cfg.clone(),
            state: EnergyState::default(),
        }
    }

    pub fn name(&self) -> &'static str {
        "energy"
    }

    /// 当前估计的噪声底（dBFS），供诊断使用
    pub fn noise_db(&self) -> f32 {
        self.state.noise_db()
    }

    /// 在窗口上检测语音区间。窗口建议 ≥ 1.5 秒。
    ///
    /// `in_utterance` 表示调用方当前是否处于「一句话还没说完」的状态：
    /// 说话期间会冻结噪声底（见 [`EnergyState`]）。
    pub fn detect(&mut self, window: &[f32], in_utterance: bool) -> Vec<SpeechSpan> {
        let mut spans = energy_detect_streaming(
            window,
            self.cfg.energy_threshold_db,
            self.cfg.min_speech_ms,
            self.cfg.min_silence_ms,
            self.cfg.speech_pad_ms,
            &mut self.state,
            in_utterance,
        );
        Self::clamp_overlaps(&mut spans);
        spans
    }

    /// 把语音段之间的重叠消掉。
    ///
    /// 两侧留白（speech_pad_ms）是各自独立加的，当两段语音的间隔小于两倍留白时，
    /// 加完留白就会互相重叠。重叠的时间戳会让字幕对不上、还会让「语音总时长」
    /// 被重复累加而超过音频总长，所以必须夹紧。
    fn clamp_overlaps(spans: &mut [SpeechSpan]) {
        for i in 1..spans.len() {
            if spans[i].start_ms < spans[i - 1].end_ms {
                spans[i].start_ms = spans[i - 1].end_ms;
            }
            if spans[i].end_ms < spans[i].start_ms {
                spans[i].end_ms = spans[i].start_ms;
            }
        }
    }
}

/* ==========================================================================
 * 能量 VAD 实现
 * ========================================================================== */

/// 无状态便捷包装：每轮用窗口自身的低分位数重新估计噪声底。
///
/// 只在单测与「一次性分析」场景下使用；实时链路必须走
/// [`energy_detect_streaming`]，否则连续语音场景会失效。
pub fn energy_detect(
    window: &[f32],
    threshold_db: f32,
    min_speech_ms: u32,
    min_silence_ms: u32,
    speech_pad_ms: u32,
) -> Vec<SpeechSpan> {
    let mut state = EnergyState::default();
    energy_detect_streaming(
        window,
        threshold_db,
        min_speech_ms,
        min_silence_ms,
        speech_pad_ms,
        &mut state,
        false,
    )
}

/// 带自适应噪声底的实现（实时链路使用）。
pub fn energy_detect_streaming(
    window: &[f32],
    threshold_db: f32,
    min_speech_ms: u32,
    min_silence_ms: u32,
    speech_pad_ms: u32,
    state: &mut EnergyState,
    in_utterance: bool,
) -> Vec<SpeechSpan> {
    let frame_ms = (FRAME as u32 * 1000) / RATE; // 20ms
    let n_frames = window.len() / FRAME;
    if n_frames == 0 {
        return Vec::new();
    }

    let dbs: Vec<f32> = (0..n_frames)
        .map(|i| analyze(&window[i * FRAME..(i + 1) * FRAME]).db)
        .collect();

    // 句间（不在说话中）或首次调用时重估噪声底；说话期间冻结。
    //
    // 关键：绝不能每轮都从当前窗口重估。一段没有停顿的连续语音会让窗口里全是语音，
    // 重估出来的"噪声底"就等于语音本身的电平，于是语音再也检不出来。
    if !in_utterance || !state.is_ready() {
        state.observe(percentile(&dbs, 0.10));
    }
    let threshold = state.noise_db() + threshold_db;

    let mut flags: Vec<bool> = dbs
        .iter()
        .map(|db| *db > threshold && *db > ABSOLUTE_SILENCE_DB)
        .collect();

    let min_speech_frames = min_speech_ms.div_ceil(frame_ms).max(1) as usize;
    let min_silence_frames = min_silence_ms.div_ceil(frame_ms).max(1) as usize;

    // 1) 去掉过短的语音突发
    remove_short_runs(&mut flags, min_speech_frames, true);
    // 2) 合并间隔很短的语音段（同一句话里的自然停顿）
    merge_internal_silences(&mut flags, min_silence_frames);

    // 3) 转成区间并按 speech_pad 向两侧扩张
    let pad_frames = speech_pad_ms.div_ceil(frame_ms) as usize;
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < flags.len() {
        if flags[i] {
            let start = i;
            while i < flags.len() && flags[i] {
                i += 1;
            }
            let end = i; // 排他
            let s = start.saturating_sub(pad_frames);
            let e = (end + pad_frames).min(n_frames);
            spans.push(SpeechSpan {
                start_ms: (s as i64) * frame_ms as i64,
                end_ms: (e as i64) * frame_ms as i64,
            });
        } else {
            i += 1;
        }
    }
    spans
}

/// 合并被语音夹住的短静音（句内自然停顿），但**必须保留句首/句尾的静音**。
///
/// 这一点非常关键：如果把窗口两端的静音也填成语音，整个窗口都会被判成语音，
/// 调用方赖以判断「话是否已经说完」的 last_end 就等于窗口末端（永远等于"现在"），
/// 结果是整段录音都不会触发断句，只能等到停止时一次性吐出。
fn merge_internal_silences(flags: &mut [bool], min_len: usize) {
    let mut i = 0usize;
    while i < flags.len() {
        if !flags[i] {
            let start = i;
            while i < flags.len() && !flags[i] {
                i += 1;
            }
            let end = i; // 排他
            let bounded_by_speech = start > 0 && end < flags.len();
            if bounded_by_speech && end - start < min_len {
                for f in flags.iter_mut().take(end).skip(start) {
                    *f = true;
                }
            }
        } else {
            i += 1;
        }
    }
}

/// 把长度不足 `min_len` 的连续区段翻转（`target = true` 时处理语音段，false 处理静音段）
fn remove_short_runs(flags: &mut [bool], min_len: usize, target: bool) {
    let mut i = 0usize;
    while i < flags.len() {
        if flags[i] == target {
            let start = i;
            while i < flags.len() && flags[i] == target {
                i += 1;
            }
            if i - start < min_len {
                for f in flags.iter_mut().take(i).skip(start) {
                    *f = !target;
                }
            }
        } else {
            i += 1;
        }
    }
}

fn percentile(values: &[f32], p: f32) -> f32 {
    if values.is_empty() {
        return -120.0;
    }
    let mut sorted: Vec<f32> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    sorted[idx]
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, ms: u32, amp: f32) -> Vec<f32> {
        let n = (RATE as f32 * ms as f32 / 1000.0) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn noise(ms: u32, amp: f32) -> Vec<f32> {
        // 用确定性双音模拟底噪，避免测试依赖随机数
        let n = (RATE as f32 * ms as f32 / 1000.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                amp * ((2.0 * std::f32::consts::PI * 137.0 * t).sin()
                    + (2.0 * std::f32::consts::PI * 311.0 * t).sin())
                    / 2.0
            })
            .collect()
    }

    #[test]
    fn analyze_computes_rms_and_peak() {
        let e = analyze(&[0.5, -0.5, 0.5, -0.5]);
        assert!((e.rms - 0.5).abs() < 1e-6);
        assert!((e.peak - 0.5).abs() < 1e-6);
        // db 是 RMS 的分贝值：RMS 0.5 → -6.02 dBFS
        assert!((e.db + 6.0206).abs() < 0.01, "实际 {}", e.db);

        // 满量程方波 → 0 dBFS
        assert!(analyze(&[1.0, -1.0, 1.0, -1.0]).db.abs() < 1e-4);

        let silent = analyze(&[0.0, 0.0]);
        assert_eq!(silent.rms, 0.0);
        assert!(silent.db < -100.0);

        assert_eq!(analyze(&[]).db, -120.0);
    }

    #[test]
    fn peak_meter_decays() {
        let mut m = PeakMeter::default();
        m.push(FrameEnergy { rms: 0.8, peak: 0.9, db: -1.0 }, 0.5);
        assert!((m.peak() - 0.9).abs() < 1e-6);
        m.push(FrameEnergy { rms: 0.0, peak: 0.0, db: -120.0 }, 0.5);
        assert!((m.peak() - 0.45).abs() < 1e-6);
        m.push(FrameEnergy { rms: 0.0, peak: 0.0, db: -120.0 }, 0.5);
        assert!((m.peak() - 0.225).abs() < 1e-6);
        m.reset();
        assert_eq!(m.peak(), 0.0);
    }

    #[test]
    fn silence_has_no_speech_spans() {
        let w = noise(4_000, 0.002); // 约 -60dB 的底噪
        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert!(spans.is_empty(), "静音窗口不应检测到语音：{spans:?}");
    }

    #[test]
    fn detects_single_utterance_and_its_position() {
        // 1s 静音 + 1.5s 语音 + 1.5s 静音
        let mut w = noise(1_000, 0.002);
        w.extend(tone(440.0, 1_500, 0.3));
        w.extend(noise(1_500, 0.002));

        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert_eq!(spans.len(), 1, "应只检测到一段语音：{spans:?}");
        let s = spans[0];
        // 真实语音在 1000~2500ms，两侧各扩 200ms 留白 → 期望 800~2700ms
        assert!((s.start_ms - 800).abs() <= 80, "起点偏差过大：{s:?}");
        assert!((s.end_ms - 2_700).abs() <= 80, "终点偏差过大：{s:?}");
        assert_eq!(s.duration_ms(), 1_900);
    }

    #[test]
    fn zero_padding_does_not_create_phantom_speech() {
        // 留白不能超出窗口边界，也不应在纯静音窗口里凭空造出语音
        let w = noise(2_000, 0.002);
        let spans = energy_detect(&w, 9.0, 250, 600, 400);
        assert!(spans.is_empty(), "留白不应制造语音段：{spans:?}");
    }

    #[test]
    fn short_pause_inside_a_sentence_is_merged() {
        // 语音 800ms + 停顿 300ms + 语音 800ms：停顿 < min_silence(600ms) 应合并成一段
        let mut w = tone(440.0, 800, 0.3);
        w.extend(noise(300, 0.002));
        w.extend(tone(440.0, 800, 0.3));
        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert_eq!(spans.len(), 1, "句内停顿不应断句：{spans:?}");
    }

    #[test]
    fn leading_and_trailing_silence_are_preserved() {
        // 回归测试：曾经句首/句尾的静音会被"合并短静音"填成语音，
        // 导致调用方无法判断话是否说完（last_end 永远等于窗口末端）。
        let mut w = noise(400, 0.002);
        w.extend(tone(440.0, 1_000, 0.3));
        w.extend(noise(400, 0.002));

        let spans = energy_detect(&w, 9.0, 250, 600, 0);
        assert_eq!(spans.len(), 1, "应只有一段语音：{spans:?}");
        let s = spans[0];
        // 末尾必须明显早于窗口结束（窗口 1800ms，语音到 1400ms）
        assert!(
            s.end_ms <= 1_500,
            "语音终点不应被拉到窗口末端：{s:?}（窗口 1800ms）"
        );
        assert!(s.start_ms >= 300, "语音起点不应被拉到窗口开头：{s:?}");
    }

    #[test]
    fn spans_never_overlap_after_padding() {
        // 两段语音间隔 800ms，留白各 200ms：加完留白仍然不能重叠
        let mut w = tone(440.0, 800, 0.3);
        w.extend(noise(800, 0.002));
        w.extend(tone(440.0, 800, 0.3));
        let mut engine = VadEngine::new(&VadSettings {
            min_silence_ms: 600,
            speech_pad_ms: 400, // 故意放大留白，制造重叠压力
            ..Default::default()
        });
        let spans = engine.detect(&w, false);
        assert!(spans.len() >= 2, "应至少两段：{spans:?}");
        for i in 1..spans.len() {
            assert!(
                spans[i].start_ms >= spans[i - 1].end_ms,
                "语音段重叠了：{:?}",
                spans
            );
        }
    }

    #[test]
    fn long_pause_splits_into_two_utterances() {
        let mut w = tone(440.0, 800, 0.3);
        w.extend(noise(1_200, 0.002));
        w.extend(tone(440.0, 800, 0.3));
        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert_eq!(spans.len(), 2, "长停顿应断成两句：{spans:?}");
        assert!(spans[0].end_ms <= spans[1].start_ms);
    }

    #[test]
    fn very_short_blip_is_ignored() {
        // 100ms 的咔哒声短于 min_speech(250ms)，应被忽略
        let mut w = noise(1_000, 0.002);
        w.extend(tone(1_000.0, 100, 0.5));
        w.extend(noise(1_000, 0.002));
        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert!(spans.is_empty(), "短促噪声不应触发识别：{spans:?}");
    }

    #[test]
    fn loud_noise_floor_does_not_swallow_speech() {
        // 环境底噪较高（约 -40dB）时仍能检出明显高于底噪的语音
        let mut w = noise(1_500, 0.01);
        w.extend(tone(440.0, 1_200, 0.5));
        w.extend(noise(1_500, 0.01));
        let spans = energy_detect(&w, 9.0, 250, 600, 200);
        assert_eq!(spans.len(), 1, "嘈杂环境下应能检出语音：{spans:?}");
    }

    #[test]
    fn empty_and_tiny_windows_are_safe() {
        assert!(energy_detect(&[], 9.0, 250, 600, 200).is_empty());
        assert!(energy_detect(&[0.0; 100], 9.0, 250, 600, 200).is_empty());
    }

    #[test]
    fn percentile_is_sane() {
        let v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile(&v, 0.0) - 1.0).abs() < 1e-6);
        assert!((percentile(&v, 1.0) - 5.0).abs() < 1e-6);
        assert!((percentile(&v, 0.5) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn continuous_speech_does_not_raise_the_noise_floor() {
        // 回归测试：如果每轮都从当前窗口重估噪声底，一段没有停顿的连续语音
        // 会把噪声底一起抬上去，语音就再也检不出来（整段录音都不会断句）。
        let mut engine = VadEngine::new(&VadSettings::default());

        // 先来 1 秒静音把噪声底压到最低
        let mut window = noise(1_000, 0.002);
        let spans = engine.detect(&window, false);
        assert!(spans.is_empty());

        // 再持续加长连续语音，每一轮都必须能检出「说到现在」
        for extra in 1..=12 {
            window.extend(tone(440.0, 500, 0.3));
            let spans = engine.detect(&window, true);
            let last_end = spans.last().map(|s| s.end_ms).unwrap_or(0);
            let window_ms = window.len() as i64 * 1000 / RATE as i64;
            assert!(
                !spans.is_empty(),
                "第 {extra} 轮（窗口 {window_ms} ms）连续语音没有检出：{spans:?}"
            );
            assert!(
                last_end >= window_ms - 300,
                "第 {extra} 轮语音终点没有延伸到窗口末端：{last_end} vs {window_ms}"
            );
            // 窗口本身在变长，语音长度也要跟着变长
            window = window[(window.len() - 8_000)..].to_vec(); // 只保留最近 0.5 秒 + 新语音
        }
    }

    #[test]
    fn noise_floor_adapts_to_a_noisier_room() {
        let mut state = EnergyState::default();
        // 安静房间
        let quiet = noise(2_000, 0.002);
        energy_detect_streaming(&quiet, 9.0, 250, 600, 200, &mut state, false);
        let quiet_floor = state.noise_db();
        // 换到明显更吵的房间（底噪 -40dB 左右）
        let loud = noise(4_000, 0.01);
        energy_detect_streaming(&loud, 9.0, 250, 600, 200, &mut state, false);
        let loud_floor = state.noise_db();
        assert!(
            loud_floor > quiet_floor,
            "噪声底应当随环境变吵而上升：{quiet_floor} → {loud_floor}"
        );
        assert!(loud_floor < -20.0, "上升幅度不应失控：{loud_floor}");

        // 说话期间必须冻结，不能继续跟随窗口里的语音电平
        let frozen_before = state.noise_db();
        let speech = tone(440.0, 3_000, 0.5);
        energy_detect_streaming(&speech, 9.0, 250, 600, 200, &mut state, true);
        assert_eq!(
            state.noise_db(),
            frozen_before,
            "说话期间噪声底不应变化"
        );
    }

    #[test]
    fn engine_reports_its_name() {
        let engine = VadEngine::new(&VadSettings::default());
        assert_eq!(engine.name(), "energy");
    }

    #[test]
    fn energy_engine_detect_end_to_end() {
        let cfg = VadSettings::default();
        let mut engine = VadEngine::new(&cfg);
        let mut w = noise(800, 0.002);
        w.extend(tone(440.0, 1_000, 0.3));
        w.extend(noise(800, 0.002));
        let spans = engine.detect(&w, false);
        assert_eq!(spans.len(), 1);
    }
}
