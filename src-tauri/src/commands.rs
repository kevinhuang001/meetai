//! Tauri 命令层：前端 `src/lib/api.ts` 里的每个方法都对应这里的一个命令。

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::bounded;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::ai::client::ConnectionTest;
use crate::ai::presets::{self, AiPreset};
use crate::asr::client::AsrConnectionTest;
use crate::asr::provider::{self as asr_presets, AsrPreset};
use crate::audio::capture::{self, AudioSourceInfo, CaptureConfig, SourceKind};
use crate::error::{AppError, AppResult};
use crate::events::{event, AiStateKind, AiStatusEvent, Emitter};
use crate::pipeline::{self, PipelineOptions};
use crate::session::{
    default_title, render_export, ExportFormat, Session, SessionConfig, SessionDetail, SessionInfo,
    SessionListItem, SessionStatus,
};
use crate::settings::{AiProvider, AsrProvider, Settings};
use crate::state::AppState;
use crate::util::now_ms;

/* ==========================================================================
 * 设置
 * ========================================================================== */

#[tauri::command(rename_all = "camelCase")]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings_snapshot()
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<Settings> {
    let mut next = settings;
    next.sanitize();
    // 落盘前按设置决定是否保留 api key（内存里始终保留，避免用户每次都要重填）
    state.save_settings(&next.for_persistence())?;
    *state.settings.lock() = next.clone();
    Ok(next)
}

#[tauri::command(rename_all = "camelCase")]
pub fn reset_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    let mut next = Settings::default();
    next.sanitize();
    state.save_settings(&next.for_persistence())?;
    *state.settings.lock() = next.clone();
    Ok(next)
}

/* ==========================================================================
 * AI
 * ========================================================================== */

#[tauri::command(rename_all = "camelCase")]
pub fn list_ai_presets() -> Vec<AiPreset> {
    presets::presets()
}

