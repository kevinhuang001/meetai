//! 音频采集：设备枚举 + 麦克风/系统内录采集线程 + 混音。
//!
//! 数据流：
//! ```text
//! 设备线程(原生采样率/多声道) ──RawBlock──▶ 混音线程 ──MixedFrame(16k 单声道)──▶ 断句线程
//! ```
//! 混音线程负责：下混单声道 → 重采样到 16kHz → 按增益混合 → 计算电平。
//! 把重采样放在这里（而不是音频回调里）是为了不阻塞实时回调线程。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use serde::Serialize;

use crate::audio::resample::{downmix_to_mono, DcBlocker, Resampler, TARGET_RATE};
use crate::audio::vad::{analyze, FrameEnergy, PeakMeter};
use crate::error::{AppError, AppResult};

/// 混音的基本块长（100ms），兼顾延迟与消息开销
const HOP_MS: u64 = 100;
const HOP_SAMPLES: usize = (TARGET_RATE as u64 * HOP_MS / 1000) as usize;
/// 单个声源最多缓存多少秒（防止设备时钟漂移导致无限增长）
const MAX_BUFFER_SECS: f32 = 2.0;
/// 向下游发送的超时：超时后检查停止标志，避免下游卡住时死锁
const SEND_TIMEOUT: Duration = Duration::from_millis(200);

/* ==========================================================================
 * 类型
 * ========================================================================== */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Microphone,
    Loopback,
}

impl SourceKind {
    /// 设备 ID 的前缀（注意：JSON 序列化用的是 `microphone`/`loopback`，
    /// 由 serde 的 rename_all 负责，这里只用于内部 ID 拼接）
    pub fn id_prefix(&self) -> &'static str {
        match self {
            SourceKind::Microphone => "mic",
            SourceKind::Loopback => "loopback",
        }
    }

    /// 展示用的英文名（与 JSON 保持一致）
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Microphone => "microphone",
            SourceKind::Loopback => "loopback",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceInfo {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    pub detail: String,
    pub is_default: bool,
    pub available: bool,
    pub note: Option<String>,
}

/// 设备 ID 形如 `mic:设备名` / `loopback:设备名`
pub fn make_id(kind: SourceKind, name: &str) -> String {
    format!("{}:{name}", kind.id_prefix())
}

/// 解析设备 ID，返回 (类型, 设备名)。无前缀时按麦克风处理。
pub fn parse_id(id: &str) -> (SourceKind, String) {
    if let Some(rest) = id.strip_prefix("loopback:") {
        (SourceKind::Loopback, rest.to_string())
    } else if let Some(rest) = id.strip_prefix("mic:") {
        (SourceKind::Microphone, rest.to_string())
    } else {
        (SourceKind::Microphone, id.to_string())
    }
}

/// 采集线程产出的原始音频块
struct RawBlock {
    /// 该块属于哪个声源（与 `CaptureHandle::sources` 的下标一致）
    source_index: usize,
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
}

/// 混音后送给断句线程的帧
#[derive(Debug, Clone)]
pub struct MixedFrame {
    /// 16kHz 单声道
    pub samples: Vec<f32>,
    /// 各声源电平（用于 UI 电平条与「说话人归属」判定）
    pub levels: Vec<SourceLevel>,
}

#[derive(Debug, Clone)]
pub struct SourceLevel {
    pub source_id: String,
    pub kind: SourceKind,
    pub energy: FrameEnergy,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub mic_device_id: Option<String>,
    pub loopback_device_id: Option<String>,
    pub mic_gain: f32,
    pub loopback_gain: f32,
}

#[derive(Debug)]
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    pub sources: Vec<SourceDescriptor>,
}

#[derive(Debug, Clone)]
pub struct SourceDescriptor {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
}

