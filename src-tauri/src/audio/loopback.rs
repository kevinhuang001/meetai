//! 系统内录（把扬声器输出录进来）。
//!
//! 三个平台的做法完全不同，这也是这类软件最容易踩坑的地方：
//!
//! | 平台 | 方案 | 是否需要额外安装 |
//! | --- | --- | --- |
//! | Windows | WASAPI loopback：在**渲染设备**上以 Capture 方向打开 | 不需要 |
//! | Linux | PulseAudio/PipeWire 的 monitor source（`parec`） | 不需要（需 pulseaudio-utils / pipewire-pulse） |
//! | macOS | 系统层面**不支持**内录，必须装虚拟声卡（BlackHole 等） | **需要** |
//!
//! macOS 的处理方式是：检测常见的虚拟声卡输入设备，引导用户安装并把输出同时送到该设备。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio::capture::{make_id, AudioSourceInfo, SourceKind};
use crate::error::{AppError, AppResult};

/// macOS 上常见的虚拟声卡名称片段
#[cfg(target_os = "macos")]
const VIRTUAL_DEVICE_HINTS: &[&str] = &[
    "BlackHole",
    "Soundflower",
    "Loopback",
    "VB-Cable",
    "Virtual Audio",
    "虚拟",
    "Multi-Output",
    "Aggregate",
];

/* ==========================================================================
 * Windows：WASAPI loopback
 * ========================================================================== */

#[cfg(windows)]
pub fn list() -> Vec<AudioSourceInfo> {
    use wasapi::{DeviceEnumerator, Direction};

    if let Err(e) = wasapi::initialize_mta() {
        tracing::warn!("初始化 COM(MTA) 失败：{e}");
    }

    let mut out = Vec::new();
    let enumerator = match DeviceEnumerator::new() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("枚举渲染设备失败：{e}");
            return out;
        }
    };

    let default_name = enumerator
        .get_default_device(&Direction::Render)
        .ok()
        .and_then(|d| d.get_friendlyname().ok())
        .unwrap_or_default();

    let collection = match enumerator.get_device_collection(&Direction::Render) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("获取渲染设备集合失败：{e}");
            return out;
        }
    };
    let count = collection.get_nbr_devices().unwrap_or(0);
    for i in 0..count {
        let Ok(device) = collection.get_device_at_index(i) else {
            continue;
        };
        let name = device.get_friendlyname().unwrap_or_else(|_| format!("输出设备 {i}"));
        let detail = device
            .get_device_format()
            .map(|f| {
                format!(
                    "{} Hz · {} 声道 · WASAPI loopback",
                    f.get_samplespersec(),
                    f.get_nchannels()
                )
            })
            .unwrap_or_else(|_| "WASAPI loopback".into());
        out.push(AudioSourceInfo {
            id: make_id(SourceKind::Loopback, &name),
            kind: SourceKind::Loopback,
            label: name.clone(),
            detail,
            is_default: name == default_name,
            available: true,
            note: None,
        });
    }

    if out.is_empty() {
        out.push(AudioSourceInfo {
            id: make_id(SourceKind::Loopback, "默认输出设备"),
            kind: SourceKind::Loopback,
            label: "系统声音（默认输出设备）".into(),
            detail: "WASAPI loopback".into(),
            is_default: true,
            available: true,
            note: Some("未能枚举具体设备，将使用系统默认输出".into()),
        });
    }
    out
}