#[tauri::command(rename_all = "camelCase")]
pub async fn test_ai_connection(app: AppHandle, provider: AiProvider) -> ConnectionTest {
    let client = {
        let state = app.state::<AppState>();
        Arc::clone(&state.ai)
    };
    client.test_connection(&provider).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn summarize_now(app: AppHandle, session_id: String) -> AppResult<()> {
    let (client, settings, session, emitter) = {
        let state = app.state::<AppState>();
        let session = active_session_or_load(&state, &session_id)?;
        (
            Arc::clone(&state.ai),
            state.settings_snapshot().ai,
            session,
            state.emitter() as Arc<dyn Emitter>,
        )
    };
    if !settings.ready() {
        return Err(AppError::ai("请先在设置里配置 AI 接口（Base URL + 模型名 + API Key）"));
    }
    pipeline::summarize_once(&client, &settings, &session, &emitter, true).await?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn generate_report(app: AppHandle, session_id: String) -> AppResult<String> {
    let (client, settings, session, emitter) = {
        let state = app.state::<AppState>();
        let session = active_session_or_load(&state, &session_id)?;
        (
            Arc::clone(&state.ai),
            state.settings_snapshot().ai,
            session,
            state.emitter() as Arc<dyn Emitter>,
        )
    };

    let provider = settings
        .active()
        .ok_or_else(|| AppError::ai("尚未配置 AI 服务商"))?
        .clone();
    if !provider.is_usable() {
        return Err(AppError::ai("AI 接口配置不完整（需要 Base URL 与模型名）"));
    }

    let (messages, meta, title) = {
        let s = session.lock();
        if s.segments.is_empty() {
            return Err(AppError::session("这场会议还没有转写内容，无法生成纪要"));
        }
        let meta = format!(
            "时长 {} · 识别模型 {} · 共 {} 段转写",
            crate::util::format_clock(s.duration_ms),
            s.config.model_id,
            s.segments.len()
        );
        (
            crate::ai::prompts::build_report_messages(
                &s.title,
                &meta,
                &s.full_transcript(usize::MAX),
                settings.max_context_chars as usize * 2,
            ),
            meta,
            s.title.clone(),
        )
    };
    let _ = (meta, title);

    crate::events::emit(
        &emitter,
        event::AI_STATUS,
        &AiStatusEvent {
            session_id: session_id.clone(),
            state: AiStateKind::Thinking,
            message: Some("正在生成完整会议纪要…".into()),
            calls: session.lock().summary.calls,
        },
    );

    let outcome = client.chat(&provider, &messages).await;
    match outcome {
        Ok(out) => {
            let report = out.content.trim().to_string();
            {
                let mut s = session.lock();
                s.report_md = Some(report.clone());
            }
            // 主动保存一次，避免用户忘记停止录音就关掉
            let snapshot = session.lock().clone();
            let store = { app.state::<AppState>().sessions.dir().to_path_buf() };
            if let Err(e) = crate::session::SessionStore::new(store).save(&snapshot) {
                tracing::warn!("保存完整纪要失败：{e}");
            }
            crate::events::emit(
                &emitter,
                event::AI_STATUS,
                &AiStatusEvent {
                    session_id,
                    state: AiStateKind::Idle,
                    message: Some("完整会议纪要已生成".into()),
                    calls: session.lock().summary.calls,
                },
            );
            Ok(report)
        }
        Err(e) => {
            crate::events::emit(
                &emitter,
                event::AI_STATUS,
                &AiStatusEvent {
                    session_id,
                    state: AiStateKind::Error,
                    message: Some(e.to_string()),
                    calls: session.lock().summary.calls,
                },
            );
            Err(e)
        }
    }
}

/* ==========================================================================
 * 语音识别服务
 * ========================================================================== */

#[tauri::command(rename_all = "camelCase")]
pub fn list_asr_presets() -> Vec<AsrPreset> {
    asr_presets::presets()
}

/// 连通性测试：往识别服务发 0.6 秒静音，验证「地址 + 鉴权 + 模型名」都对。
///
/// 用静音是为了让这个测试在任何环境都能跑；返回文本可能为空（服务认为没有语音），
/// 这属于正常。判断依据是 HTTP 是否成功。
#[tauri::command(rename_all = "camelCase")]
pub async fn test_asr_connection(app: AppHandle, provider: AsrProvider) -> AsrConnectionTest {
    let client = {
        let state = app.state::<AppState>();
        Arc::clone(&state.asr)
    };
    client.test_connection(&provider).await
}

/* ==========================================================================
 * 音频设备
 * ========================================================================== */

#[tauri::command(rename_all = "camelCase")]
pub fn list_audio_sources() -> Vec<AudioSourceInfo> {
    capture::list_sources()
}

/* ==========================================================================
 * 会话
 * ========================================================================== */

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StartSessionRequest {
    pub title: Option<String>,
    pub enable_mic: bool,
    pub enable_loopback: bool,
    pub mic_device_id: Option<String>,
    pub loopback_device_id: Option<String>,
    /// 覆盖设置里的模型
    pub model_id: Option<String>,
    pub save_audio: Option<bool>,
}

impl Default for StartSessionRequest {
    fn default() -> Self {
        Self {
            title: None,
            enable_mic: true,
            enable_loopback: false,
            mic_device_id: None,
            loopback_device_id: None,
            model_id: None,
            save_audio: None,
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn start_session(
    app: AppHandle,
    state: State<'_, AppState>,
    req: StartSessionRequest,
) -> AppResult<SessionInfo> {
    state.ensure_idle()?;
    let settings = state.settings_snapshot();

    // ---- 解析声源 ----
    let mic_id = if req.enable_mic {
        req.mic_device_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| settings.audio.mic_device_id.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| capture::default_source_id(SourceKind::Microphone))
            .ok_or_else(|| AppError::audio("没有找到可用的麦克风，请在设置里检查音频设备"))?
    } else {
        String::new()
    };
    let loopback_id = if req.enable_loopback {
        req.loopback_device_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| settings.audio.loopback_device_id.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| capture::default_source_id(SourceKind::Loopback))
            .ok_or_else(|| {
                AppError::audio(
                    "没有找到可用的系统内录设备。Windows 一般开箱可用；\
                     Linux 需要 PulseAudio/PipeWire 的 monitor 源；\
                     macOS 需要先安装 BlackHole 等虚拟声卡。",
                )
            })?
    } else {
        String::new()
    };

    // ---- 识别服务 ----
    let asr = settings.asr.clone();
    let provider = asr
        .active()
        .cloned()
        .ok_or_else(|| AppError::asr("尚未配置语音识别服务，请打开「设置 → 语音识别」"))?;
    if !provider.is_usable() {
        return Err(AppError::asr(
            "语音识别服务配置不完整（需要 Base URL 与模型名），请打开「设置 → 语音识别」",
        ));
    }
    let source_labels = source_labels(&mic_id, &loopback_id, req.enable_mic, req.enable_loopback);
    let config = SessionConfig {
        model_id: format!("{} · {}", provider.name, provider.model),
        enable_mic: req.enable_mic,
        enable_loopback: req.enable_loopback,
        mic_label: source_labels.0,
        loopback_label: source_labels.1,
    };

    let title = req
        .title
        .clone()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| default_title(now_ms(), "会议"));

    let mut session = Session::new(title, config);
    session.summary.error = None;

    // ---- 录音文件 ----
    let save_audio = req.save_audio.unwrap_or(settings.audio.save_audio);
    let record_path = if save_audio {
        Some(state.paths.recordings.join(format!("{}.wav", session.id)))
    } else {
        None
    };
    session.audio_path = record_path.as_ref().map(|p| p.display().to_string());

    let session = Arc::new(parking_lot::Mutex::new(session));
    state.sessions.save(&session.lock().clone())?;

    // ---- 采集 ----
    let (frame_tx, frame_rx) = bounded(64);
    let (err_tx, err_rx) = bounded(8);
    let capture_cfg = CaptureConfig {
        mic_device_id: if req.enable_mic { Some(mic_id.clone()) } else { None },
        loopback_device_id: if req.enable_loopback { Some(loopback_id.clone()) } else { None },
        mic_gain: settings.audio.mic_gain,
        loopback_gain: settings.audio.loopback_gain,
    };
    let handle = capture::start(capture_cfg, frame_tx, err_tx)?;
    let sources: Vec<(String, SourceKind)> =
        handle.sources.iter().map(|s| (s.id.clone(), s.kind)).collect();

    // ---- 流水线 ----
    let emitter = state.emitter();
    let ai_client = if settings.ai.ready() {
        Some(Arc::clone(&state.ai))
    } else {
        None
    };
    let pipeline = pipeline::spawn(
        PipelineOptions {
            provider,
            asr,
            vad: settings.vad.clone(),
            single_source: None,
            record_path,
            ai: settings.ai.clone(),
            // Tauri 的全局运行时可以在同步命令里安全调用（内部会 enter 运行时上下文）
            spawn_task: Some(Box::new(|fut| {
                tauri::async_runtime::spawn(fut);
            })),
        },
        frame_rx,
        Some(handle),
        sources,
        Arc::clone(&session),
        emitter.clone(),
        ai_client,
    )?;

    let info = session.lock().info(now_ms());
    *state.active.lock() = Some(pipeline);

    // 采集线程的错误：**立刻停止录音**。
    //
    // 声源线程一旦退出（设备被拔掉、被别的程序独占、PulseAudio 掉线…），
    // 后面再录下去只会得到一片寂静，而用户看到的是一份「还在录、但没有内容」
    // 的会议 —— 比直接报错糟糕得多。这里报错并收尾，把已经录到的部分存好。
    {
        let emitter = emitter.clone();
        let session_id = info.id.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            // 只有第一条错误才触发停止；后面的重复报错直接丢弃
            let first = loop {
                match err_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(msg) => break Some(msg),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break None, // 采集线程全部退出且没有报错（正常停止）
                }
            };
            let Some(msg) = first else { return };

            crate::events::emit(
                &emitter,
                event::ERROR,
                &crate::events::AppErrorEvent {
                    scope: "audio".into(),
                    message: format!("{msg}；已自动停止录音，避免继续录到空白内容"),
                },
            );

            // 这里也可能被手动停止抢先，失败属正常
            match stop_active_session(app).await {
                Ok(detail) => {
                    crate::events::emit(
                        &emitter,
                        event::STATE,
                        &crate::events::AsrStateEvent {
                            session_id: detail.id.clone(),
                            state: crate::events::AsrStateKind::Idle,
                            message: Some(format!("音频采集中断，已停止录音并保存（{} 条转写）", detail.segments.len())),
                        },
                    );
                }
                Err(e) => tracing::warn!("音频故障后自动停止失败：{e}"),
            }
            let _ = &session_id;
        });
    }

    Ok(info)
}