impl CaptureHandle {
    /// 停止采集并等待线程退出
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

/* ==========================================================================
 * 设备枚举
 * ========================================================================== */

/// 列出所有可用的采集源（麦克风 + 系统内录）
pub fn list_sources() -> Vec<AudioSourceInfo> {
    let mut out = Vec::new();
    out.extend(list_microphones());
    out.extend(list_loopback());
    out
}

fn list_microphones() -> Vec<AudioSourceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());

    let devices = match host.input_devices() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("枚举麦克风失败：{e}");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for device in devices {
        // cpal 0.18 的 DeviceId 可持久化（形如 "alsa:hw:0,0"），比设备名稳定
        let Ok(id) = device.id() else { continue };
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| id.id().to_string());

        // ALSA 会枚举出一堆插件伪设备（null、dmix、surround…），
        // 它们「可用」但录不到任何真实声音，列出来只会误导用户。
        if is_pseudo_device(&name, id.id()) {
            tracing::debug!("忽略 ALSA 伪设备：{name}（{}）", id.id());
            continue;
        }

        let (detail, available) = match device.default_input_config() {
            Ok(cfg) => (
                format!(
                    "{} Hz · {} 声道 · {}",
                    cfg.sample_rate(),
                    cfg.channels(),
                    sample_format_label(cfg.sample_format())
                ),
                true,
            ),
            Err(e) => (format!("不可用：{e}"), false),
        };

        let is_default = default_id.as_ref() == Some(&id);
        out.push(AudioSourceInfo {
            id: make_id(SourceKind::Microphone, &id.to_string()),
            kind: SourceKind::Microphone,
            label: name,
            detail,
            is_default,
            available,
            note: None,
        });
    }
    out
}

/// ALSA 插件伪设备识别：这些设备能打开、但没有真实硬件输入
fn is_pseudo_device(name: &str, id: &str) -> bool {
    let lower_name = name.to_lowercase();
    let lower_id = id.to_lowercase();

    // 明确没有音频输入的
    const NULL_HINTS: &[&str] = &[
        "discard all samples",
        "generate zero samples",
        "null output",
        "zero samples",
        "dmix",
        "surround21",
        "surround40",
        "surround41",
        "surround50",
        "surround51",
        "surround71",
        "iec958",
        "hdmi",
        "modem",
        "phoneline",
    ];
    if NULL_HINTS.iter().any(|h| lower_name.contains(h)) {
        return true;
    }
    // id 形如 "alsa:null" / "alsa:dmix"
    matches!(lower_id.as_str(), "alsa:null" | "alsa:dmix" | "alsa:default:null")
        || lower_id.ends_with(":null")
}

/// 解析麦克风：优先按可持久化的 DeviceId 匹配，失败再按设备名匹配。
fn resolve_input_device(
    host: &cpal::Host,
    payload: &str,
) -> Option<<cpal::Host as cpal::traits::HostTrait>::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};

    if let Ok(id) = payload.parse::<cpal::DeviceId>() {
        if let Some(device) = host.device_by_id(&id) {
            return Some(device);
        }
    }
    host.input_devices()
        .ok()?
        .find(|d| d.description().map(|x| x.name() == payload).unwrap_or(false))
}

/// 按类型挑一个默认可用的声源 id（用户没有显式选择时使用）
pub fn default_source_id(kind: SourceKind) -> Option<String> {
    let list = list_sources();
    list.iter()
        .find(|s| s.kind == kind && s.is_default && s.available)
        .or_else(|| list.iter().find(|s| s.kind == kind && s.available))
        .map(|s| s.id.clone())
}

fn sample_format_label(f: cpal::SampleFormat) -> &'static str {
    match f {
        cpal::SampleFormat::I8 | cpal::SampleFormat::I16 | cpal::SampleFormat::I32 => "整型",
        cpal::SampleFormat::F32 | cpal::SampleFormat::F64 => "浮点",
        _ => "未知",
    }
}

/// 各平台的系统内录实现
#[cfg(windows)]
fn list_loopback() -> Vec<AudioSourceInfo> {
    crate::audio::loopback::list()
}

#[cfg(target_os = "linux")]
fn list_loopback() -> Vec<AudioSourceInfo> {
    crate::audio::loopback::list()
}

