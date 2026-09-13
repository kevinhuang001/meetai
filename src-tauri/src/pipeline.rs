//! 实时转写流水线：断句 → 识别 → 滚动纪要。
//!
//! ```text
//! MixedFrame(16k 单声道) ─▶ 断句线程 ─AsrJob─▶ 识别线程 ─▶ 事件(partial/segment)
//!                              │                                │
//!                              └──▶ 会话状态(Arc<Mutex<Session>>) ──▶ 纪要循环 ─▶ AI 接口
//! ```
//!
//! 三个关键设计：
//!
//! 1. **按句定稿**。识别服务只接受「一段完整音频」，所以用 VAD 把连续语音切成
//!    「一句话」，每句话单独发一次请求 —— 这既让服务端拿到完整上下文（准确率最高），
//!    也让延迟有上界。可选开启「边说边出字」（`live_preview`）：说话过程中每 800ms
//!    对当前这句话再识别一次，用 LocalAgreement 取两次结果的最长公共前缀作为已确认部分。
//!
//! 2. **任务合并**。一次请求往返要几百毫秒，所以识别线程入口有一个合并队列：
//!    同一句话的旧 partial 直接被新 partial 顶掉，某句话一旦定稿，
//!    它遗留的 partial 全部作废。否则队列会越积越多、字幕越来越滞后。
//!
//! 3. **放弃无用请求**。定稿会置位 abort 标志，排队中的增量任务被识别线程直接跳过，
//!    正在飞的请求返回后也会被丢弃 —— 不会为了过期的预览白花一次调用。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use futures_util::future::BoxFuture;
use parking_lot::Mutex;

use crate::ai::client::{AiClient, TokenUsage};
use crate::ai::prompts;
use crate::ai::summarizer::{self, SummaryPatch};
use crate::asr::client::{AsrClient, TranscribeRequest};
use crate::asr::filter::{clean_text, is_likely_hallucination};
use crate::asr::hypothesis::HypothesisBuffer;
use crate::audio::capture::{CaptureHandle, MixedFrame, SourceKind};
use crate::audio::vad::{FrameEnergy, SpeechSpan, VadEngine};
use crate::error::{AppError, AppResult};
use crate::events::{
    self, event, AiStateKind, AiStatusEvent, AiSummaryEvent, AsrLevelEvent, AsrPartialEvent,
    AsrSegmentEvent, AsrStateEvent, AsrStateKind, Emitter, LevelItem, SessionUpdatedEvent,
};
use crate::session::{Session, Speaker, TranscriptSegment};
use crate::settings::{AiSettings, AsrProvider, AsrSettings, VadSettings};
use crate::util::{level_to_display, now_ms};

/// partial 重识别间隔
const PARTIAL_STEP_MS: i64 = 800;
/// VAD 运行间隔
const VAD_STEP_MS: i64 = 500;
/// 断句时的前滚留白（避免吃掉句首辅音）
const PRE_ROLL_MS: i64 = 400;
/// 电平事件间隔
const LEVEL_STEP_MS: i64 = 100;
/// 历史音频最多保留（相对 max_utterance 的余量）
const HISTORY_MARGIN_MS: i64 = 3_000;
/// 声源能量记录窗口（用于判定说话人）
const ENERGY_RING_MS: i64 = 10_000;

/* ==========================================================================
 * 声源能量占比 → 说话人
 * ========================================================================== */

#[derive(Debug, Clone, Copy, Default)]
pub struct SourceShare {
    pub mic: f64,
    pub loopback: f64,
}

impl SourceShare {
    pub fn add_frame(&mut self, kind: SourceKind, energy: FrameEnergy) {
        let e = (energy.rms as f64) * (energy.rms as f64);
        match kind {
            SourceKind::Microphone => self.mic += e,
            SourceKind::Loopback => self.loopback += e,
        }
    }

    /// 双路采集时按能量占比判定「我 / 对方 / 双方」
    pub fn speaker(&self) -> Speaker {
        let total = self.mic + self.loopback;
        if total <= 1e-9 {
            return Speaker::Unknown;
        }
        let mic_ratio = self.mic / total;
        if mic_ratio >= 0.78 {
            Speaker::Me
        } else if mic_ratio <= 0.22 {
            Speaker::Others
        } else {
            Speaker::Mixed
        }
    }
}

/// 只有一路时的说话人归属由调用方指定
pub fn speaker_for_single_source(kind: Option<SourceKind>) -> Speaker {
    match kind {
        Some(SourceKind::Microphone) => Speaker::Me,
        Some(SourceKind::Loopback) => Speaker::Others,
        None => Speaker::Unknown,
    }
}

/* ==========================================================================
 * 断句
 * ========================================================================== */

#[derive(Debug)]
pub enum AsrJob {
    Partial {
        utt_id: u64,
        samples: Vec<f32>,
        start_ms: i64,
        /// 定稿时会置位，用于抢占正在跑的 partial 推理
        abort: Arc<AtomicBool>,
    },
    Final {
        utt_id: u64,
        samples: Vec<f32>,
        start_ms: i64,
        end_ms: i64,
        share: SourceShare,
    },
}

#[derive(Debug)]
pub enum SegmenterAction {
    SpeechStarted { start_ms: i64 },
    SpeechEnded { end_ms: i64 },
    Job(AsrJob),
}

/// 只保留必要的任务，避免识别线程被过期任务淹没
#[derive(Debug, Default)]
pub struct JobQueue {
    pending: VecDeque<AsrJob>,
}

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn push(&mut self, job: AsrJob) {
        match &job {
            AsrJob::Partial { utt_id, .. } => {
                let utt_id = *utt_id;
                // 这句话已经定稿了，过期 partial 直接丢掉
                let superseded = self.pending.iter().any(|j| {
                    matches!(j, AsrJob::Final { utt_id: id, .. } if *id == utt_id)
                });
                if superseded {
                    return;
                }
                // 同一句话的旧 partial 被新 partial 顶掉
                self.pending.retain(|j| {
                    !matches!(j, AsrJob::Partial { utt_id: id, .. } if *id == utt_id)
                });
                self.pending.push_back(job);
            }
            AsrJob::Final { utt_id, .. } => {
                let utt_id = *utt_id;
                // 这句话已经由定稿任务接管，之前排队的 partial 没有意义了，
                // 直接丢掉可以省下一次完整推理
                self.pending.retain(|j| {
                    !matches!(j, AsrJob::Partial { utt_id: id, .. } if *id == utt_id)
                });
                self.pending.push_back(job);
            }
        }
    }

    pub fn pop(&mut self) -> Option<AsrJob> {
        self.pending.pop_front()
    }
}

/// 断句状态机。与线程无关，可独立单测。
pub struct Segmenter {
    vad: VadEngine,
    cfg: VadSettings,
    /// 是否产出增量预览任务（关闭时不发 partial，省掉 API 调用）
    live_preview: bool,
    history: Vec<f32>,
    history_max: usize,
    /// 已消费音频总时长（会话时间线，毫秒）
    audio_ms: i64,
    in_speech: bool,
    utt_id: u64,
    utt: Vec<f32>,
    utt_start_ms: i64,
    utt_abort: Option<Arc<AtomicBool>>,
    share: SourceShare,
    /// 单路采集时的说话人
    single_source: Option<SourceKind>,
    energy_ring: VecDeque<(i64, SourceKind, f32)>,
    last_vad_ms: i64,
    last_partial_ms: i64,
    total_speech_ms: i64,
    /// 最近一次 VAD 确实看到语音的位置（毫秒）
    last_speech_end_ms: i64,
    /// 最近一次 VAD 确实看到语音的时刻（音频时间线）
    last_speech_seen_at: i64,
    /// 上一句话的终点：新句起点不得早于它，否则时间线会重叠、
    /// 语音总时长会被重复累加而超过音频总长
    last_finalized_end_ms: i64,
}