fn source_labels(
    mic_id: &str,
    loopback_id: &str,
    enable_mic: bool,
    enable_loopback: bool,
) -> (Option<String>, Option<String>) {
    let all = capture::list_sources();
    let find = |id: &str| all.iter().find(|s| s.id == id).map(|s| s.label.clone());
    (
        if enable_mic { find(mic_id) } else { None },
        if enable_loopback { find(loopback_id) } else { None },
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn pause_session(state: State<'_, AppState>) -> AppResult<SessionInfo> {
    let guard = state.active.lock();
    let pipeline = guard
        .as_ref()
        .ok_or_else(|| AppError::session("当前没有正在进行的录音"))?;
    pipeline.paused.store(true, Ordering::Relaxed);
    let info = {
        let mut s = pipeline.session.lock();
        if let Some(t) = s.resumed_at.take() {
            s.duration_ms += (now_ms() - t).max(0);
        }
        s.status = SessionStatus::Paused;
        s.info(now_ms())
    };
    Ok(info)
}

#[tauri::command(rename_all = "camelCase")]
pub fn resume_session(state: State<'_, AppState>) -> AppResult<SessionInfo> {
    let guard = state.active.lock();
    let pipeline = guard
        .as_ref()
        .ok_or_else(|| AppError::session("当前没有正在进行的录音"))?;
    pipeline.paused.store(false, Ordering::Relaxed);
    let info = {
        let mut s = pipeline.session.lock();
        s.status = SessionStatus::Recording;
        s.resumed_at = Some(now_ms());
        s.info(now_ms())
    };
    Ok(info)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn stop_session(app: AppHandle) -> AppResult<SessionDetail> {
    stop_active_session(app).await
}

/// 停止当前录音并落盘（手动停止与「音频故障自动停止」共用）
async fn stop_active_session(app: AppHandle) -> AppResult<SessionDetail> {
    // 先把 pipeline 从状态里取出来（不跨 await 持锁）
    let pipeline = {
        let state = app.state::<AppState>();
        // 先把守卫绑定到局部变量，否则临时守卫会活过 state 的借用范围
        let taken = state.active.lock().take();
        taken.ok_or_else(|| AppError::session("当前没有正在进行的录音"))?
    };

    // 停止涉及 join 线程（可能要等最后一次识别），放到阻塞线程池
    let session = tauri::async_runtime::spawn_blocking(move || pipeline.stop())
        .await
        .map_err(|e| AppError::session(format!("停止录音失败：{e}")))?;

    let snapshot = session.lock().clone();
    let state = app.state::<AppState>();
    state.sessions.save(&snapshot)?;

    // 停止后自动生成完整纪要（可选，失败不影响会话保存）
    let settings = state.settings_snapshot();
    if settings.ai.final_report_on_stop && settings.ai.ready() && !snapshot.segments.is_empty() {
        if let Err(e) = generate_report(app.clone(), snapshot.id.clone()).await {
            tracing::warn!("自动生成完整纪要失败：{e}");
        }
    }

    let detail = session.lock().detail(now_ms());
    Ok(detail)
}

#[tauri::command(rename_all = "camelCase")]
pub fn active_session(state: State<'_, AppState>) -> Option<SessionInfo> {
    state
        .active
        .lock()
        .as_ref()
        .map(|p| p.session.lock().info(now_ms()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_session(app: AppHandle, id: String) -> AppResult<SessionDetail> {
    let state = app.state::<AppState>();
    // 正在录音的会话优先从内存取（磁盘上是旧快照）
    if let Some(pipeline) = state.active.lock().as_ref() {
        let s = pipeline.session.lock();
        if s.id == id {
            return Ok(s.detail(now_ms()));
        }
    }
    Ok(state.sessions.load(&id)?.detail(now_ms()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_sessions(state: State<'_, AppState>) -> AppResult<Vec<SessionListItem>> {
    let mut items = state.sessions.list()?;
    // 把正在进行中的会话状态修正为最新
    if let Some(pipeline) = state.active.lock().as_ref() {
        let live = pipeline.session.lock().list_item();
        if let Some(slot) = items.iter_mut().find(|i| i.id == live.id) {
            *slot = live;
        } else {
            items.insert(0, live);
        }
    }
    Ok(items)
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_session(state: State<'_, AppState>, id: String) -> AppResult<()> {
    if let Some(pipeline) = state.active.lock().as_ref() {
        if pipeline.session.lock().id == id {
            return Err(AppError::session("该会话正在录音中，请先停止"));
        }
    }
    // 一并删掉录音文件
    if let Ok(session) = state.sessions.load(&id) {
        if let Some(path) = session.audio_path {
            let p = PathBuf::from(path);
            if p.is_file() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    state.sessions.delete(&id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn rename_session(app: AppHandle, id: String, title: String) -> AppResult<SessionInfo> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::session("标题不能为空"));
    }
    let state = app.state::<AppState>();

    if let Some(pipeline) = state.active.lock().as_ref() {
        let mut s = pipeline.session.lock();
        if s.id == id {
            s.title = title;
            let info = s.info(now_ms());
            let snapshot = s.clone();
            drop(s);
            let _ = state.sessions.save(&snapshot);
            return Ok(info);
        }
    }

    let mut session = state.sessions.load(&id)?;
    session.title = title;
    state.sessions.save(&session)?;
    Ok(session.info(now_ms()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn export_session(app: AppHandle, id: String, format: String, path: String) -> AppResult<String> {
    let state = app.state::<AppState>();
    let format = ExportFormat::parse(&format)?;

    let session = if let Some(pipeline) = state.active.lock().as_ref() {
        let s = pipeline.session.lock();
        if s.id == id {
            s.clone()
        } else {
            state.sessions.load(&id)?
        }
    } else {
        state.sessions.load(&id)?
    };

    let content = render_export(&session, format)?;
    let target = PathBuf::from(&path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&target, content)?;
    tracing::info!("已导出：{}", target.display());
    Ok(target.display().to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn transcribe_file(app: AppHandle, path: String) -> AppResult<SessionInfo> {
    let settings = {
        let state = app.state::<AppState>();
        state.ensure_idle()?;
        state.settings_snapshot()
    };

    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(AppError::audio(format!("文件不存在：{path}")));
    }

    // 解码很吃 CPU，放阻塞线程池
    let decode_path = file.clone();
    let samples = tauri::async_runtime::spawn_blocking(move || {
        crate::audio::decode::decode_to_16k_mono(&decode_path)
    })
    .await
    .map_err(|e| AppError::audio(format!("解码任务失败：{e}")))??;

    if samples.is_empty() {
        return Err(AppError::audio("音频文件里没有可识别的内容"));
    }
    let audio_ms = samples.len() as i64 * 1000 / crate::audio::vad::RATE as i64;

    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("导入的音频");
    let title = format!("导入：{}", crate::util::truncate_chars(name, 60));

    let asr = settings.asr.clone();
    let provider = asr
        .active()
        .cloned()
        .ok_or_else(|| AppError::asr("尚未配置语音识别服务，请打开「设置 → 语音识别」"))?;
    if !provider.is_usable() {
        return Err(AppError::asr("语音识别服务配置不完整（需要 Base URL 与模型名）"));
    }

    let config = SessionConfig {
        model_id: format!("{} · {}", provider.name, provider.model),
        enable_mic: false,
        enable_loopback: false,
        mic_label: None,
        loopback_label: None,
    };
    let session = Session::new(title, config);

    let session = Arc::new(parking_lot::Mutex::new(session));
    let state = app.state::<AppState>();
    state.sessions.save(&session.lock().clone())?;

    let (frame_tx, frame_rx) = bounded(64);
    let (err_tx, _err_rx) = bounded::<String>(8);
    drop(err_tx);

    let emitter = state.emitter();
    let ai_client = if settings.ai.ready() {
        Some(Arc::clone(&state.ai))
    } else {
        None
    };
    let pipeline = pipeline::spawn(
        PipelineOptions {
            provider,
            asr,
            vad: settings.vad.clone(),
            single_source: None,
            record_path: None,
            ai: settings.ai.clone(),
            spawn_task: Some(Box::new(|fut| {
                tauri::async_runtime::spawn(fut);
            })),
        },
        frame_rx,
        None,
        vec![("file:import".to_string(), SourceKind::Microphone)],
        Arc::clone(&session),
        emitter,
        ai_client,
    )?;

    // 用「比实时快」的速度喂入：靠下游反压自然限速，既快又能看到实时滚动效果
    let feed_stop = Arc::clone(&pipeline.stop);
    pipeline::feed_audio(
        &samples,
        frame_tx,
        "file:import",
        SourceKind::Microphone,
        (crate::audio::vad::RATE as usize) / 10,
        Some(Duration::from_millis(8)),
        feed_stop,
    );
    tracing::info!("导入音频：{path}（{audio_ms} ms）");

    let info = session.lock().info(now_ms());
    *state.active.lock() = Some(pipeline);
    Ok(info)
}

/* ==========================================================================
 * 应用
 * ========================================================================== */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub data_dir: String,
    pub sessions_dir: String,
    pub recordings_dir: String,
    /// 当前生效的识别服务描述（用于「关于」页展示）
    pub asr_service: String,
    pub vad_engine: String,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        name: "MeetingHear".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_dir: state.paths.data.display().to_string(),
        sessions_dir: state.paths.sessions.display().to_string(),
        recordings_dir: state.paths.recordings.display().to_string(),
        asr_service: {
            let settings = state.settings_snapshot();
            match settings.asr.active() {
                Some(p) if settings.asr.ready() => {
                    format!("{} · {} · {}", p.name, p.model, p.endpoint())
                }
                _ => "未配置语音识别服务".into(),
            }
        },
        vad_engine: "energy（自适应能量 VAD）".into(),
    }
}

/// 用系统默认浏览器打开链接（首次向导里要跳到 BlackHole / 各家控制台等页面）
#[tauri::command(rename_all = "camelCase")]
pub fn open_url(app: AppHandle, url: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let url = url.trim().to_string();
    // 只放行 http/https：避免把任意字符串交给系统 opener 执行
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(AppError::Other(
            "只允许打开 http/https 链接".into(),
        ));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Other(format!("打开链接失败：{e}")))
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    open_path_internal(&app, &PathBuf::from(path))
}

fn open_path_internal(app: &AppHandle, path: &std::path::Path) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.to_path_buf())
    };
    app.opener()
        .open_path(target.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Other(format!("打开目录失败：{e}")))
}

/// 取当前会话（内存中的优先），供 AI 相关命令使用
fn active_session_or_load(
    state: &AppState,
    id: &str,
) -> AppResult<Arc<parking_lot::Mutex<Session>>> {
    if let Some(pipeline) = state.active.lock().as_ref() {
        if pipeline.session.lock().id == id {
            return Ok(Arc::clone(&pipeline.session));
        }
    }
    let session = state.sessions.load(id)?;
    Ok(Arc::new(parking_lot::Mutex::new(session)))
}