#[cfg(target_os = "macos")]
fn list_loopback() -> Vec<AudioSourceInfo> {
    crate::audio::loopback::list()
}

/* ==========================================================================
 * 启动采集
 * ========================================================================== */

/// 启动采集。返回的句柄 drop/stop 后所有线程退出。
pub fn start(
    config: CaptureConfig,
    frame_tx: Sender<MixedFrame>,
    error_tx: Sender<String>,
) -> AppResult<CaptureHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let (raw_tx, raw_rx) = bounded::<RawBlock>(64);

    let mut handles = Vec::new();
    let mut sources = Vec::new();
    // 声源下标决定混音顺序与电平上报顺序，必须与 sources 一一对应
    let mut next_index = 0usize;

    // ---- 麦克风 ----
    if let Some(device_id) = config.mic_device_id.clone() {
        let (kind, name) = parse_id(&device_id);
        debug_assert_eq!(kind, SourceKind::Microphone);
        let index = next_index;
        next_index += 1;
        let tx = raw_tx.clone();
        let stop_flag = Arc::clone(&stop);
        let err = error_tx.clone();
        let label = name.clone();
        let handle = std::thread::Builder::new()
            .name("mh-mic".into())
            .spawn(move || {
                let result = run_cpal_input(&name, stop_flag, move |samples, ch, rate| {
                    let _ = tx.try_send(RawBlock {
                        source_index: index,
                        samples: samples.to_vec(),
                        channels: ch,
                        rate,
                    });
                });
                if let Err(e) = result {
                    tracing::error!("麦克风采集失败：{e}");
                    let _ = err.send(format!("麦克风采集失败：{e}"));
                }
            })
            .map_err(|e| AppError::audio(format!("无法启动麦克风线程：{e}")))?;
        handles.push(handle);
        sources.push(SourceDescriptor {
            id: device_id.clone(),
            kind: SourceKind::Microphone,
            label: format!("麦克风（{label}）"),
        });
    }

    // ---- 系统内录 ----
    if let Some(device_id) = config.loopback_device_id.clone() {
        let (kind, name) = parse_id(&device_id);
        debug_assert_eq!(kind, SourceKind::Loopback);
        let index = next_index;
        let tx = raw_tx.clone();
        let stop_flag = Arc::clone(&stop);
        let err = error_tx.clone();
        let label = name.clone();
        let handle = std::thread::Builder::new()
            .name("mh-loopback".into())
            .spawn(move || {
                let result = crate::audio::loopback::run(&name, stop_flag, move |samples, ch, rate| {
                    let _ = tx.try_send(RawBlock {
                        source_index: index,
                        samples: samples.to_vec(),
                        channels: ch,
                        rate,
                    });
                });
                if let Err(e) = result {
                    tracing::error!("系统内录失败：{e}");
                    let _ = err.send(format!("系统内录失败：{e}"));
                }
            })
            .map_err(|e| AppError::audio(format!("无法启动系统内录线程：{e}")))?;
        handles.push(handle);
        sources.push(SourceDescriptor {
            id: device_id.clone(),
            kind: SourceKind::Loopback,
            label: format!("系统声音（{label}）"),
        });
    }

    if sources.is_empty() {
        return Err(AppError::audio("没有选择任何音频来源，请至少勾选麦克风或系统内录"));
    }

    // 采集线程都持有 raw_tx 的克隆，这里丢掉主线程的克隆，通道才能在其全部退出后关闭
    drop(raw_tx);

    // ---- 混音线程 ----
    let mix_sources = sources.clone();
    let gains = (
        config.mic_gain.clamp(0.0, 4.0),
        config.loopback_gain.clamp(0.0, 4.0),
    );
    let mix_error = error_tx;
    let mix_stop = Arc::clone(&stop);
    let mixer = std::thread::Builder::new()
        .name("mh-mixer".into())
        .spawn(move || {
            if let Err(e) = run_mixer(raw_rx, frame_tx, mix_sources, gains, mix_stop) {
                tracing::error!("混音线程异常：{e}");
                let _ = mix_error.send(format!("音频混音异常：{e}"));
            }
        })
        .map_err(|e| AppError::audio(format!("无法启动混音线程：{e}")))?;
    handles.push(mixer);

    Ok(CaptureHandle { stop, handles, sources })
}

