//! 应用全局状态。

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::AppHandle;

use crate::ai::client::AiClient;
use crate::asr::client::AsrClient;
use crate::error::{AppError, AppResult};
use crate::events::TauriEmitter;
use crate::pipeline::Pipeline;
use crate::session::SessionStore;
use crate::settings::{AppPaths, Settings};

pub struct AppState {
    pub app: AppHandle,
    pub paths: AppPaths,
    pub settings: Mutex<Settings>,
    pub sessions: SessionStore,
    pub ai: Arc<AiClient>,
    pub asr: Arc<AsrClient>,
    /// 当前进行中的录音（同一时刻只允许一个）
    pub active: Mutex<Option<Pipeline>>,
}

impl AppState {
    pub fn new(app: AppHandle, paths: AppPaths, settings: Settings) -> AppResult<Self> {
        paths.ensure()?;
        Ok(Self {
            app,
            sessions: SessionStore::new(paths.sessions.clone()),
            ai: Arc::new(AiClient::new()?),
            asr: Arc::new(AsrClient::new()?),
            settings: Mutex::new(settings),
            active: Mutex::new(None),
            paths,
        })
    }

    pub fn emitter(&self) -> Arc<TauriEmitter> {
        Arc::new(TauriEmitter::new(self.app.clone()))
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings.lock().clone()
    }

    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        settings.save(&self.paths.data)
    }

    /// 确保当前没有进行中的录音
    pub fn ensure_idle(&self) -> AppResult<()> {
        if self.active.lock().is_some() {
            return Err(AppError::session("已有正在进行的录音，请先停止"));
        }
        Ok(())
    }
}

/// 读取配置（不存在的字段用默认值）
pub fn load_settings(paths: &AppPaths) -> Settings {
    let mut s = Settings::load(&paths.data);
    s.sanitize();
    s
}