impl Segmenter {
    pub fn new(cfg: &VadSettings, live_preview: bool) -> Self {
        let history_max = ((cfg.max_utterance_ms as i64 + HISTORY_MARGIN_MS) as usize)
            * crate::audio::vad::RATE as usize
            / 1000;
        Self {
            vad: VadEngine::new(cfg),
            live_preview,
            cfg: cfg.clone(),
            history: Vec::with_capacity(history_max),
            history_max,
            audio_ms: 0,
            in_speech: false,
            utt_id: 0,
            utt: Vec::new(),
            utt_start_ms: 0,
            utt_abort: None,
            share: SourceShare::default(),
            single_source: None,
            energy_ring: VecDeque::new(),
            last_vad_ms: -VAD_STEP_MS,
            last_partial_ms: 0,
            total_speech_ms: 0,
            last_speech_end_ms: 0,
            last_speech_seen_at: 0,
            last_finalized_end_ms: 0,
        }
    }

    /// 告知本次会话只有一路声源（说话人归属直接由它决定）
    pub fn set_single_source(&mut self, kind: Option<SourceKind>) {
        self.single_source = kind;
    }

    pub fn vad_name(&self) -> &'static str {
        self.vad.name()
    }

    pub fn audio_ms(&self) -> i64 {
        self.audio_ms
    }

    pub fn total_speech_ms(&self) -> i64 {
        self.total_speech_ms
    }

    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// 喂入一帧 16k 单声道音频，返回需要执行的动作。
    pub fn push_frame(
        &mut self,
        samples: &[f32],
        levels: &[(String, SourceKind, FrameEnergy)],
    ) -> Vec<SegmenterAction> {
        let mut actions = Vec::new();
        if samples.is_empty() {
            return actions;
        }
        let frame_ms = samples.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
        let frame_start_ms = self.audio_ms;

        self.audio_ms += frame_ms;

        // 记录本帧各声源能量（用于说话人判定与断句起点归属）
        for (_, kind, energy) in levels {
            self.energy_ring.push_back((frame_start_ms, *kind, energy.rms));
        }
        while let Some((ms, _, _)) = self.energy_ring.front() {
            if self.audio_ms - *ms > ENERGY_RING_MS {
                self.energy_ring.pop_front();
            } else {
                break;
            }
        }

        // 维护滚动历史（同时也是 VAD 的输入源）
        self.history.extend_from_slice(samples);
        if self.history.len() > self.history_max {
            let overflow = self.history.len() - self.history_max;
            self.history.drain(..overflow);
        }

        if self.in_speech {
            self.utt.extend_from_slice(samples);
            for (_, kind, energy) in levels {
                self.share.add_frame(*kind, *energy);
            }
        }

        // 超长句子强制断句，避免延迟无限增长
        if self.in_speech {
            let utt_ms = self.utt.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
            if utt_ms >= self.cfg.max_utterance_ms as i64 {
                self.force_split(&mut actions);
            }
        }

        if self.audio_ms - self.last_vad_ms >= VAD_STEP_MS {
            self.last_vad_ms = self.audio_ms;
            self.run_vad(&mut actions);
        }

        if self.live_preview
            && self.in_speech
            && self.audio_ms - self.last_partial_ms >= PARTIAL_STEP_MS
        {
            self.last_partial_ms = self.audio_ms;
            let abort = Arc::new(AtomicBool::new(false));
            self.utt_abort = Some(Arc::clone(&abort));
            actions.push(SegmenterAction::Job(AsrJob::Partial {
                utt_id: self.utt_id,
                samples: self.utt.clone(),
                start_ms: self.utt_start_ms,
                abort,
            }));
        }

        actions
    }

    /// 会话结束：把正在进行中的话立即定稿。
    ///
    /// 注意这里要先强制跑一次 VAD：VAD 是按音频时间线每 500ms 调一次的，
    /// 录音恰好在两次调用之间结束时，最后一次 VAD 就没机会跑，
    /// 于是最后一句只能用「音频末尾」当终点（尾部静音越长，时间戳偏得越多）。
    pub fn flush(&mut self) -> Vec<SegmenterAction> {
        let mut actions = Vec::new();
        if self.in_speech {
            self.run_vad(&mut actions);
        }
        if self.in_speech {
            let end_ms = self
                .last_speech_end_ms
                .max(self.utt_start_ms + 1)
                .min(self.audio_ms);
            self.finalize(end_ms, &mut actions);
        }
        actions
    }

    fn run_vad(&mut self, actions: &mut Vec<SegmenterAction>) {
        // 说话中只对「本句 + 最近一点」跑 VAD，空闲时只看最近 1.5 秒，
        // 这样 VAD 的开销与句子长度无关，长会议也不会越来越慢。
        let (window_start_ms, window) = if self.in_speech {
            let from = self.utt_start_ms.min(self.audio_ms);
            let idx = self.sample_index(from);
            (from, &self.history[idx.min(self.history.len())..])
        } else {
            let from = (self.audio_ms - 1_500).max(0);
            let idx = self.sample_index(from);
            (from, &self.history[idx.min(self.history.len())..])
        };

        if window.len() < (crate::audio::vad::RATE as usize / 2) {
            return;
        }

        let spans = self.vad.detect(window, self.in_speech);
        let abs: Vec<SpeechSpan> = spans
            .into_iter()
            .map(|s| SpeechSpan {
                start_ms: window_start_ms + s.start_ms,
                end_ms: window_start_ms + s.end_ms,
            })
            .collect();

        let now = self.audio_ms;
        let (last_start, last_end) = match abs.last() {
            Some(s) => (s.start_ms, s.end_ms),
            None => (0, 0),
        };

        // 只要这一轮看到了语音，就更新「最后看到语音」的坐标。
        //
        // 关键：VAD 偶尔会返回空结果（能量 VAD 在纯语音窗口上噪声底估高、
        // Silero 在句中短暂停顿上抖动都会这样）。如果把空结果当成「立刻静音」，
        // 就会用 last_end=0 去断句，产生 end==start 的 1 毫秒垃圾片段，
        // 并且把 speech_ms 算成大于音频总时长。所以必须用「距最后一次看到语音多久」
        // 来判定断句，而不是用本轮结果直接判定。
        // 语音是否一直延伸到「现在」
        let speech_now = !abs.is_empty() && last_end >= now - 450;

        if speech_now {
            // 只有当语音确实延续到现在，才算「刚刚还在说话」
            self.last_speech_seen_at = now;
            self.last_speech_end_ms = self.last_speech_end_ms.max(last_end);
        } else if !abs.is_empty() {
            // 窗口里还残留着语音（说明话已经说完了），只更新终点候选，
            // 绝不能刷新 last_speech_seen_at —— 否则只要窗口里还有一点语音，
            // 静音计时就永远不会开始，整段录音都不会断句。
            self.last_speech_end_ms = self.last_speech_end_ms.max(last_end);
        }

        if !self.in_speech {
            if speech_now {
                // 向前把连成一片的语音段合并成一句话的起点
                let mut start = last_start;
                for pair in abs.windows(2).rev() {
                    let gap = pair[1].start_ms - pair[0].end_ms;
                    if gap <= self.cfg.min_silence_ms as i64 + 250 {
                        start = pair[0].start_ms;
                    } else {
                        break;
                    }
                }
                let start = (start - PRE_ROLL_MS).max(0);
                self.start_utterance(start, actions);
            }
        } else if abs.len() >= 2 {
            // 窗口里出现了两段被静音隔开的语音。
            //
            // VAD 输出里两段独立语音之间必然隔着 ≥ min_silence 的静音（更短的停顿
            // 已经被 VAD 自己合并了），所以「倒数第二段」的结束就是当前这句话的终点。
            //
            // 这一步不能省：如果只看最后一段（认为"语音还延续到现在"），新的语音
            // 一出现就会把上一句重新粘回来，多句话会被错误地合并成一句。
            let end = abs[abs.len() - 2].end_ms;
            self.finalize(end.max(self.utt_start_ms + 1), actions);
        } else if !speech_now && now - self.last_speech_seen_at >= self.cfg.min_silence_ms as i64
        {
            // 用最后一次确认的语音终点断句
            let end = self.last_speech_end_ms.max(self.utt_start_ms + 1);
            self.finalize(end, actions);
        }
    }

    fn sample_index(&self, abs_ms: i64) -> usize {
        let history_start_ms = self.audio_ms - (self.history.len() as i64 * 1000 / crate::audio::vad::RATE as i64);
        let rel_ms = (abs_ms - history_start_ms).max(0);
        (rel_ms as usize * crate::audio::vad::RATE as usize / 1000).min(self.history.len())
    }

    fn start_utterance(&mut self, start_ms: i64, actions: &mut Vec<SegmenterAction>) {
        // 前滚与两侧留白都可能让新句起点落在上一句范围内，夹紧保证时间线单调
        let start_ms = start_ms.max(self.last_finalized_end_ms);
        let idx = self.sample_index(start_ms);
        self.utt = self.history[idx.min(self.history.len())..].to_vec();
        self.utt_start_ms = start_ms;
        self.utt_id += 1;
        self.in_speech = true;
        self.last_partial_ms = 0; // 立刻产出一次 partial
        self.utt_abort = None;
        self.last_speech_end_ms = start_ms;
        self.last_speech_seen_at = self.audio_ms;

        // 把这段区间内的声源能量累加起来，用于说话人判定
        let mut share = SourceShare::default();
        for (ms, kind, rms) in &self.energy_ring {
            if *ms >= start_ms {
                share.add_frame(*kind, FrameEnergy { rms: *rms, peak: *rms, db: 0.0 });
            }
        }
        self.share = share;

        actions.push(SegmenterAction::SpeechStarted { start_ms });
    }

    fn finalize(&mut self, end_ms: i64, actions: &mut Vec<SegmenterAction>) {
        // 取消正在跑的 partial 推理，让定稿尽快排到
        if let Some(abort) = self.utt_abort.take() {
            abort.store(true, Ordering::Relaxed);
        }

        let utt_ms = self.utt.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
        let min_speech = self.cfg.min_speech_ms as i64;

        if utt_ms >= min_speech && !self.utt.is_empty() {
            let mut end_ms = end_ms.clamp(self.utt_start_ms + 1, self.audio_ms);
            // 兜底：VAD 给出的终点明显短于实际音频长度时，以音频长度为准，
            // 避免出现 end≈start 的退化片段（会让时间戳与字幕完全对不上）
            if end_ms - self.utt_start_ms < utt_ms * 6 / 10 {
                let fallback = (self.utt_start_ms + utt_ms).min(self.audio_ms);
                tracing::debug!(
                    "VAD 终点异常（{} ms < 音频 {} ms），改用音频长度兜底",
                    end_ms - self.utt_start_ms,
                    utt_ms
                );
                end_ms = fallback;
            }
            self.total_speech_ms += (end_ms - self.utt_start_ms).max(0);
            self.last_finalized_end_ms = end_ms;
            actions.push(SegmenterAction::Job(AsrJob::Final {
                utt_id: self.utt_id,
                samples: std::mem::take(&mut self.utt),
                start_ms: self.utt_start_ms,
                end_ms,
                share: self.share,
            }));
            actions.push(SegmenterAction::SpeechEnded { end_ms });
        }

        self.in_speech = false;
        self.utt = Vec::new();
        self.share = SourceShare::default();
        self.last_partial_ms = 0;
    }

    /// 超长句：在最近 3 秒里找一个能量最低的位置切开
    fn force_split(&mut self, actions: &mut Vec<SegmenterAction>) {
        let rate = crate::audio::vad::RATE as usize;
        let search_ms = 3_000usize.min(self.utt.len() * 1000 / rate);
        let search_samples = search_ms * rate / 1000;
        if search_samples < rate / 2 {
            // 太短，直接整体定稿
            let end = self.audio_ms;
            self.finalize(end, actions);
            return;
        }

        let from = self.utt.len() - search_samples;
        let window = rate / 5; // 200ms
        let mut best_idx = from;
        let mut best_energy = f64::MAX;
        let mut i = from;
        while i + window <= self.utt.len() {
            let energy: f64 = self.utt[i..i + window]
                .iter()
                .map(|v| (*v as f64) * (*v as f64))
                .sum();
            if energy < best_energy {
                best_energy = energy;
                best_idx = i;
            }
            i += window / 2;
        }

        let split_ms = self.utt_start_ms + (best_idx as i64 * 1000 / rate as i64);
        let tail = self.utt.split_off(best_idx);
        let head = std::mem::take(&mut self.utt);

        // 先定稿前半段
        let utt_id = self.utt_id;
        let start_ms = self.utt_start_ms;
        let share = self.share;
        if let Some(abort) = self.utt_abort.take() {
            abort.store(true, Ordering::Relaxed);
        }
        let head_ms = head.len() as i64 * 1000 / rate as i64;
        if head_ms >= self.cfg.min_speech_ms as i64 {
            self.total_speech_ms += head_ms;
            actions.push(SegmenterAction::Job(AsrJob::Final {
                utt_id,
                samples: head,
                start_ms,
                end_ms: split_ms,
                share,
            }));
            actions.push(SegmenterAction::SpeechEnded { end_ms: split_ms });
        }

        // 剩下的部分作为新的一句话接着走
        self.utt_id += 1;
        self.utt = tail;
        self.utt_start_ms = split_ms;
        self.in_speech = !self.utt.is_empty();
        self.share = SourceShare::default();
        self.last_partial_ms = 0;
        self.utt_abort = None;
        self.last_speech_end_ms = split_ms;
        self.last_speech_seen_at = self.audio_ms;

        if self.in_speech {
            actions.push(SegmenterAction::SpeechStarted { start_ms: split_ms });
        }
    }
}