/* ==========================================================================
 * 麦克风（cpal，三平台通用）
 * ========================================================================== */

/// 用 cpal 打开一个输入设备，把原始多声道数据回调出去。
///
/// 麦克风（三平台）与 macOS 的虚拟声卡内录共用这条路径。
pub(crate) fn run_cpal_input(
    name: &str,
    stop: Arc<AtomicBool>,
    on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
) -> AppResult<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    // 空字符串 / "default" 表示系统默认输入设备
    let device = if name.trim().is_empty() || name.eq_ignore_ascii_case("default") {
        host.default_input_device()
            .ok_or_else(|| AppError::audio("系统没有可用的默认输入设备"))?
    } else {
        resolve_input_device(&host, name)
            .ok_or_else(|| AppError::audio(format!("找不到输入设备「{name}」，可能已被拔出")))?
    };

    let supported = device
        .default_input_config()
        .map_err(|e| AppError::audio(format!("读取设备默认配置失败：{e}")))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let channels = config.channels;
    let rate = config.sample_rate;

    let display_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| name.to_string());
    tracing::info!("打开输入设备「{display_name}」：{rate} Hz · {channels} 声道 · {sample_format:?}");

    let error_cb = |e: cpal::Error| {
        tracing::error!("音频流错误：{e}");
    };

    // 设备可能支持多种采样格式，按实际格式构造对应类型的流。
    // 各分支互斥，因此 on_samples 可以在每个分支里被移动。
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_input_stream::<f32, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        cpal::SampleFormat::I16 => build_input_stream::<i16, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        cpal::SampleFormat::I32 => build_input_stream::<i32, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        cpal::SampleFormat::I8 => build_input_stream::<i8, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        cpal::SampleFormat::U8 => build_input_stream::<u8, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        cpal::SampleFormat::F64 => build_input_stream::<f64, _>(&device, config.clone(), channels, rate, on_samples, error_cb),
        other => {
            return Err(AppError::audio(format!("不支持的采样格式：{other:?}")));
        }
    }?;

    stream
        .play()
        .map_err(|e| AppError::audio(format!("启动音频流失败：{e}")))?;

    // cpal 的 Stream 在部分平台上不是 Send，必须留在创建它的线程里存活
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);
    Ok(())
}

fn build_input_stream<T, F>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: u16,
    rate: u32,
    mut on_samples: F,
    error_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> AppResult<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    F: FnMut(&[f32], u16, u32) + Send + 'static,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;

    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.reserve(data.len());
                for s in data {
                    scratch.push(s.to_sample::<f32>());
                }
                on_samples(&scratch, channels, rate);
            },
            error_cb,
            Some(Duration::from_millis(2_000)),
        )
        .map_err(|e| AppError::audio(format!("创建音频流失败：{e}")))
}

/* ==========================================================================
 * 混音线程
 * ========================================================================== */

struct SourceState {
    id: String,
    kind: SourceKind,
    resampler: Resampler,
    dc: DcBlocker,
    pending: VecDeque<f32>,
    meter: PeakMeter,
    last_data: Instant,
    gain: f32,
}