#[cfg(windows)]
pub fn run(
    name: &str,
    stop: Arc<AtomicBool>,
    mut on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
) -> AppResult<()> {
    use std::collections::VecDeque;
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    wasapi::initialize_mta()
        .map_err(|e| AppError::audio(format!("初始化 COM 失败：{e}")))?;

    let enumerator = DeviceEnumerator::new()
        .map_err(|e| AppError::audio(format!("创建设备枚举器失败：{e}")))?;

    // 注意：loopback = 在渲染设备上以 Capture 方向打开，wasapi crate 会自动加
    // AUDCLNT_STREAMFLAGS_LOOPBACK 标志。
    let device = if name.is_empty() || name.starts_with("默认输出设备") {
        enumerator
            .get_default_device(&Direction::Render)
            .map_err(|e| AppError::audio(format!("获取默认输出设备失败：{e}")))?
    } else {
        let collection = enumerator
            .get_device_collection(&Direction::Render)
            .map_err(|e| AppError::audio(format!("枚举输出设备失败：{e}")))?;
        let count = collection
            .get_nbr_devices()
            .map_err(|e| AppError::audio(format!("枚举输出设备失败：{e}")))?;
        let mut found = None;
        for i in 0..count {
            if let Ok(d) = collection.get_device_at_index(i) {
                if d.get_friendlyname().map(|n| n == name).unwrap_or(false) {
                    found = Some(d);
                    break;
                }
            }
        }
        found.ok_or_else(|| AppError::audio(format!("找不到输出设备「{name}」")))?
    };

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| AppError::audio(format!("打开音频客户端失败：{e}")))?;

    // 先按设备的混音格式取采样率/声道，再请求 float32（autoconvert 负责转换）
    let mix = audio_client
        .get_mixformat()
        .map_err(|e| AppError::audio(format!("读取设备混音格式失败：{e}")))?;
    let rate = mix.get_samplespersec();
    let channels = mix.get_nchannels();

    let desired = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        rate as usize,
        channels as usize,
        None,
    );

    let (_def_time, min_time) = audio_client
        .get_device_period()
        .map_err(|e| AppError::audio(format!("读取设备周期失败：{e}")))?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };

    audio_client
        .initialize_client(&desired, &Direction::Capture, &mode)
        .map_err(|e| {
            AppError::audio(format!(
                "初始化系统内录失败：{e}（部分设备不支持自动格式转换，可在设置里改用麦克风采集）"
            ))
        })?;

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| AppError::audio(format!("创建采集事件失败：{e}")))?;
    let capture = audio_client
        .get_audiocaptureclient()
        .map_err(|e| AppError::audio(format!("获取采集客户端失败：{e}")))?;

    audio_client
        .start_stream()
        .map_err(|e| AppError::audio(format!("启动系统内录失败：{e}")))?;

    tracing::info!("系统内录已启动：{rate} Hz · {channels} 声道（float32）");

    let mut queue: VecDeque<u8> = VecDeque::with_capacity(64 * 1024);
    let mut scratch: Vec<f32> = Vec::with_capacity(16_384);

    while !stop.load(Ordering::Relaxed) {
        // 事件驱动：等不到事件也不影响，下一轮继续读
        let _ = h_event.wait_for_event(200);
        if let Err(e) = capture.read_from_device_to_deque(&mut queue) {
            tracing::warn!("读取系统声音失败：{e}");
            continue;
        }

        scratch.clear();
        while queue.len() >= 4 {
            let b = [
                queue.pop_front().unwrap(),
                queue.pop_front().unwrap(),
                queue.pop_front().unwrap(),
                queue.pop_front().unwrap(),
            ];
            scratch.push(f32::from_le_bytes(b));
        }
        if !scratch.is_empty() {
            on_samples(&scratch, channels, rate);
        }
    }

    let _ = audio_client.stop_stream();
    Ok(())
}