/* ==========================================================================
 * 线程：断句
 * ========================================================================== */

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn run_segmenter(
    mut segmenter: Segmenter,
    frame_rx: Receiver<MixedFrame>,
    job_tx: Sender<AsrJob>,
    session: Arc<Mutex<Session>>,
    emitter: Arc<dyn Emitter>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    recorder: Option<crate::audio::wav::WavRecorder>,
    sources: Vec<(String, SourceKind)>,
) -> AppResult<()> {
    let session_id = session.lock().id.clone();
    let mut recorder = recorder;
    let mut last_level_ms = -LEVEL_STEP_MS;
    let mut last_update_ms = 0i64;

    loop {
        // 停止时不要立刻跳出：通道里可能还压着最多几十帧音频（几百毫秒），
        // 直接退出会丢掉句尾。先把已有的帧处理完，再收尾定稿。
        if stop.load(Ordering::Relaxed) && frame_rx.is_empty() {
            break;
        }

        let frame = match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        // 暂停：直接丢弃这一帧（不推进时间线、不写录音），恢复后接着走
        if paused.load(Ordering::Relaxed) {
            continue;
        }

        if let Some(rec) = recorder.as_mut() {
            if let Err(e) = rec.write(&frame.samples) {
                tracing::warn!("写入录音文件失败：{e}");
                recorder = None;
            }
        }

        // 声源电平（按 sources 顺序补齐，保证 UI 上电平条顺序稳定）
        let levels: Vec<(String, SourceKind, FrameEnergy)> = sources
            .iter()
            .map(|(id, kind)| {
                let energy = frame
                    .levels
                    .iter()
                    .find(|l| l.source_id == *id)
                    .map(|l| l.energy)
                    .unwrap_or(FrameEnergy { rms: 0.0, peak: 0.0, db: -120.0 });
                (id.clone(), *kind, energy)
            })
            .collect();

        for action in segmenter.push_frame(&frame.samples, &levels) {
            apply_action(action, &job_tx, &session, &emitter, &session_id);
        }

        // 电平事件（10Hz）
        if segmenter.audio_ms() - last_level_ms >= LEVEL_STEP_MS {
            last_level_ms = segmenter.audio_ms();
            let items: Vec<LevelItem> = levels
                .iter()
                .map(|(id, kind, energy)| LevelItem {
                    source_id: id.clone(),
                    kind: *kind,
                    peak: level_to_display(energy.peak),
                    rms: level_to_display(energy.rms),
                })
                .collect();

            let (stats, duration_ms) = {
                let mut s = session.lock();
                s.stats.audio_ms = segmenter.audio_ms();
                s.stats.speech_ms = segmenter.total_speech_ms();
                (s.stats, s.live_duration_ms(now_ms()))
            };

            events::emit(
                &emitter,
                event::LEVEL,
                &AsrLevelEvent { session_id: session_id.clone(), levels: items, stats, duration_ms },
            );
        }

        // 会话元信息（1Hz）
        if segmenter.audio_ms() - last_update_ms >= 1_000 {
            last_update_ms = segmenter.audio_ms();
            let info = session.lock().info(now_ms());
            events::emit(&emitter, event::SESSION_UPDATED, &SessionUpdatedEvent { session: info });
        }
    }

    // 收尾：把最后一句吐出来并等待识别线程消化
    for action in segmenter.flush() {
        apply_action(action, &job_tx, &session, &emitter, &session_id);
    }
    // 同步最终统计（电平事件可能停在最后一次定稿之前）
    {
        let mut s = session.lock();
        s.stats.speech_ms = segmenter.total_speech_ms();
        s.stats.audio_ms = segmenter.audio_ms();
    }
    if let Some(rec) = recorder {
        if let Err(e) = rec.finalize() {
            tracing::warn!("完成录音文件失败：{e}");
        }
    }
    Ok(())
}