fn run_mixer(
    rx: Receiver<RawBlock>,
    out: Sender<MixedFrame>,
    sources: Vec<SourceDescriptor>,
    gains: (f32, f32),
    stop: Arc<AtomicBool>,
) -> AppResult<()> {
    let mut states: Vec<SourceState> = sources
        .iter()
        .map(|s| SourceState {
            id: s.id.clone(),
            kind: s.kind,
            resampler: Resampler::new(TARGET_RATE, TARGET_RATE), // 真实采样率在首块数据时设定
            dc: DcBlocker::new(TARGET_RATE, 70.0),
            pending: VecDeque::new(),
            meter: PeakMeter::default(),
            last_data: Instant::now(),
            gain: match s.kind {
                SourceKind::Microphone => gains.0,
                SourceKind::Loopback => gains.1,
            },
        })
        .collect();

    let mut mono: Vec<f32> = Vec::new();
    let mut resampled: Vec<f32> = Vec::new();
    let max_pending = (TARGET_RATE as f32 * MAX_BUFFER_SECS) as usize;

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        // 有数据就处理，没数据最多等 20ms
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(block) => {
                // 数据块自带声源下标，不依赖通道到达顺序
                let Some(state) = states.get_mut(block.source_index) else {
                    tracing::warn!("收到未知声源下标 {} 的数据，已丢弃", block.source_index);
                    continue;
                };

                if state.resampler.in_rate() != block.rate && state.pending.is_empty() {
                    state.resampler = Resampler::new(block.rate, TARGET_RATE);
                    tracing::info!(
                        "声源 {} 采样率 {} Hz → {} Hz",
                        state.id,
                        block.rate,
                        TARGET_RATE
                    );
                }

                mono.clear();
                downmix_to_mono(&block.samples, block.channels, &mut mono);
                state.dc.process_in_place(&mut mono);
                resampled.clear();
                state.resampler.process(&mono, &mut resampled);
                state.pending.extend(resampled.iter().copied());
                if state.pending.len() > max_pending {
                    let drop = state.pending.len() - max_pending;
                    state.pending.drain(..drop);
                    tracing::warn!("声源 {} 缓冲溢出，丢弃 {drop} 个采样", state.id);
                }
                state.last_data = Instant::now();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // 所有采集线程都退出了：把残余数据吐完就结束
                flush_remaining(&mut states, &out)?;
                return Ok(());
            }
        }

        // 只有「真的有数据可混」才产帧。
        //
        // 关键：不能因为某一路没数据就无条件产出静音帧 —— 设备掉线时会变成无限产帧，
        // 把下游通道塞满并卡死整条链路。某一路掉线时，另一路的数据仍会正常驱动产帧
        //（缺失的一路按静音参与混音），既保持时间线连续，也不会失控。
        let ready = states.iter().any(|s| s.pending.len() >= HOP_SAMPLES);
        if !ready {
            continue;
        }

        let mut mixed = vec![0.0f32; HOP_SAMPLES];
        let mut levels = Vec::with_capacity(states.len());
        for state in states.iter_mut() {
            let take = state.pending.len().min(HOP_SAMPLES);
            let mut frame = vec![0.0f32; HOP_SAMPLES];
            for (i, v) in state.pending.drain(..take).enumerate() {
                frame[i] = v * state.gain;
            }
            let energy = analyze(&frame);
            state.meter.push(energy, 0.85);
            levels.push(SourceLevel {
                source_id: state.id.clone(),
                kind: state.kind,
                energy: FrameEnergy {
                    rms: state.meter.rms(),
                    peak: state.meter.peak(),
                    db: state.meter.db(),
                },
            });
            // 多路相加后再限幅，避免削顶失真
            for (dst, src) in mixed.iter_mut().zip(frame.iter()) {
                *dst += *src;
            }
        }

        // 软限幅：超过 ±1 时按双曲正切压缩，保留可懂度
        for v in mixed.iter_mut() {
            if v.abs() > 0.98 {
                *v = v.signum() * (0.98 + (v.abs() - 0.98) / (1.0 + (v.abs() - 0.98)) * 0.02);
            }
        }

        // 带超时发送：下游卡住时也能通过停止标志退出，绝不永久阻塞
        match out.send_timeout(MixedFrame { samples: mixed, levels }, SEND_TIMEOUT) {
            Ok(()) => {}
            Err(crossbeam_channel::SendTimeoutError::Timeout(_)) => {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
            }
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                // 下游已关闭（用户停止录音）
                return Ok(());
            }
        }
    }
}

