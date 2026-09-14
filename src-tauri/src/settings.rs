//! 应用配置模型。
//!
//! 与前端 `src/lib/contract.ts` 一一对应，全部 `camelCase` 序列化。
//! 字段一律带 `#[serde(default)]`，保证旧配置文件在新增字段后仍可加载。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub const SETTINGS_VERSION: u32 = 1;

/* ==========================================================================
 * 识别（ASR）
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AsrProvider {
    pub id: String,
    pub name: String,
    /// 服务根地址，例如 https://api.openai.com/v1 或 http://localhost:8080
    pub base_url: String,
    /// 转写接口路径，例如 /audio/transcriptions（OpenAI 兼容）或 /inference（whisper.cpp server）
    pub transcription_path: String,
    pub api_key: String,
    /// 模型名，例如 whisper-1 / whisper-large-v3-turbo / large-v3
    pub model: String,
    /// 请求格式：json（最兼容）或 verbose_json（能拿到分段）
    pub response_format: String,
    pub timeout_secs: u32,
    /// 额外请求头，例如自建网关需要的鉴权字段
    pub extra_headers: Vec<(String, String)>,
}

impl Default for AsrProvider {
    fn default() -> Self {
        Self {
            id: "custom".into(),
            name: "自定义".into(),
            base_url: String::new(),
            transcription_path: "/audio/transcriptions".into(),
            api_key: String::new(),
            model: String::new(),
            response_format: "json".into(),
            timeout_secs: 120,
            extra_headers: Vec::new(),
        }
    }
}

impl AsrProvider {
    /// 去掉尾部斜杠、去掉用户可能多粘贴的完整路径，得到服务根地址
    pub fn root_url(&self) -> String {
        let mut u = self.base_url.trim().trim_end_matches('/').to_string();
        for suffix in ["/audio/transcriptions", "/audio/translations", "/inference"] {
            if let Some(stripped) = u.strip_suffix(suffix) {
                u = stripped.trim_end_matches('/').to_string();
                break;
            }
        }
        u
    }

    /// 完整的转写接口地址
    pub fn endpoint(&self) -> String {
        let root = self.root_url();
        let mut path = self.transcription_path.trim().to_string();
        if path.is_empty() {
            path = "/audio/transcriptions".into();
        }
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        format!("{root}{path}")
    }

    /// 从 base_url 里取出主机名（用于判断配置是否真的可用）
    pub fn host(&self) -> String {
        let root = self.root_url();
        match root.split_once("://") {
            // 必须写成 scheme://host 的形式；只有 "https://" 这种占位符不算
            Some((_, rest)) => rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
            None => String::new(),
        }
    }

    /// 这个服务是否真的会用到「模型名」。
    ///
    /// whisper.cpp server 的 `/inference` 只认它启动时 `-m` 加载的那个模型，
    /// 请求里的 model 字段它**完全忽略**。以前把它做成必填，用户就被卡在
    /// 「模型名填什么」这种问题上过不去 —— 一个没有意义的值不该成为门槛。
    pub fn model_is_required(&self) -> bool {
        !self.transcription_path.trim_end_matches('/').ends_with("/inference")
    }

    /// 是否具备调用条件
    pub fn is_usable(&self) -> bool {
        !self.host().is_empty() && (!self.model_is_required() || !self.model.trim().is_empty())
    }

    pub fn sanitized(mut self) -> Self {
        self.timeout_secs = self.timeout_secs.clamp(5, 600);
        if self.name.trim().is_empty() {
            self.name = "未命名服务".into();
        }
        if !matches!(self.response_format.as_str(), "json" | "verbose_json" | "text") {
            self.response_format = "json".into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AsrSettings {
    pub enabled: bool,
    pub providers: Vec<AsrProvider>,
    pub active_provider_id: String,
    /// 把上一句已确认文本作为 prompt 传给服务，提升人名/术语一致性
    /// （仅部分服务支持，OpenAI 兼容接口支持 prompt 字段）
    pub context_prompt: bool,
    pub temperature: f32,
    /// **边说边出字**：对当前这句话做增量识别。
    /// 每次增量都是一次 API 请求，云端服务会显著增加调用次数与费用，
    /// 自建/本地服务建议开启。
    pub live_preview: bool,
    /// 单次请求最多上传多少秒音频（超长句会被强制断句）
    pub max_chunk_secs: u32,
}

impl Default for AsrSettings {
    fn default() -> Self {
        let providers = vec![crate::asr::provider::provider_from_preset("groq")];
        Self {
            enabled: true,
            active_provider_id: providers[0].id.clone(),
            providers,
            context_prompt: true,
            temperature: 0.0,
            live_preview: false,
            max_chunk_secs: 25,
        }
    }
}

impl AsrSettings {
    pub fn active(&self) -> Option<&AsrProvider> {
        self.providers
            .iter()
            .find(|p| p.id == self.active_provider_id)
            .or_else(|| self.providers.first())
    }

    /// 是否具备开始转写的条件
    pub fn ready(&self) -> bool {
        self.enabled && self.active().map(|p| p.is_usable()).unwrap_or(false)
    }

    pub fn sanitize(&mut self) {
        self.temperature = self.temperature.clamp(0.0, 1.0);
        self.max_chunk_secs = self.max_chunk_secs.clamp(5, 120);
        if self.providers.is_empty() {
            self.providers = vec![crate::asr::provider::provider_from_preset("groq")];
        }
        if !self.providers.iter().any(|p| p.id == self.active_provider_id) {
            self.active_provider_id = self.providers[0].id.clone();
        }
        for (i, p) in self.providers.iter_mut().enumerate() {
            if p.id.trim().is_empty() {
                p.id = format!("asr{i}");
            }
            *p = p.clone().sanitized();
        }
    }
}

/* ==========================================================================
 * 语音活动检测（VAD）
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VadSettings {
    /// 高于自适应噪声底多少 dB 判定为语音
    pub energy_threshold_db: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub max_utterance_ms: u32,
    pub speech_pad_ms: u32,
}

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            energy_threshold_db: 9.0,
            min_speech_ms: 250,
            min_silence_ms: 600,
            max_utterance_ms: 25_000,
            speech_pad_ms: 200,
        }
    }
}

impl VadSettings {
    /// 参数纠偏，避免用户填出不可用的组合
    pub fn sanitize(&mut self) {
        self.energy_threshold_db = self.energy_threshold_db.clamp(2.0, 30.0);
        self.min_speech_ms = self.min_speech_ms.clamp(50, 3_000);
        self.min_silence_ms = self.min_silence_ms.clamp(150, 5_000);
        self.max_utterance_ms = self.max_utterance_ms.clamp(3_000, 60_000);
        self.speech_pad_ms = self.speech_pad_ms.clamp(0, 1_000);
        if self.max_utterance_ms <= self.min_silence_ms {
            self.max_utterance_ms = self.min_silence_ms + 2_000;
        }
    }
}

/* ==========================================================================
 * AI 接口
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub json_mode: bool,
    pub timeout_secs: u32,
    /// 额外请求头，例如自建网关需要的鉴权字段
    pub extra_headers: Vec<(String, String)>,
}

impl Default for AiProvider {
    fn default() -> Self {
        Self {
            id: "custom".into(),
            name: "自定义".into(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            temperature: 0.2,
            max_tokens: 1_200,
            json_mode: false,
            timeout_secs: 60,
            extra_headers: Vec::new(),
        }
    }
}

impl AiProvider {
    /// 去掉尾部斜杠、去掉用户可能多粘贴的 `/chat/completions`，
    /// 得到「根地址」，便于统一拼 `/chat/completions` 与 `/models`。
    pub fn root_url(&self) -> String {
        let mut u = self.base_url.trim().trim_end_matches('/').to_string();
        for suffix in ["/chat/completions", "/completions", "/models"] {
            if let Some(stripped) = u.strip_suffix(suffix) {
                u = stripped.trim_end_matches('/').to_string();
                break;
            }
        }
        u
    }

    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.root_url())
    }

    pub fn models_url(&self) -> String {
        format!("{}/models", self.root_url())
    }

    /// 从 base_url 里取出主机名（用于判断配置是否真的可用）
    pub fn host(&self) -> String {
        let root = self.root_url();
        match root.split_once("://") {
            // 必须写成 scheme://host 的形式；只有 "https://" 这种占位符不算
            Some((_, rest)) => rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
            None => String::new(),
        }
    }

    pub fn is_usable(&self) -> bool {
        !self.host().is_empty() && !self.model.trim().is_empty()
    }

    pub fn sanitized(mut self) -> Self {
        self.temperature = self.temperature.clamp(0.0, 2.0);
        self.max_tokens = self.max_tokens.clamp(64, 32_000);
        self.timeout_secs = self.timeout_secs.clamp(5, 600);
        if self.name.trim().is_empty() {
            self.name = "未命名服务".into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub enabled: bool,
    pub providers: Vec<AiProvider>,
    pub active_provider_id: String,
    pub auto_summary: bool,
    pub interval_secs: u32,
    pub min_new_chars: u32,
    pub live_window_secs: u32,
    pub max_context_chars: u32,
    pub final_report_on_stop: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        let providers = vec![crate::ai::presets::provider_from_preset("deepseek")];
        Self {
            enabled: true,
            active_provider_id: providers[0].id.clone(),
            providers,
            auto_summary: true,
            interval_secs: 20,
            min_new_chars: 60,
            live_window_secs: 90,
            max_context_chars: 4_000,
            final_report_on_stop: false,
        }
    }
}

impl AiSettings {
    pub fn active(&self) -> Option<&AiProvider> {
        self.providers
            .iter()
            .find(|p| p.id == self.active_provider_id)
            .or_else(|| self.providers.first())
    }

    /// 是否具备自动总结的条件
    pub fn ready(&self) -> bool {
        self.enabled && self.active().map(|p| p.is_usable()).unwrap_or(false)
    }

    pub fn sanitize(&mut self) {
        self.interval_secs = self.interval_secs.clamp(5, 600);
        self.min_new_chars = self.min_new_chars.clamp(0, 5_000);
        self.live_window_secs = self.live_window_secs.clamp(20, 600);
        self.max_context_chars = self.max_context_chars.clamp(500, 60_000);
        if self.providers.is_empty() {
            self.providers = vec![crate::ai::presets::provider_from_preset("deepseek")];
        }
        if !self.providers.iter().any(|p| p.id == self.active_provider_id) {
            self.active_provider_id = self.providers[0].id.clone();
        }
        for p in &mut self.providers {
            // 去重 id
            if p.id.trim().is_empty() {
                p.id = format!("p{}", uuid::Uuid::new_v4().simple());
            }
        }
    }
}

/* ==========================================================================
 * 音频
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioSettings {
    pub enable_mic: bool,
    pub mic_device_id: Option<String>,
    pub enable_loopback: bool,
    pub loopback_device_id: Option<String>,
    pub mic_gain: f32,
    pub loopback_gain: f32,
    pub save_audio: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            enable_mic: true,
            mic_device_id: None,
            enable_loopback: true,
            loopback_device_id: None,
            mic_gain: 1.0,
            loopback_gain: 1.0,
            save_audio: true,
        }
    }
}

impl AudioSettings {
    pub fn sanitize(&mut self) {
        self.mic_gain = self.mic_gain.clamp(0.0, 4.0);
        self.loopback_gain = self.loopback_gain.clamp(0.0, 4.0);
    }
}

/* ==========================================================================
 * 通用
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneralSettings {
    pub theme: String,
    pub auto_scroll: bool,
    pub font_scale: f32,
    pub data_dir: Option<String>,
    pub persist_api_key: bool,
    /// 首次配置向导是否已完成。为 false 时前端启动后会引导用户配置
    /// 识别服务与 AI 接口（这两件事不配好，应用其实什么都做不了）。
    pub onboarding_completed: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            auto_scroll: true,
            font_scale: 1.0,
            data_dir: None,
            persist_api_key: true,
            onboarding_completed: false,
        }
    }
}

impl GeneralSettings {
    pub fn sanitize(&mut self) {
        self.font_scale = self.font_scale.clamp(0.8, 1.6);
        if !matches!(self.theme.as_str(), "dark" | "light" | "system") {
            self.theme = "dark".into();
        }
    }
}

/* ==========================================================================
 * Settings 根
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub version: u32,
    pub asr: AsrSettings,
    pub vad: VadSettings,
    pub ai: AiSettings,
    pub audio: AudioSettings,
    pub general: GeneralSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            asr: AsrSettings::default(),
            vad: VadSettings::default(),
            ai: AiSettings::default(),
            audio: AudioSettings::default(),
            general: GeneralSettings::default(),
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        self.version = SETTINGS_VERSION;
        self.vad.sanitize();
        self.ai.sanitize();
        self.audio.sanitize();
        self.general.sanitize();
        self.asr.sanitize();
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    s.sanitize();
                    s
                }
                Err(err) => {
                    tracing::warn!("配置文件解析失败（{err}），已回退默认配置：{}", path.display());
                    let mut s = Settings::default();
                    s.sanitize();
                    s
                }
            },
            Err(_) => {
                let mut s = Settings::default();
                s.sanitize();
                s
            }
        }
    }

    pub fn save(&self, dir: &Path) -> AppResult<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("settings.json");
        let text = serde_json::to_string_pretty(self)?;
        // 先写临时文件再原子改名，避免写一半崩溃导致配置损坏
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        restrict_permissions(&path);
        Ok(())
    }

    /// 落盘前按设置决定是否保留 api key
    pub fn for_persistence(&self) -> Self {
        let mut s = self.clone();
        if !s.general.persist_api_key {
            for p in &mut s.ai.providers {
                p.api_key.clear();
            }
            for p in &mut s.asr.providers {
                p.api_key.clear();
            }
        }
        s
    }
}

/// 配置文件含 api key，尽量收紧权限（非 Windows）
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// 应用目录集合
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data: PathBuf,
    pub sessions: PathBuf,
    pub recordings: PathBuf,
}

impl AppPaths {
    pub fn new(data: PathBuf) -> Self {
        Self {
            sessions: data.join("sessions"),
            recordings: data.join("recordings"),
            data,
        }
    }

    pub fn ensure(&self) -> AppResult<()> {
        for d in [&self.data, &self.sessions, &self.recordings] {
            std::fs::create_dir_all(d).map_err(|e| {
                AppError::config(format!("无法创建目录 {}：{e}", d.display()))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalization() {
        let mut p = AiProvider::default();
        p.base_url = "https://api.deepseek.com/v1/".into();
        assert_eq!(p.chat_url(), "https://api.deepseek.com/v1/chat/completions");

        p.base_url = "https://api.openai.com/v1/chat/completions".into();
        assert_eq!(p.chat_url(), "https://api.openai.com/v1/chat/completions");
        assert_eq!(p.models_url(), "https://api.openai.com/v1/models");

        p.base_url = "http://localhost:11434/v1".into();
        assert_eq!(p.chat_url(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn sanitize_keeps_values_in_range() {
        let mut v = VadSettings {
            energy_threshold_db: 99.0,
            min_silence_ms: 10,
            max_utterance_ms: 100,
            ..Default::default()
        };
        v.sanitize();
        assert!(v.energy_threshold_db <= 30.0);
        assert!(v.min_silence_ms >= 150);
        assert!(v.max_utterance_ms > v.min_silence_ms);
    }

    #[test]
    fn settings_roundtrip_with_missing_fields() {
        // 模拟旧版本配置文件：缺少 audio/general，且带着早已废弃的 language 字段。
        // 未知字段必须被安静忽略，不能让老用户升级后启动失败。
        let text = r#"{"version":1,"asr":{"language":"zh","translateToEnglish":true}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        assert!(!s.vad.energy_threshold_db.is_nan());
        assert!(s.audio.enable_mic);

        // 再写一遍时，废弃字段不再出现（语言归识别服务管，应用不存）
        let out = serde_json::to_string(&s).unwrap();
        assert!(!out.contains("language"));
        assert!(!out.contains("translateToEnglish"));
    }

    #[test]
    fn model_name_is_optional_for_whisper_cpp_server() {
        // whisper.cpp server 只认启动时 -m 加载的模型，请求里的 model 字段它不看。
        // 以前把它做成必填，用户会被「模型名填什么」卡住。
        let whisper_cpp = AsrProvider {
            base_url: "http://127.0.0.1:8090".into(),
            transcription_path: "/inference".into(),
            model: String::new(),
            ..Default::default()
        };
        assert!(!whisper_cpp.model_is_required());
        assert!(whisper_cpp.is_usable(), "本地 whisper.cpp server 不应因为没填模型名而不可用");

        // OpenAI 兼容接口必须给模型名，服务端会按名字挑模型
        let openai = AsrProvider {
            base_url: "https://api.openai.com/v1".into(),
            transcription_path: "/audio/transcriptions".into(),
            model: String::new(),
            ..Default::default()
        };
        assert!(openai.model_is_required());
        assert!(!openai.is_usable(), "云端服务没有模型名必须算配置不完整");

        // 但地址本身还是必须的
        let no_host = AsrProvider {
            base_url: String::new(),
            transcription_path: "/inference".into(),
            model: String::new(),
            ..Default::default()
        };
        assert!(!no_host.is_usable());
    }

    #[test]
    fn placeholder_url_is_not_usable() {
        // 预设「自定义」的初值是 https://，UI 会把它当成未配置；
        // 后端必须同样判定，否则会出现「测试连接说不能用、开始录音却放行」的分叉
        let mut p = AsrProvider {
            base_url: "https://".into(),
            model: "whisper-1".into(),
            ..Default::default()
        };
        assert!(p.host().is_empty());
        assert!(!p.is_usable(), "https:// 占位符不应被判定为可用");

        p.base_url = "http://".into();
        assert!(!p.is_usable());

        p.base_url = String::new();
        assert!(!p.is_usable());

        // 合法地址
        p.base_url = "https://api.groq.com/openai/v1".into();
        assert_eq!(p.host(), "api.groq.com");
        assert!(p.is_usable());

        // 本地服务
        p.base_url = "http://localhost:8090".into();
        assert_eq!(p.host(), "localhost:8090");
        assert!(p.is_usable());
    }

    #[test]
    fn asr_endpoint_is_built_from_base_and_path() {
        let mut p = AsrProvider {
            base_url: "https://api.openai.com/v1/".into(),
            transcription_path: "/audio/transcriptions".into(),
            ..Default::default()
        };
        assert_eq!(p.endpoint(), "https://api.openai.com/v1/audio/transcriptions");

        // 用户把完整地址粘进 base_url 也要能纠正
        p.base_url = "https://api.groq.com/openai/v1/audio/transcriptions".into();
        assert_eq!(p.endpoint(), "https://api.groq.com/openai/v1/audio/transcriptions");

        // 本地 whisper.cpp server
        p.base_url = "http://localhost:8080".into();
        p.transcription_path = "/inference".into();
        assert_eq!(p.endpoint(), "http://localhost:8080/inference");

        // path 不带前导斜杠也要能工作
        p.transcription_path = "inference".into();
        assert_eq!(p.endpoint(), "http://localhost:8080/inference");
    }

    #[test]
    fn api_key_is_stripped_when_not_persisted() {
        let mut s = Settings::default();
        s.ai.providers[0].api_key = "sk-secret".into();
        s.asr.providers[0].api_key = "asr-secret".into();
        s.general.persist_api_key = false;
        let stripped = s.for_persistence();
        assert!(stripped.ai.providers[0].api_key.is_empty());
        assert!(stripped.asr.providers[0].api_key.is_empty());
        s.general.persist_api_key = true;
        assert_eq!(s.for_persistence().ai.providers[0].api_key, "sk-secret");
        assert_eq!(s.for_persistence().asr.providers[0].api_key, "asr-secret");
    }

    #[test]
    fn app_never_sends_a_language_field() {
        // 语言是识别服务自己的事：应用只管上传音频。
        // （踩过的事故：whisper.cpp server 的 language 默认是 en，
        //   应用一旦自己决定语言，就会把中文按英文识别。）
        let req = crate::asr::client::TranscribeRequest {
            samples: vec![0.0; 160],
            prompt: None,
            temperature: 0.0,
        };
        let form = crate::asr::client::build_form(
            &AsrProvider {
                model: "whisper-1".into(),
                ..Default::default()
            },
            &req,
            vec![0u8; 4],
        )
        .expect("应能构建表单");
        let body = format!("{form:?}");
        assert!(
            !body.contains("language"),
            "应用不得自行决定识别语言，表单里不应有 language 字段"
        );
        assert!(body.contains("audio.wav"), "必须包含音频字段");
    }
}