fn apply_action(
    action: SegmenterAction,
    job_tx: &Sender<AsrJob>,
    session: &Arc<Mutex<Session>>,
    emitter: &Arc<dyn Emitter>,
    session_id: &str,
) {
    match action {
        SegmenterAction::Job(job) => {
            let _ = job_tx.send(job);
        }
        SegmenterAction::SpeechStarted { start_ms } => {
            let _ = start_ms;
            let language = session.lock().language.clone();
            events::emit(
                emitter,
                event::STATE,
                &AsrStateEvent {
                    session_id: session_id.to_string(),
                    state: AsrStateKind::Speech,
                    message: None,
                    language,
                        },
            );
        }
        SegmenterAction::SpeechEnded { .. } => {
            let language = session.lock().language.clone();
            events::emit(
                emitter,
                event::STATE,
                &AsrStateEvent {
                    session_id: session_id.to_string(),
                    state: AsrStateKind::Listening,
                    message: None,
                    language,
                        },
            );
        }
    }
}

/* ==========================================================================
 * 线程：识别
 * ========================================================================== */

pub struct AsrWorkerConfig {
    pub session_id: String,
    pub provider: AsrProvider,
    pub settings: AsrSettings,
    pub single_source: Option<SourceKind>,
}

fn run_asr_worker(
    cfg: AsrWorkerConfig,
    job_rx: Receiver<AsrJob>,
    session: Arc<Mutex<Session>>,
    emitter: Arc<dyn Emitter>,
    stop: Arc<AtomicBool>,
    summarizer_kick: Arc<tokio::sync::Notify>,
    ai_ready: bool,
) -> AppResult<()> {
    let sid = cfg.session_id.clone();

    events::emit(
        &emitter,
        event::STATE,
        &AsrStateEvent {
            session_id: sid.clone(),
            state: AsrStateKind::Starting,
            message: Some(format!(
                "正在连接识别服务「{}」（{}）…",
                cfg.provider.name, cfg.provider.model
            )),
            language: None,
        },
    );

    // 识别工作线程自己持有一个 Tokio 运行时来跑 HTTP 请求。
    // 用 current_thread 即可：这个线程本来就是串行处理识别任务的。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::asr(format!("创建识别运行时失败：{e}")))?;
    let client = AsrClient::new()?;
    let endpoint = cfg.provider.endpoint();
    tracing::info!("语音识别服务：{endpoint}（模型 {}）", cfg.provider.model);

    let mut language = cfg.settings.language_arg().map(str::to_string);
    let mut locked_language = language.is_some();
    {
        let mut s = session.lock();
        s.language = language.clone();
    }

    events::emit(
        &emitter,
        event::STATE,
        &AsrStateEvent {
            session_id: sid.clone(),
            state: AsrStateKind::Listening,
            message: None,
            language: language.clone(),
        },
    );

    let mut queue = JobQueue::new();
    let mut hb = HypothesisBuffer::new();
    let mut current_utt: Option<u64> = None;
    let mut prompt_tail: Option<String> = None;
    let mut rtf_samples: VecDeque<f32> = VecDeque::new();
    let mut latency_samples: VecDeque<i64> = VecDeque::new();
    let mut request_counter: u64 = 0;

    loop {
        // 取任务：只有「通道断开且队列为空」才退出。
        //
        // 不能用停止标志直接 break：停止时断句线程还要 flush 出最后一句话，
        // 而且 Pipeline::stop 会先 join 断句线程再 join 本线程，
        // 所以必须以 job_tx 全部 drop（通道断开）作为唯一退出条件，
        // 否则会丢掉最后一句的定稿结果。
        if queue.is_empty() {
            let timeout = if stop.load(Ordering::Relaxed) {
                Duration::from_millis(400)
            } else {
                Duration::from_millis(150)
            };
            match job_rx.recv_timeout(timeout) {
                Ok(job) => queue.push(job),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::Relaxed) && job_rx.is_empty() {
                        break;
                    }
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(job) = job_rx.try_recv() {
            queue.push(job);
        }

        let Some(job) = queue.pop() else { continue };

        match job {
            // ---------- 增量预览（"边说边出字"）----------
            AsrJob::Partial { utt_id, samples, start_ms, abort } => {
                if !cfg.settings.live_preview {
                    continue;
                }
                // 已经被定稿抢占，不用再花一次请求
                if abort.load(Ordering::Relaxed) {
                    continue;
                }
                if current_utt != Some(utt_id) {
                    hb.reset();
                    current_utt = Some(utt_id);
                }
                let audio_ms = samples.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
                let req = TranscribeRequest {
                    samples,
                    language: language.clone(),
                    prompt: context_prompt(&cfg.settings, &prompt_tail),
                    temperature: cfg.settings.temperature,
                };
                match rt.block_on(client.transcribe(&cfg.provider, &req)) {
                    Ok(out) => {
                        if abort.load(Ordering::Relaxed) {
                            continue;
                        }
                        let text = clean_text(&out.text);
                        if text.is_empty() || is_likely_hallucination(&text, audio_ms) {
                            continue;
                        }
                        let agreement = hb.update(&text);
                        if agreement.is_empty() {
                            continue;
                        }
                        events::emit(
                            &emitter,
                            event::PARTIAL,
                            &AsrPartialEvent {
                                session_id: sid.clone(),
                                committed: agreement.committed,
                                tentative: agreement.tentative,
                                start_ms,
                            },
                        );
                    }
                    Err(e) => tracing::warn!("增量识别失败：{e}"),
                }
            }

            // ---------- 定稿 ----------
            AsrJob::Final { utt_id, samples, start_ms, end_ms, share } => {
                hb.reset();
                current_utt = None;

                let audio_ms = samples.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
                let req = TranscribeRequest {
                    samples,
                    language: language.clone(),
                    prompt: context_prompt(&cfg.settings, &prompt_tail),
                    temperature: cfg.settings.temperature,
                };
                request_counter += 1;

                let out = match rt.block_on(client.transcribe(&cfg.provider, &req)) {
                    Ok(o) => o,
                    Err(e) => {
                        // 单句失败不应该中断整场会议：记录错误、清掉灰字、继续下一句
                        tracing::warn!("识别失败（第 {request_counter} 次请求）：{e}");
                        events::emit(
                            &emitter,
                            event::ERROR,
                            &events::AppErrorEvent {
                                scope: "asr".into(),
                                message: e.to_string(),
                            },
                        );
                        events::emit(
                            &emitter,
                            event::PARTIAL,
                            &AsrPartialEvent {
                                session_id: sid.clone(),
                                committed: String::new(),
                                tentative: String::new(),
                                start_ms: end_ms,
                            },
                        );
                        continue;
                    }
                };

                let rtf = out.rtf();
                if rtf > 0.0 {
                    rtf_samples.push_back(rtf);
                    while rtf_samples.len() > 10 {
                        rtf_samples.pop_front();
                    }
                }
                latency_samples.push_back(out.elapsed_ms as i64);
                while latency_samples.len() > 10 {
                    latency_samples.pop_front();
                }

                let text = clean_text(&out.text);
                if text.is_empty() || is_likely_hallucination(&text, audio_ms) {
                    events::emit(
                        &emitter,
                        event::PARTIAL,
                        &AsrPartialEvent {
                            session_id: sid.clone(),
                            committed: String::new(),
                            tentative: String::new(),
                            start_ms: end_ms,
                        },
                    );
                    continue;
                }

                // 首次拿到服务返回的语言时锁定，后续请求就带上它（更快更稳）
                if !locked_language {
                    if let Some(detected) = out.language.clone() {
                        if !detected.is_empty() && !detected.eq_ignore_ascii_case("auto") {
                            language = Some(normalize_language(&detected));
                            locked_language = true;
                            let lang = language.clone();
                            session.lock().language = lang.clone();
                            events::emit(
                                &emitter,
                                event::STATE,
                                &AsrStateEvent {
                                    session_id: sid.clone(),
                                    state: AsrStateKind::Listening,
                                    message: None,
                                    language: lang,
                                                        },
                            );
                        }
                    }
                }

                let speaker = match cfg.single_source {
                    Some(kind) => speaker_for_single_source(Some(kind)),
                    None => share.speaker(),
                };

                let segment = {
                    let mut s = session.lock();
                    let id = s.next_id();
                    let seg = TranscriptSegment {
                        id,
                        text: text.clone(),
                        start_ms,
                        end_ms: end_ms.max(start_ms + 1),
                        speaker,
                        language: language.clone(),
                        confidence: None,
                    };
                    s.push_segment(seg.clone());
                    s.stats.rtf = median_f32(&rtf_samples);
                    s.stats.latency_ms = median_i64(&latency_samples);
                    seg
                };

                if cfg.settings.context_prompt {
                    prompt_tail = Some(text);
                }

                events::emit(
                    &emitter,
                    event::SEGMENT,
                    &AsrSegmentEvent {
                        session_id: sid.clone(),
                        segment,
                    },
                );
                events::emit(
                    &emitter,
                    event::PARTIAL,
                    &AsrPartialEvent {
                        session_id: sid.clone(),
                        committed: String::new(),
                        tentative: String::new(),
                        start_ms: end_ms,
                    },
                );

                if ai_ready {
                    summarizer_kick.notify_one();
                }
                let _ = utt_id;
            }
        }
    }

    Ok(())
}

/// 服务返回的语言可能是 `chinese` / `zh` / `Chinese` 这类形式，统一成 ISO 代码
fn normalize_language(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    match lower.as_str() {
        "chinese" | "zh" | "cmn" | "mandarin" => "zh".into(),
        "english" | "en" => "en".into(),
        "japanese" | "ja" | "jpn" => "ja".into(),
        "korean" | "ko" | "kor" => "ko".into(),
        "cantonese" | "yue" => "yue".into(),
        other if other.len() <= 5 => other.to_string(),
        // 长名字（例如 "chinese (simplified)"）取首词再试一次
        _ => lower.split_whitespace().next().map(normalize_language).unwrap_or(lower),
    }
}

/// 是否把上一句文本作为 prompt 传给识别服务（提升人名/术语一致性）
fn context_prompt(asr: &AsrSettings, tail: &Option<String>) -> Option<String> {
    if !asr.context_prompt {
        return None;
    }
    tail.as_ref().map(|t| crate::util::truncate_chars(t, 120))
}

fn median_f32(v: &VecDeque<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f32> = v.iter().copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

fn median_i64(v: &VecDeque<i64>) -> i64 {
    if v.is_empty() {
        return 0;
    }
    let mut sorted: Vec<i64> = v.iter().copied().collect();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/* ==========================================================================
 * 纪要循环
 * ========================================================================== */

/// 滚动纪要：定时把「已有纪要 + 新增转写」交给模型，得到更新后的完整纪要。
pub async fn summarize_once(
    client: &AiClient,
    settings: &AiSettings,
    session: &Arc<Mutex<Session>>,
    emitter: &Arc<dyn Emitter>,
    force: bool,
) -> AppResult<bool> {
    let provider = settings
        .active()
        .ok_or_else(|| AppError::ai("尚未配置 AI 服务商"))?
        .clone();

    // 1) 取出本轮要送的内容（注意：不要跨 await 持有锁）
    let (session_id, title, messages, covered_from, covered_to, calls) = {
        let s = session.lock();
        let last_end = s.last_segment_end_ms();
        let covered_from = s.summary.covered_until_ms;
        if last_end <= covered_from && !force {
            return Ok(false);
        }
        let new_text = s.transcript_between(covered_from, last_end);
        if new_text.trim().is_empty() && !force {
            return Ok(false);
        }
        let live = s.recent_transcript(now_ms(), settings.live_window_secs);
        let max_chars = settings.max_context_chars as usize;
        let messages = prompts::build_incremental_messages(
            &s.title,
            &s.summary.overview,
            &s.summary.summary,
            &s.summary.key_points,
            &s.summary.decisions,
            &s.summary.action_items_text(),
            &s.summary.topics,
            &crate::util::truncate_middle(&new_text, max_chars),
            &crate::util::truncate_middle(&live, max_chars / 2),
            covered_from,
            last_end,
            now_ms(),
            settings.live_window_secs,
        );
        (
            s.id.clone(),
            s.title.clone(),
            messages,
            covered_from,
            last_end,
            s.summary.calls,
        )
    };
    let _ = title;

    events::emit(
        emitter,
        event::AI_STATUS,
        &AiStatusEvent {
            session_id: session_id.clone(),
            state: AiStateKind::Thinking,
            message: None,
            calls: calls + 1,
        },
    );

    match client.chat(&provider, &messages).await {
        Ok(outcome) => {
            let patch: SummaryPatch = match summarizer::parse_summary_patch(&outcome.content) {
                Ok(p) => p,
                Err(e) => {
                    // 解析失败也不能丢掉这一轮内容：不推进 covered_until_ms，下一轮重试
                    let mut s = session.lock();
                    s.summary.set_error(e.to_string());
                    let summary = s.summary.clone();
                    let calls = summary.calls;
                    drop(s);
                    events::emit(
                        emitter,
                        event::AI_STATUS,
                        &AiStatusEvent {
                            session_id: session_id.clone(),
                            state: AiStateKind::Error,
                            message: Some(e.to_string()),
                            calls,
                        },
                    );
                    events::emit(emitter, event::SUMMARY, &AiSummaryEvent { session_id, summary });
                    return Err(e);
                }
            };

            let summary = {
                let mut s = session.lock();
                let model = if outcome.model.is_empty() {
                    provider.model.clone()
                } else {
                    outcome.model.clone()
                };
                summarizer::apply_patch(
                    &mut s.summary,
                    patch,
                    &model,
                    covered_to,
                    &TokenUsage {
                        prompt_tokens: outcome.usage.prompt_tokens,
                        completion_tokens: outcome.usage.completion_tokens,
                        total_tokens: outcome.usage.total_tokens,
                    },
                );
                s.summary.clone()
            };

            let calls = summary.calls;
            events::emit(
                emitter,
                event::AI_STATUS,
                &AiStatusEvent {
                    session_id: session_id.clone(),
                    state: AiStateKind::Idle,
                    message: None,
                    calls,
                },
            );
            events::emit(
                emitter,
                event::SUMMARY,
                &AiSummaryEvent { session_id, summary },
            );
            let _ = covered_from;
            Ok(true)
        }
        Err(e) => {
            let mut s = session.lock();
            s.summary.set_error(e.to_string());
            let summary = s.summary.clone();
            let calls = summary.calls;
            drop(s);
            events::emit(
                emitter,
                event::AI_STATUS,
                &AiStatusEvent {
                    session_id: session_id.clone(),
                    state: AiStateKind::Error,
                    message: Some(e.to_string()),
                    calls,
                },
            );
            events::emit(emitter, event::SUMMARY, &AiSummaryEvent { session_id, summary });
            Err(e)
        }
    }
}

/// 周期总结循环（纯异步函数，由调用方决定怎么调度）。
///
/// 这里刻意**不**自己 `tokio::spawn`：Tauri 的同步命令里没有 Tokio 运行时上下文，
/// 直接 spawn 会 panic（`there is no reactor running`）。把调度权交给调用方，
/// 既避免了这个坑，也让 pipeline 不必依赖 Tauri（测试里可以用自己的运行时）。
/// 循环本身通过 `stop` 标志退出，因此不需要调用方持有 JoinHandle。
pub async fn summary_loop(
    client: Arc<AiClient>,
    settings: AiSettings,
    session: Arc<Mutex<Session>>,
    emitter: Arc<dyn Emitter>,
    kick: Arc<tokio::sync::Notify>,
    stop: Arc<AtomicBool>,
) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let mut last_run = Instant::now() - Duration::from_secs(3600);
        loop {
            let forced = tokio::select! {
                _ = interval.tick() => false,
                _ = kick.notified() => true,
            };
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if !settings.ready() {
                continue;
            }

            let (new_chars, due) = {
                let s = session.lock();
                let last_end = s.last_segment_end_ms();
                let new_text = s.transcript_between(s.summary.covered_until_ms, last_end);
                let chars = new_text.chars().count() as u32;
                (
                    chars,
                    last_run.elapsed() >= Duration::from_secs(settings.interval_secs as u64),
                )
            };

            let should = forced
                || (new_chars >= settings.min_new_chars.max(1)
                    && (due || new_chars >= settings.min_new_chars.saturating_mul(3).max(1)));

            if !should {
                continue;
            }
            // 手动触发要立刻生效；自动触发尊重间隔
            if !forced && !due {
                continue;
            }

            last_run = Instant::now();
            if let Err(e) = summarize_once(&client, &settings, &session, &emitter, forced).await {
                tracing::warn!("自动总结失败：{e}");
            }
        }
}

/* ==========================================================================
 * 流水线
 * ========================================================================== */

/// 把「如何调度异步任务」交给调用方（Tauri 用 async_runtime，测试用自建运行时）
pub type TaskSpawner = Box<dyn FnOnce(BoxFuture<'static, ()>) + Send>;

pub struct PipelineOptions {
    pub provider: AsrProvider,
    pub asr: AsrSettings,
    pub vad: VadSettings,
    pub single_source: Option<SourceKind>,
    pub record_path: Option<PathBuf>,
    pub ai: AiSettings,
    /// AI 可用时，用这个能力把纪要循环挂到运行时上
    pub spawn_task: Option<TaskSpawner>,
}

pub struct Pipeline {
    pub stop: Arc<AtomicBool>,
    /// 暂停标志：暂停期间采集继续但音频被丢弃
    pub paused: Arc<AtomicBool>,
    pub session: Arc<Mutex<Session>>,
    pub sources: Vec<(String, SourceKind)>,
    pub vad_name: &'static str,
    capture: Option<CaptureHandle>,
    threads: Vec<JoinHandle<()>>,
    kick: Arc<tokio::sync::Notify>,
}

impl Pipeline {
    pub fn summary_kick(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.kick)
    }

    /// 停止并等待所有线程退出，返回收敛后的会话
    pub fn stop(mut self) -> Arc<Mutex<Session>> {
        self.stop.store(true, Ordering::Relaxed);

        // 先停采集，让上游通道关闭
        if let Some(capture) = self.capture.take() {
            // stop() 会 join 采集线程；混音线程随后因上游关闭而退出
            capture.stop();
        }

        for h in self.threads.drain(..) {
            let _ = h.join();
        }
        // 纪要循环靠 stop 标志自行退出（最长一个 tick），不需要在这里 join

        let mut s = self.session.lock();
        s.status = crate::session::SessionStatus::Finished;
        if let Some(t) = s.resumed_at.take() {
            s.duration_ms += (now_ms() - t).max(0);
        }
        Arc::clone(&self.session)
    }
}

/// 启动流水线。`frame_rx` 既可以是真实采集，也可以是文件喂入。
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    options: PipelineOptions,
    frame_rx: Receiver<MixedFrame>,
    capture: Option<CaptureHandle>,
    sources: Vec<(String, SourceKind)>,
    session: Arc<Mutex<Session>>,
    emitter: Arc<dyn Emitter>,
    ai_client: Option<Arc<AiClient>>,
) -> AppResult<Pipeline> {
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (job_tx, job_rx) = bounded::<AsrJob>(32);
    let session_id = session.lock().id.clone();

    let single_source = if sources.len() == 1 {
        options.single_source.or_else(|| sources.first().map(|(_, k)| *k))
    } else {
        None
    };

    let segmenter = Segmenter::new(&options.vad, options.asr.live_preview);
    let vad_name = segmenter.vad_name();
    let mut segmenter = segmenter;
    segmenter.set_single_source(single_source);

    // 录音文件
    let recorder = match &options.record_path {
        Some(path) => match crate::audio::wav::WavRecorder::create(path) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("无法创建录音文件：{e}");
                None
            }
        },
        None => None,
    };
    if let Some(path) = &options.record_path {
        session.lock().audio_path = Some(path.display().to_string());
    }

    let mut threads = Vec::new();

    // 断句线程
    {
        let job_tx = job_tx.clone();
        let session = Arc::clone(&session);
        let emitter = Arc::clone(&emitter);
        let stop = Arc::clone(&stop);
        let paused_flag = Arc::clone(&paused);
        let sources = sources.clone();
        let handle = std::thread::Builder::new()
            .name("mh-segmenter".into())
            .spawn(move || {
                if let Err(e) = run_segmenter(
                    segmenter, frame_rx, job_tx, session, emitter, stop, paused_flag, recorder,
                    sources,
                ) {
                    tracing::error!("断句线程异常：{e}");
                }
            })
            .map_err(|e| AppError::asr(format!("无法启动断句线程：{e}")))?;
        threads.push(handle);
    }

    // 识别线程
    let kick = Arc::new(tokio::sync::Notify::new());
    let ai_ready = options.ai.ready();
    {
        let session = Arc::clone(&session);
        let emitter = Arc::clone(&emitter);
        let stop = Arc::clone(&stop);
        let kick = Arc::clone(&kick);
        let worker_cfg = AsrWorkerConfig {
            session_id,
            provider: options.provider.clone(),
            settings: options.asr.clone(),
            single_source,
        };
        let handle = std::thread::Builder::new()
            .name("mh-asr".into())
            .spawn(move || {
                if let Err(e) =
                    run_asr_worker(worker_cfg, job_rx, session, emitter, stop, kick, ai_ready)
                {
                    tracing::error!("识别线程异常：{e}");
                }
            })
            .map_err(|e| AppError::asr(format!("无法启动识别线程：{e}")))?;
        threads.push(handle);
    }

    // 纪要循环
    if let (Some(client), true, Some(spawn)) = (ai_client, ai_ready, options.spawn_task) {
        let settings = options.ai.clone();
        let session = Arc::clone(&session);
        let emitter = Arc::clone(&emitter);
        let kick = Arc::clone(&kick);
        let stop = Arc::clone(&stop);
        let fut: BoxFuture<'static, ()> = Box::pin(async move {
            summary_loop(client, settings, session, emitter, kick, stop).await;
        });
        spawn(fut);
    }

    Ok(Pipeline {
        stop,
        paused,
        session,
        sources,
        vad_name,
        capture,
        threads,
        kick,
    })
}