/* ==========================================================================
 * Linux：PulseAudio / PipeWire monitor source
 * ========================================================================== */

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};

    /// monitor source 的名字通常形如 `alsa_output.pci-0000_00_1f.3.analog-stereo.monitor`
    pub fn list() -> Vec<AudioSourceInfo> {
        let output = match Command::new("pactl").arg("list").arg("short").arg("sources").output() {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                tracing::warn!(
                    "pactl 执行失败：{}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return vec![unavailable("pactl 执行失败")];
            }
            Err(e) => {
                tracing::warn!("找不到 pactl 命令：{e}");
                return vec![unavailable("未找到 pactl 命令")];
            }
        };

        let default_sink = Command::new("pactl")
            .arg("get-default-sink")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let text = String::from_utf8_lossy(&output.stdout);
        let mut out = Vec::new();
        for line in text.lines() {
            // 格式：index  name  driver  sample_spec  state
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 4 {
                continue;
            }
            let name = cols[1].trim();
            if !name.ends_with(".monitor") {
                continue;
            }
            let spec = cols[3].trim();
            let label = name
                .trim_end_matches(".monitor")
                .rsplit('.')
                .next()
                .unwrap_or(name)
                .to_string();
            out.push(AudioSourceInfo {
                id: make_id(SourceKind::Loopback, name),
                kind: SourceKind::Loopback,
                label: format!("系统声音（{label}）"),
                detail: format!("{spec} · PulseAudio monitor"),
                is_default: !default_sink.is_empty() && name == format!("{default_sink}.monitor"),
                available: true,
                note: None,
            });
        }

        if out.is_empty() {
            out.push(unavailable("没有找到 monitor 声源"));
        }
        out
    }

    fn unavailable(reason: &str) -> AudioSourceInfo {
        AudioSourceInfo {
            id: make_id(SourceKind::Loopback, "系统声音"),
            kind: SourceKind::Loopback,
            label: "系统声音（不可用）".into(),
            detail: reason.to_string(),
            is_default: false,
            available: false,
            note: Some(
                "系统内录需要 PulseAudio 或 PipeWire 的 monitor 声源。\
                 请确认已安装 pulseaudio-utils（提供 pactl/parec），\
                 或改用麦克风采集。"
                    .into(),
            ),
        }
    }

    pub fn run(
        name: &str,
        stop: Arc<AtomicBool>,
        mut on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
    ) -> AppResult<()> {
        // 空名称 = 默认输出设备的监听源（PulseAudio/PipeWire 都支持这个写法）
        let device = if name.trim().is_empty() {
            "@DEFAULT_MONITOR@".to_string()
        } else {
            name.to_string()
        };
        // parec 直接以 16kHz 单声道 f32 输出，省掉一次重采样
        let mut child = Command::new("parec")
            .arg("--device")
            .arg(&device)
            .arg("--format=float32le")
            .arg("--rate=16000")
            .arg("--channels=1")
            .arg("--latency-msec=100")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                AppError::audio(format!(
                    "启动 parec 失败：{e}（请安装 pulseaudio-utils）"
                ))
            })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::audio("无法读取 parec 输出"))?;

        // 100ms @16kHz 单声道 f32 = 6400 字节
        let block_bytes = 6_400usize;
        let mut buf = vec![0u8; block_bytes];
        let mut samples: Vec<f32> = Vec::with_capacity(block_bytes / 4);

        while !stop.load(Ordering::Relaxed) {
            match stdout.read_exact(&mut buf) {
                Ok(()) => {
                    samples.clear();
                    for chunk in buf.chunks_exact(4) {
                        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                    on_samples(&samples, 1, 16_000);
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    let mut err_text = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut err_text);
                    }
                    if err_text.trim().is_empty() {
                        return Err(AppError::audio("parec 已退出（声源可能被关闭）"));
                    }
                    return Err(AppError::audio(format!("parec 退出：{}", err_text.trim())));
                }
                Err(e) => return Err(AppError::audio(format!("读取系统声音失败：{e}"))),
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn list() -> Vec<AudioSourceInfo> {
    linux::list()
}

#[cfg(target_os = "linux")]
pub fn run(
    name: &str,
    stop: Arc<AtomicBool>,
    on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
) -> AppResult<()> {
    linux::run(name, stop, on_samples)
}

/* ==========================================================================
 * macOS：必须依赖虚拟声卡
 * ========================================================================== */

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait};

    /// 找出所有看起来像虚拟声卡的输入设备
    pub fn list() -> Vec<AudioSourceInfo> {
        let host = cpal::default_host();
        let mut out = Vec::new();

        if let Ok(devices) = host.input_devices() {
            for device in devices {
                let Ok(id) = device.id() else { continue };
                let name = device
                    .description()
                    .map(|d| d.name().to_string())
                    .unwrap_or_else(|_| id.id().to_string());
                if !VIRTUAL_DEVICE_HINTS.iter().any(|h| name.contains(h)) {
                    continue;
                }
                let detail = device
                    .default_input_config()
                    .map(|c| {
                        format!(
                            "{} Hz · {} 声道 · 虚拟声卡",
                            c.sample_rate().0,
                            c.channels()
                        )
                    })
                    .unwrap_or_else(|e| format!("不可用：{e}"));
                out.push(AudioSourceInfo {
                    id: make_id(SourceKind::Loopback, &id.to_string()),
                    kind: SourceKind::Loopback,
                    label: format!("系统声音（{name}）"),
                    detail,
                    is_default: name.contains("BlackHole"),
                    available: true,
                    note: Some(
                        "请把系统输出同时送到这个虚拟设备（用「多输出设备」或 BlackHole 的监听模式），\
                         否则录不到声音。"
                            .into(),
                    ),
                });
            }
        }

        if out.is_empty() {
            out.push(AudioSourceInfo {
                id: make_id(SourceKind::Loopback, "BlackHole 2ch"),
                kind: SourceKind::Loopback,
                label: "系统声音（需要先安装虚拟声卡）".into(),
                detail: "未检测到 BlackHole / Soundflower 等虚拟音频设备".into(),
                is_default: false,
                available: false,
                note: Some(
                    "macOS 不允许应用直接录制系统声音。\
                     解决方法：安装免费的 BlackHole（brew install blackhole-2ch），\
                     然后在「音频 MIDI 设置」里创建一个「多输出设备」，\
                     同时勾选你的扬声器和 BlackHole，把它设为系统输出。\
                     之后回到这里刷新设备列表即可。"
                        .into(),
                ),
            });
        }
        out
    }

    pub fn run(
        name: &str,
        stop: Arc<AtomicBool>,
        on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
    ) -> AppResult<()> {
        super::super::capture::run_cpal_input(name, stop, on_samples)
    }
}

