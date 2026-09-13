//! MeetingHear —— 实时会议转写与 AI 纪要。
//!
//! 模块划分：
//! * [`audio`]  采集（麦克风 / 系统内录）、重采样、VAD、录音落盘、音频解码
//! * [`asr`]    本地 whisper.cpp 推理、流式文本合并、模型下载管理
//! * [`ai`]     OpenAI 兼容客户端、提示词、滚动纪要状态机
//! * [`pipeline`] 断句 → 识别 → 纪要 的实时流水线
//! * [`session`] 会话模型、持久化、导出
//! * [`commands`] 前端可调用的全部命令

pub mod ai;
pub mod asr;
pub mod audio;
pub mod commands;
pub mod diagnostics;
pub mod error;
pub mod events;
pub mod pipeline;
pub mod session;
pub mod settings;
pub mod state;
pub mod util;

use std::path::PathBuf;

use tauri::Manager;

use crate::settings::{AppPaths, Settings};
use crate::state::AppState;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,whisper_rs=warn")),
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let default_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法定位应用数据目录：{e}"))?;

            // 先按默认目录读配置（配置里可能指定了自定义数据目录）
            let mut settings = Settings::load(&default_dir);
            let data_dir = settings
                .general
                .data_dir
                .clone()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| default_dir.clone());

            let paths = AppPaths::new(data_dir);
            paths
                .ensure()
                .map_err(|e| format!("无法创建数据目录：{e}"))?;
            settings.sanitize();

            tracing::info!("数据目录：{}", paths.data.display());
            match settings.asr.active() {
                Some(p) if settings.asr.ready() => {
                    tracing::info!("语音识别服务：{} · {} · {}", p.name, p.model, p.endpoint())
                }
                _ => tracing::warn!("尚未配置语音识别服务，请在设置里选择服务商"),
            }

            let state = AppState::new(app.handle().clone(), paths, settings)
                .map_err(|e| format!("初始化失败：{e}"))?;
            app.manage(state);
            Ok(())
        })
        // 关窗口时如果还在录音：先停掉流水线并把会话落盘，
        // 否则用户直接叉掉窗口会丢掉整场会议的转写与纪要。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    let pipeline = state.active.lock().take();
                    if let Some(pipeline) = pipeline {
                        tracing::info!("窗口关闭，正在停止录音并保存会话…");
                        let session = pipeline.stop();
                        let snapshot = session.lock().clone();
                        match state.sessions.save(&snapshot) {
                            Ok(()) => tracing::info!(
                                "会话已保存：{}（{} 段转写）",
                                snapshot.title,
                                snapshot.segments.len()
                            ),
                            Err(e) => tracing::error!("保存会话失败：{e}"),
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // 设置
            commands::get_settings,
            commands::save_settings,
            commands::reset_settings,
            // AI
            commands::list_ai_presets,
            commands::test_ai_connection,
            commands::summarize_now,
            commands::generate_report,
            // 语音识别服务
            commands::list_asr_presets,
            commands::test_asr_connection,
            // 设备
            commands::list_audio_sources,
            // 会话
            commands::start_session,
            commands::pause_session,
            commands::resume_session,
            commands::stop_session,
            commands::active_session,
            commands::get_session,
            commands::list_sessions,
            commands::delete_session,
            commands::rename_session,
            commands::export_session,
            commands::transcribe_file,
            // 应用
            commands::get_app_info,
            commands::open_path,
        ])
        .run(tauri::generate_context!())
        .expect("MeetingHear 启动失败");
}