/// 供测试与「导入音频文件」使用：把一段 16k 单声道音频按 100ms 帧喂入通道
pub fn feed_audio(
    samples: &[f32],
    tx: Sender<MixedFrame>,
    source_id: &str,
    kind: SourceKind,
    samples_per_step: usize,
    step_pause: Option<Duration>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    let samples = samples.to_vec();
    let source_id = source_id.to_string();
    std::thread::spawn(move || {
        let mut pos = 0usize;
        while pos < samples.len() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let end = (pos + samples_per_step).min(samples.len());
            let chunk = &samples[pos..end];
            let energy = crate::audio::vad::analyze(chunk);
            let frame = MixedFrame {
                samples: chunk.to_vec(),
                levels: vec![crate::audio::capture::SourceLevel {
                    source_id: source_id.clone(),
                    kind,
                    energy,
                }],
            };
            if tx.send(frame).is_err() {
                return;
            }
            pos = end;
            if let Some(pause) = step_pause {
                std::thread::sleep(pause);
            }
        }
    })
}



/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::vad::analyze;

    const FRAME: usize = 1_600; // 100ms @16kHz

    fn tone_frame(amp: f32, offset: usize) -> Vec<f32> {
        (0..FRAME)
            .map(|i| {
                let t = (offset + i) as f32 / crate::audio::vad::RATE as f32;
                amp * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect()
    }

    fn quiet_frame() -> Vec<f32> {
        vec![0.0; FRAME]
    }

    fn levels_for(samples: &[f32]) -> Vec<(String, SourceKind, FrameEnergy)> {
        vec![(
            "mic:test".to_string(),
            SourceKind::Microphone,
            analyze(samples),
        )]
    }

    fn drive(seg: &mut Segmenter, frames: &[Vec<f32>]) -> Vec<SegmenterAction> {
        let mut actions = Vec::new();
        for f in frames {
            let levels = levels_for(f);
            actions.extend(seg.push_frame(f, &levels));
        }
        actions
    }

    fn finals(actions: &[SegmenterAction]) -> Vec<(i64, i64, usize)> {
        actions
            .iter()
            .filter_map(|a| match a {
                SegmenterAction::Job(AsrJob::Final { start_ms, end_ms, samples, .. }) => {
                    Some((*start_ms, *end_ms, samples.len()))
                }
                _ => None,
            })
            .collect()
    }

    fn test_vad() -> VadSettings {
        VadSettings {
            min_silence_ms: 600,
            min_speech_ms: 250,
            speech_pad_ms: 200,
            max_utterance_ms: 25_000,
            ..Default::default()
        }
    }

    /// 构造「静音 → 语音 → 静音」的音频，返回帧与语音的真实起止（毫秒）
    fn speech_clip(speech_frames: usize) -> (Vec<Vec<f32>>, i64, i64) {
        let lead = 10usize;
        let tail = 15usize;
        let mut frames = Vec::new();
        for _ in 0..lead {
            frames.push(quiet_frame());
        }
        for i in 0..speech_frames {
            frames.push(tone_frame(0.3, i * FRAME));
        }
        for _ in 0..tail {
            frames.push(quiet_frame());
        }
        let start = (lead * 100) as i64;
        let end = ((lead + speech_frames) * 100) as i64;
        (frames, start, end)
    }


    #[test]
    fn segmenter_emits_exactly_one_sane_segment() {
        let cfg = test_vad();
        let mut seg = Segmenter::new(&cfg, true);
        let (frames, speech_start, speech_end) = speech_clip(30); // 3 秒语音
        let actions = drive(&mut seg, &frames);
        let flushed = seg.flush();
        let mut all = actions;
        all.extend(flushed);

        let f = finals(&all);
        assert_eq!(f.len(), 1, "应只定稿一句话，实际：{f:?}");
        let (start, end, samples) = f[0];
        let utt_ms = samples as i64 * 1000 / crate::audio::vad::RATE as i64;

        // 起点应落在真实语音起点之前一点（有 400ms 前滚）
        assert!(
            (start - (speech_start - PRE_ROLL_MS)).abs() <= 250,
            "起点偏差过大：{start} vs {}",
            speech_start - PRE_ROLL_MS
        );
        // 终点应接近真实语音终点（含 speech_pad 留白）
        assert!(
            (end - speech_end).abs() <= 500,
            "终点偏差过大：{end} vs {speech_end}"
        );
        // 关键：不能出现退化的极短片段
        assert!(
            end - start >= 2_500,
            "片段过短：{start} → {end}（音频实际 {utt_ms} ms）"
        );
        // 终点必须和音频长度大致吻合
        // 段长不应超过音频缓冲长度，且必须覆盖真实语音时长
        assert!(
            end - start <= utt_ms,
            "段长不应超过音频缓冲：段长 {} ms，缓冲 {utt_ms} ms",
            end - start
        );
        assert!(end - start >= speech_end - speech_start, "段长应覆盖真实语音");
    }

    #[test]
    fn degenerate_end_is_replaced_by_audio_length() {
        // 回归测试：VAD 某轮返回空片段时，曾经会用一个「几乎等于起点」的终点去定稿，
        // 产生 end - start == 1ms 的垃圾片段，同时让 speech_ms 大于音频总时长。
        let cfg = test_vad();
        let mut seg = Segmenter::new(&cfg, true);
        let (frames, _, _) = speech_clip(20);
        let mut actions = Vec::new();
        for f in &frames[..30] {
            let levels = levels_for(f);
            actions.extend(seg.push_frame(f, &levels));
        }
        assert!(seg.is_in_speech(), "此时应当处于说话中");

        let start = seg.utt_start_ms;
        let utt_ms = seg.utt.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
        // 故意传入一个荒谬的终点（模拟 VAD 返回空结果时的 last_end = 0）
        seg.finalize(start + 1, &mut actions);

        let f = finals(&actions);
        assert_eq!(f.len(), 1);
        let (s, e, _) = f[0];
        assert!(
            e - s >= utt_ms * 6 / 10,
            "荒谬终点没有被兜底：{s} → {e}，音频长度 {utt_ms} ms"
        );
    }

    #[test]
    fn speech_ms_never_exceeds_audio_ms() {
        let cfg = test_vad();
        let mut seg = Segmenter::new(&cfg, true);
        // 三段语音夹着静音
        let mut frames = Vec::new();
        for _ in 0..8 {
            frames.push(quiet_frame());
        }
        for round in 0..3 {
            for i in 0..15 {
                frames.push(tone_frame(0.3, (round * 15 + i) * FRAME));
            }
            for _ in 0..10 {
                frames.push(quiet_frame());
            }
        }
        let actions = drive(&mut seg, &frames);
        let mut all = actions;
        all.extend(seg.flush());

        assert!(
            seg.total_speech_ms() <= seg.audio_ms(),
            "语音时长 {} 不应大于音频总时长 {}",
            seg.total_speech_ms(),
            seg.audio_ms()
        );
        for (s, e, _) in finals(&all) {
            assert!(e > s, "非法片段：{s} → {e}");
            assert!(e <= seg.audio_ms(), "终点超出音频总长：{e} > {}", seg.audio_ms());
        }
        let f = finals(&all);
        eprintln!("多句场景定稿结果：{f:?}（音频总长 {} ms，语音 {} ms）", seg.audio_ms(), seg.total_speech_ms());
        assert!(f.len() >= 2, "应识别出多句话，实际 {f:?}");
    }

    #[test]
    fn short_pause_inside_a_sentence_does_not_split() {
        let cfg = test_vad();
        let mut seg = Segmenter::new(&cfg, true);
        let mut frames = Vec::new();
        for _ in 0..10 {
            frames.push(quiet_frame());
        }
        for i in 0..15 {
            frames.push(tone_frame(0.3, i * FRAME));
        }
        // 句内 300ms 自然停顿（短于 min_silence 600ms）
        for _ in 0..3 {
            frames.push(quiet_frame());
        }
        for i in 15..30 {
            frames.push(tone_frame(0.3, i * FRAME));
        }
        for _ in 0..15 {
            frames.push(quiet_frame());
        }
        let actions = drive(&mut seg, &frames);
        let f = finals(&actions);
        assert_eq!(f.len(), 1, "句内停顿不应断句：{f:?}");
    }

    #[test]
    fn long_monologue_is_force_split() {
        let mut cfg = test_vad();
        cfg.max_utterance_ms = 3_000; // 压到 3 秒，方便测试
        let mut seg = Segmenter::new(&cfg, true);
        let mut frames = Vec::new();
        for _ in 0..8 {
            frames.push(quiet_frame());
        }
        // 连续 8 秒语音（中间没有停顿）
        for i in 0..80 {
            let amp = if i % 7 == 0 { 0.1 } else { 0.3 };
            frames.push(tone_frame(amp, i * FRAME));
        }
        for _ in 0..15 {
            frames.push(quiet_frame());
        }
        let actions = drive(&mut seg, &frames);
        let mut all = actions;
        all.extend(seg.flush());
        let f = finals(&all);
        assert!(
            f.len() >= 2,
            "超长独白应被强制切成多段，实际 {} 段",
            f.len()
        );
        // 每段都不应超过上限太多，且互相不重叠
        let mut prev_end = -1i64;
        for (s, e, _) in &f {
            assert!(
                *e - *s <= cfg.max_utterance_ms as i64 + 1_000,
                "切分后仍有一段过长：{s} → {e}"
            );
            assert!(*s >= prev_end, "切分后出现时间重叠：{s} < {prev_end}");
            prev_end = *e;
        }
    }

    #[test]
    fn partial_jobs_are_emitted_while_speaking() {
        let cfg = test_vad();
        let mut seg = Segmenter::new(&cfg, true);
        let (frames, _, _) = speech_clip(30);
        let actions = drive(&mut seg, &frames);
        let partials = actions
            .iter()
            .filter(|a| matches!(a, SegmenterAction::Job(AsrJob::Partial { .. })))
            .count();
        // 3 秒语音，每 800ms 一次 → 至少 3 次
        assert!(partials >= 3, "部分结果太少：{partials}");
        // partial 的音频应当随说话不断变长
        let lens: Vec<usize> = actions
            .iter()
            .filter_map(|a| match a {
                SegmenterAction::Job(AsrJob::Partial { samples, .. }) => Some(samples.len()),
                _ => None,
            })
            .collect();
        assert!(lens.windows(2).all(|w| w[1] >= w[0]), "partial 音频应单调增长：{lens:?}");
    }

    /* ------------------------------ 任务合并 ------------------------------ */

    fn dummy_partial(utt_id: u64, len: usize) -> AsrJob {
        AsrJob::Partial {
            utt_id,
            samples: vec![0.0; len],
            start_ms: 0,
            abort: Arc::new(AtomicBool::new(false)),
        }
    }

    fn dummy_final(utt_id: u64) -> AsrJob {
        AsrJob::Final {
            utt_id,
            samples: vec![0.0; 1600],
            start_ms: 0,
            end_ms: 1600,
            share: SourceShare::default(),
        }
    }

    #[test]
    fn job_queue_drops_superseded_partials() {
        let mut q = JobQueue::new();
        q.push(dummy_partial(1, 1600));
        q.push(dummy_partial(1, 3200));
        q.push(dummy_partial(1, 4800));
        assert_eq!(q.len(), 1, "同一句话的旧 partial 应被顶掉");
        match q.pop() {
            Some(AsrJob::Partial { samples, .. }) => assert_eq!(samples.len(), 4800),
            other => panic!("应拿到最新的 partial，实际 {other:?}"),
        }
    }

    #[test]
    fn job_queue_drops_partials_after_final() {
        let mut q = JobQueue::new();
        q.push(dummy_partial(1, 1600));
        q.push(dummy_final(1));
        q.push(dummy_partial(1, 3200)); // 这句话已经定稿，迟到的 partial 必须丢弃
        assert_eq!(q.len(), 1, "定稿后的 partial 应被丢弃");
        assert!(matches!(q.pop(), Some(AsrJob::Final { utt_id: 1, .. })));
        assert!(q.pop().is_none());
    }

    #[test]
    fn job_queue_keeps_different_utterances_in_order() {
        let mut q = JobQueue::new();
        q.push(dummy_partial(1, 1600));
        q.push(dummy_final(1));
        q.push(dummy_partial(2, 1600));
        q.push(dummy_final(2));
        // Final 入队时会清掉同一句话排队的 partial，所以两句的 partial 都已消失
        assert_eq!(q.len(), 2);
        assert!(matches!(q.pop(), Some(AsrJob::Final { utt_id: 1, .. })));
        assert!(matches!(q.pop(), Some(AsrJob::Final { utt_id: 2, .. })));
        assert!(q.pop().is_none());
    }

    /* ------------------------------ 说话人归属 ------------------------------ */

    #[test]
    fn speaker_attribution_by_energy_share() {
        let e = |rms: f32| FrameEnergy { rms, peak: rms, db: 0.0 };

        let mut only_mic = SourceShare::default();
        only_mic.add_frame(SourceKind::Microphone, e(0.5));
        assert_eq!(only_mic.speaker(), Speaker::Me);

        let mut only_loop = SourceShare::default();
        only_loop.add_frame(SourceKind::Loopback, e(0.5));
        assert_eq!(only_loop.speaker(), Speaker::Others);

        let mut both = SourceShare::default();
        both.add_frame(SourceKind::Microphone, e(0.5));
        both.add_frame(SourceKind::Loopback, e(0.5));
        assert_eq!(both.speaker(), Speaker::Mixed);

        // 麦克风明显占优 → 我
        let mut mic_dominant = SourceShare::default();
        mic_dominant.add_frame(SourceKind::Microphone, e(1.0));
        mic_dominant.add_frame(SourceKind::Loopback, e(0.2));
        assert_eq!(mic_dominant.speaker(), Speaker::Me);

        // 全程静音 → 未知
        assert_eq!(SourceShare::default().speaker(), Speaker::Unknown);
    }

    #[test]
    fn single_source_speaker_mapping() {
        assert_eq!(speaker_for_single_source(Some(SourceKind::Microphone)), Speaker::Me);
        assert_eq!(speaker_for_single_source(Some(SourceKind::Loopback)), Speaker::Others);
        assert_eq!(speaker_for_single_source(None), Speaker::Unknown);
    }

    #[test]
    fn vad_engine_is_wired_up() {
        let cfg = test_vad();
        assert_eq!(Segmenter::new(&cfg, true).vad_name(), "energy");
    }
}