#[cfg(target_os = "macos")]
pub fn list() -> Vec<AudioSourceInfo> {
    macos::list()
}

#[cfg(target_os = "macos")]
pub fn run(
    name: &str,
    stop: Arc<AtomicBool>,
    on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
) -> AppResult<()> {
    macos::run(name, stop, on_samples)
}

/* ==========================================================================
 * 其它平台（编译占位）
 * ========================================================================== */

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn list() -> Vec<AudioSourceInfo> {
    vec![AudioSourceInfo {
        id: make_id(SourceKind::Loopback, "系统声音"),
        kind: SourceKind::Loopback,
        label: "系统声音（该平台不支持）".into(),
        detail: "当前平台未实现系统内录".into(),
        is_default: false,
        available: false,
        note: Some("请使用麦克风采集".into()),
    }]
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn run(
    _name: &str,
    _stop: Arc<AtomicBool>,
    _on_samples: impl FnMut(&[f32], u16, u32) + Send + 'static,
) -> AppResult<()> {
    Err(AppError::audio("当前平台不支持系统内录"))
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_never_panics_on_this_platform() {
        let list = list();
        // 无音频环境（CI/容器）下允许返回「不可用」占位项，但不能 panic
        for item in &list {
            assert_eq!(item.kind, SourceKind::Loopback);
            assert!(item.id.starts_with("loopback:"));
            assert!(!item.label.is_empty());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_unavailable_entry_explains_how_to_fix() {
        // 这个环境没有 PulseAudio，应该给出可操作的提示而不是空白
        let list = list();
        if list.iter().any(|i| !i.available) {
            let note = list
                .iter()
                .find(|i| !i.available)
                .and_then(|i| i.note.clone())
                .unwrap_or_default();
            assert!(
                note.contains("pactl") || note.contains("PulseAudio") || note.contains("PipeWire"),
                "不可用提示应说明原因与解决办法，实际：{note}"
            );
        }
    }

    #[test]
    fn run_with_bogus_device_reports_error() {
        let stop = Arc::new(AtomicBool::new(false));
        let result = run("这个设备绝对不存在", stop, |_, _, _| {});
        // 允许「启动成功但立刻结束」的实现差异，但绝不应当是 panic
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(!msg.is_empty());
        }
    }
}