fn flush_remaining(states: &mut [SourceState], out: &Sender<MixedFrame>) -> AppResult<()> {
    loop {
        let any = states.iter().any(|s| !s.pending.is_empty());
        if !any {
            return Ok(());
        }
        let mut mixed = vec![0.0f32; HOP_SAMPLES];
        let mut levels = Vec::with_capacity(states.len());
        for state in states.iter_mut() {
            let take = state.pending.len().min(HOP_SAMPLES);
            let mut frame = vec![0.0f32; HOP_SAMPLES];
            for (i, v) in state.pending.drain(..take).enumerate() {
                frame[i] = v * state.gain;
            }
            let energy = analyze(&frame);
            levels.push(SourceLevel {
                source_id: state.id.clone(),
                kind: state.kind,
                energy,
            });
            for (dst, src) in mixed.iter_mut().zip(frame.iter()) {
                *dst += *src;
            }
        }
        match out.send_timeout(MixedFrame { samples: mixed, levels }, SEND_TIMEOUT) {
            Ok(()) => {}
            // 收尾阶段下游若已满/已关闭，直接放弃剩余数据
            Err(_) => return Ok(()),
        }
    }
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        let id = make_id(SourceKind::Loopback, "扬声器 (Realtek)");
        assert_eq!(id, "loopback:扬声器 (Realtek)");
        let (kind, name) = parse_id(&id);
        assert_eq!(kind, SourceKind::Loopback);
        assert_eq!(name, "扬声器 (Realtek)");

        let (kind, name) = parse_id("mic:MacBook 麦克风");
        assert_eq!(kind, SourceKind::Microphone);
        assert_eq!(name, "MacBook 麦克风");

        // 没有前缀时按麦克风处理
        let (kind, name) = parse_id("裸设备名");
        assert_eq!(kind, SourceKind::Microphone);
        assert_eq!(name, "裸设备名");
    }

    #[test]
    fn id_prefix_roundtrips_with_parse_id() {
        // 曾经的 bug：make_id 用 "microphone:" 前缀，而 parse_id 只认 "mic:"，
        // 结果麦克风永远解析不到设备名，录音直接失败。这里锁死两者的一致性。
        for kind in [SourceKind::Microphone, SourceKind::Loopback] {
            let id = make_id(kind, "设备:名字");
            let (parsed_kind, payload) = parse_id(&id);
            assert_eq!(parsed_kind, kind, "前缀与解析不一致：{id}");
            assert_eq!(payload, "设备:名字", "类型前缀被误当成设备名：{id}");
        }
    }

    #[test]
    fn source_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&SourceKind::Microphone).unwrap(),
            "\"microphone\""
        );
        assert_eq!(
            serde_json::to_string(&SourceKind::Loopback).unwrap(),
            "\"loopback\""
        );
    }

    /// 本机（CI/无音频设备）上枚举不能 panic，只能优雅地返回空列表
    #[test]
    fn listing_sources_never_panics() {
        let list = list_sources();
        for s in &list {
            assert!(!s.id.is_empty());
            assert!(!s.label.is_empty());
            assert!(!s.detail.is_empty());
        }
    }

    #[test]
    fn pseudo_devices_are_filtered_out() {
        // 这台机器（容器/无音频）上 ALSA 会报出这些假设备
        assert!(is_pseudo_device(
            "Discard all samples (playback) or generate zero samples (capture)",
            "alsa:null"
        ));
        assert!(is_pseudo_device("Null Output", "alsa:null"));
        assert!(is_pseudo_device("dmix", "alsa:dmix"));
        assert!(is_pseudo_device("HDA Intel HDMI", "alsa:hdmi:0"));
        // 真实设备与 PulseAudio 转发设备要保留
        assert!(!is_pseudo_device("MacBook Pro 麦克风", "coreaudio:1"));
        assert!(!is_pseudo_device("Playback/recording through the PulseAudio sound server", "alsa:pulse"));
        assert!(!is_pseudo_device("USB 会议全向麦", "alsa:hw:1,0"));
    }

    #[test]
    fn list_sources_excludes_pseudo_devices() {
        let list = list_microphones();
        for item in &list {
            assert!(
                !item.label.to_lowercase().contains("discard all samples"),
                "伪设备不应出现在列表里：{}",
                item.label
            );
        }
    }

    #[test]
    fn start_without_sources_is_an_error() {
        let (frame_tx, _frame_rx) = bounded(4);
        let (err_tx, _err_rx) = bounded(4);
        let result = start(
            CaptureConfig {
                mic_device_id: None,
                loopback_device_id: None,
                mic_gain: 1.0,
                loopback_gain: 1.0,
            },
            frame_tx,
            err_tx,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("没有选择任何音频来源"));
    }

    #[test]
    fn start_with_bogus_device_reports_error_not_panic() {
        let (frame_tx, _frame_rx) = bounded(4);
        let (err_tx, err_rx) = bounded(4);
        let handle = start(
            CaptureConfig {
                mic_device_id: Some("mic:这个设备不存在".into()),
                loopback_device_id: None,
                mic_gain: 1.0,
                loopback_gain: 1.0,
            },
            frame_tx,
            err_tx,
        )
        .expect("启动本身应当成功（错误在采集线程里上报）");

        // 采集线程应该上报一条可读错误。
        // 超时给得比较宽松：macOS 的 CoreAudio 在没有音频设备的 runner 上
        // 初始化可能要好几种，卡在 5 秒会变成偶发失败。
        let msg = err_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("应当收到设备错误");
        assert!(msg.contains("麦克风采集失败"), "实际：{msg}");
        handle.stop();
    }

    #[test]
    fn mixer_routes_blocks_by_source_index() {
        // 两路同采样率，靠 source_index 区分；若实现退回「按到达顺序」会串源
        let (raw_tx, raw_rx) = bounded::<RawBlock>(64);
        let (frame_tx, frame_rx) = bounded::<MixedFrame>(64);
        let sources = vec![
            SourceDescriptor {
                id: "mic:m".into(),
                kind: SourceKind::Microphone,
                label: "麦克风".into(),
            },
            SourceDescriptor {
                id: "loopback:l".into(),
                kind: SourceKind::Loopback,
                label: "系统声音".into(),
            },
        ];

        let producer = std::thread::spawn(move || {
            for _ in 0..8 {
                // 0 号声源：440Hz 正弦（有声音）；1 号声源：静音 —— 用它判断有没有串源
                let mut mic = Vec::new();
                let mut loopback = Vec::new();
                for i in 0..4_800 {
                    let v = 0.5
                        * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
                    mic.push(v);
                    mic.push(v);
                    loopback.push(0.0f32);
                    loopback.push(0.0f32);
                }
                let _ = raw_tx.send(RawBlock { source_index: 0, samples: mic, channels: 2, rate: 48_000 });
                let _ = raw_tx.send(RawBlock { source_index: 1, samples: loopback, channels: 2, rate: 48_000 });
                std::thread::sleep(Duration::from_millis(40));
            }
        });

        let mixer = std::thread::spawn(move || {
            run_mixer(raw_rx, frame_tx, sources, (1.0, 1.0), Arc::new(AtomicBool::new(false))).unwrap();
        });

        let mut checked = 0;
        while checked < 3 {
            let Ok(frame) = frame_rx.recv_timeout(Duration::from_secs(5)) else {
                break;
            };
            assert_eq!(frame.levels.len(), 2);
            // 有声音的一路电平应明显非零；静音的那一路必须接近零（否则说明串源了）
            assert!(
                frame.levels[0].energy.rms > 0.2,
                "麦克风电平不应为静音：{:?}",
                frame.levels[0]
            );
            assert!(
                frame.levels[1].energy.rms < 0.01,
                "系统声音应为静音，实际 {:?}（说明串源了）",
                frame.levels[1]
            );
            checked += 1;
        }
        assert!(checked >= 3, "应至少收到 3 帧");

        producer.join().unwrap();
        mixer.join().unwrap();
    }

    #[test]
    fn mixer_emits_16k_mono_frames_and_levels() {
        let (raw_tx, raw_rx) = bounded::<RawBlock>(64);
        let (frame_tx, frame_rx) = bounded::<MixedFrame>(64);
        let sources = vec![SourceDescriptor {
            id: "mic:测试".into(),
            kind: SourceKind::Microphone,
            label: "测试麦克风".into(),
        }];

        let producer = std::thread::spawn(move || {
            // 48000Hz 立体声正弦，持续 1 秒
            for _ in 0..10 {
                let mut samples = Vec::with_capacity(4_800 * 2);
                for i in 0..4_800 {
                    let v = 0.5
                        * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
                    samples.push(v);
                    samples.push(v);
                }
                let _ = raw_tx.send(RawBlock {
                    source_index: 0,
                    samples,
                    channels: 2,
                    rate: 48_000,
                });
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        let mixer = std::thread::spawn(move || {
            run_mixer(raw_rx, frame_tx, sources, (1.0, 1.0), Arc::new(AtomicBool::new(false))).unwrap();
        });

        let mut total = 0usize;
        let mut saw_level = false;
        for _ in 0..6 {
            match frame_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(frame) => {
                    assert_eq!(frame.samples.len(), HOP_SAMPLES, "混音帧长度应为 100ms");
                    assert_eq!(frame.levels.len(), 1);
                    if frame.levels[0].energy.peak > 0.1 {
                        saw_level = true;
                    }
                    total += frame.samples.len();
                }
                Err(_) => break,
            }
        }
        assert!(total >= HOP_SAMPLES * 4, "应至少产出 4 帧，实际 {total} 个采样");
        assert!(saw_level, "电平表应当检测到信号");

        producer.join().unwrap();
        mixer.join().unwrap();
    }

    #[test]
    fn mixer_emits_silence_when_a_source_stalls() {
        // 一路有数据、一路掉线：仍应持续产出帧（否则整场录音会卡死）
        let (raw_tx, raw_rx) = bounded::<RawBlock>(64);
        let (frame_tx, frame_rx) = bounded::<MixedFrame>(16);
        let sources = vec![
            SourceDescriptor { id: "mic:m".into(), kind: SourceKind::Microphone, label: "m".into() },
            SourceDescriptor { id: "loopback:l".into(), kind: SourceKind::Loopback, label: "l".into() },
        ];

        let producer = std::thread::spawn(move || {
            for _ in 0..6 {
                let samples = vec![0.3f32; 4_800 * 2];
                let _ = raw_tx.send(RawBlock { source_index: 0, samples, channels: 2, rate: 48_000 });
                std::thread::sleep(Duration::from_millis(60));
            }
            // 之后不再发送 1 号源的数据，模拟掉线
            std::thread::sleep(Duration::from_millis(800));
        });

        let mixer = std::thread::spawn(move || {
            run_mixer(raw_rx, frame_tx, sources, (1.0, 1.0), Arc::new(AtomicBool::new(false))).unwrap();
        });

        let mut frames = 0;
        while frames < 5 {
            match frame_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(f) => {
                    assert_eq!(f.samples.len(), HOP_SAMPLES);
                    frames += 1;
                }
                Err(_) => break,
            }
        }
        assert!(frames >= 5, "掉线声源不应阻塞输出，实际 {frames} 帧");

        producer.join().unwrap();
        mixer.join().unwrap();
    }

    #[test]
    fn mixer_ends_when_upstream_channel_closes() {
        let (raw_tx, raw_rx) = bounded::<RawBlock>(4);
        let (frame_tx, _frame_rx) = bounded::<MixedFrame>(4);
        drop(raw_tx); // 上游立即关闭
        let sources = vec![SourceDescriptor {
            id: "mic:x".into(),
            kind: SourceKind::Microphone,
            label: "x".into(),
        }];
        // 应当正常返回而不是死循环
        run_mixer(raw_rx, frame_tx, sources, (1.0, 1.0), Arc::new(AtomicBool::new(false))).unwrap();
    }
}
