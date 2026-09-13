//! 语音识别 HTTP 客户端。
//!
//! 上传的音频固定是 **16kHz 单声道 16bit WAV**（所有服务都认这个格式），
//! 每句话一个请求，因此不需要服务端支持流式。
//!
//! 同时兼容两种服务形态：
//!   * OpenAI 兼容 `/audio/transcriptions`（OpenAI / Groq / 硅基流动 / faster-whisper-server）
//!   * whisper.cpp server `/inference`
//! 两者的请求都是 multipart、响应都是 `{"text": "..."}`，所以一套代码即可。

use std::time::{Duration, Instant};

use reqwest::multipart::{Form, Part};
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::settings::AsrProvider;
use crate::util::truncate_chars;

const USER_AGENT: &str = concat!("MeetingHear/", env!("CARGO_PKG_VERSION"));
/// 请求失败时的重试次数（限流/5xx/网络抖动）
const MAX_ATTEMPTS: u32 = 2;

#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    /// 16kHz 单声道 f32
    pub samples: Vec<f32>,
    pub language: Option<String>,
    /// 上一句已确认文本（作为 prompt，提升人名/术语一致性）
    pub prompt: Option<String>,
    pub temperature: f32,
}

#[derive(Debug, Clone)]
pub struct AsrOutcome {
    pub text: String,
    pub language: Option<String>,
    pub elapsed_ms: u64,
    pub audio_ms: i64,
}

impl AsrOutcome {
    /// 实时率：请求往返耗时 / 音频时长。小于 1 表示能跟上实时。
    pub fn rtf(&self) -> f32 {
        if self.audio_ms <= 0 {
            return 0.0;
        }
        self.elapsed_ms as f32 / self.audio_ms as f32
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrConnectionTest {
    pub ok: bool,
    pub latency_ms: u64,
    pub model: String,
    pub sample: String,
    pub error: Option<String>,
}

pub struct AsrClient {
    proxied: Client,
    direct: Client,
}

impl AsrClient {
    pub fn new() -> AppResult<Self> {
        let proxied = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| AppError::asr(format!("初始化 HTTP 客户端失败：{e}")))?;
        // 本地识别服务绝不能走代理（企业网络里很常见）
        let direct = Client::builder()
            .user_agent(USER_AGENT)
            .no_proxy()
            .build()
            .map_err(|e| AppError::asr(format!("初始化 HTTP 客户端失败：{e}")))?;
        Ok(Self { proxied, direct })
    }

    fn client_for(&self, url: &str) -> &Client {
        if is_local_url(url) {
            &self.direct
        } else {
            &self.proxied
        }
    }

    /// 把一句话的音频送去识别
    pub async fn transcribe(
        &self,
        provider: &AsrProvider,
        req: &TranscribeRequest,
    ) -> AppResult<AsrOutcome> {
        let provider = provider.clone().sanitized();
        if provider.host().is_empty() {
            return Err(AppError::asr(
                "尚未配置语音识别服务的 Base URL（应形如 https://api.groq.com/openai/v1）",
            ));
        }
        if provider.model.trim().is_empty() {
            return Err(AppError::asr("尚未配置语音识别模型名"));
        }
        if req.samples.is_empty() {
            return Err(AppError::asr("音频为空"));
        }

        let audio_ms = req.samples.len() as i64 * 1000 / crate::audio::vad::RATE as i64;
        let wav = crate::audio::wav::encode_wav_16k_mono(&req.samples)?;
        let started = Instant::now();
        let url = provider.endpoint();
        let timeout = Duration::from_secs(provider.timeout_secs as u64);

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .post_once(&url, &provider, req, wav.clone(), timeout)
                .await
            {
                Ok((text, language)) => {
                    return Ok(AsrOutcome {
                        text,
                        // 服务返回的语言优先（verbose_json 才有），否则用请求里指定的
                        language: language.or_else(|| req.language.clone()),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        audio_ms,
                    })
                }
                Err(err) => {
                    if attempt < MAX_ATTEMPTS && err.is_retryable() {
                        tracing::warn!("识别请求失败，1.5 秒后重试一次：{err}");
                        tokio::time::sleep(Duration::from_millis(1_500)).await;
                        continue;
                    }
                    return Err(err.to_app_error());
                }
            }
        }
    }

    async fn post_once(
        &self,
        url: &str,
        provider: &AsrProvider,
        req: &TranscribeRequest,
        wav: Vec<u8>,
        timeout: Duration,
    ) -> Result<(String, Option<String>), AsrError> {
        let mut form = Form::new()
            .part(
                "file",
                Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| AsrError::Transport(AppError::asr(e.to_string())))?,
            )
            .text("model", provider.model.trim().to_string())
            .text("response_format", provider.response_format.clone())
            .text("temperature", format!("{:.2}", req.temperature));

        if let Some(lang) = req.language.as_deref() {
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
                form = form.text("language", lang.to_string());
            }
        }
        if let Some(prompt) = req.prompt.as_deref() {
            let prompt = prompt.replace('\0', " ");
            if !prompt.trim().is_empty() {
                form = form.text("prompt", truncate_chars(prompt.trim(), 200));
            }
        }

        let mut request = self
            .client_for(url)
            .post(url)
            .timeout(timeout)
            .multipart(form);

        if !provider.api_key.trim().is_empty() {
            request = request.bearer_auth(provider.api_key.trim());
        }
        for (k, v) in &provider.extra_headers {
            if !k.trim().is_empty() {
                request = request.header(k.trim(), v.trim());
            }
        }

        let resp = request
            .send()
            .await
            .map_err(|e| AsrError::Transport(map_reqwest_error(e)))?;

        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = resp
            .text()
            .await
            .map_err(|e| AsrError::Transport(map_reqwest_error(e)))?;

        if !status.is_success() {
            return Err(AsrError::Status {
                code: status.as_u16(),
                message: extract_api_error(&body).unwrap_or_default(),
            });
        }

        extract_result(&body, &content_type).ok_or_else(|| {
            AsrError::Transport(AppError::asr(format!(
                "识别服务返回了无法解析的内容（Content-Type: {content_type}）：{}",
                truncate_chars(body.trim(), 200)
            )))
        })
    }

    /// 连通性测试：上传 0.6 秒静音，验证「地址 + 鉴权 + 模型名」是否都正确。
    ///
    /// 用静音而不是真实语音，是因为这个测试要能在任何环境跑通，
    /// 返回值可能为空文本（服务认为没有语音），这属于正常。
    pub async fn test_connection(&self, provider: &AsrProvider) -> AsrConnectionTest {
        let provider = provider.clone().sanitized();
        let model = provider.model.clone();
        let silence = vec![0.0f32; (crate::audio::vad::RATE as usize) * 6 / 10];

        let started = Instant::now();
        let req = TranscribeRequest {
            samples: silence,
            language: Some("zh".into()),
            prompt: None,
            temperature: 0.0,
        };
        match self.transcribe(&provider, &req).await {
            Ok(out) => AsrConnectionTest {
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                model,
                sample: out.text,
                error: None,
            },
            Err(e) => AsrConnectionTest {
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                model,
                sample: String::new(),
                error: Some(e.to_string()),
            },
        }
    }
}

/* ==========================================================================
 * 内部错误
 * ========================================================================== */

enum AsrError {
    Status { code: u16, message: String },
    Transport(AppError),
}

impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrError::Status { code, message } => {
                if message.trim().is_empty() {
                    write!(f, "HTTP {code}")
                } else {
                    write!(f, "HTTP {code}: {message}")
                }
            }
            AsrError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl AsrError {
    fn is_retryable(&self) -> bool {
        match self {
            AsrError::Status { code, .. } => *code == 429 || *code >= 500,
            AsrError::Transport(e) => !matches!(e, AppError::Asr(_)),
        }
    }

    fn to_app_error(self) -> AppError {
        match self {
            AsrError::Status { code, message } => {
                let hint = match code {
                    401 | 403 => "鉴权失败，请检查 API Key",
                    404 => "接口或模型不存在，请检查 Base URL 路径与模型名",
                    413 => "音频太大被拒绝，可在「识别」页调小单次上传时长",
                    415 => "服务不支持上传的音频格式（应用固定发送 16kHz 单声道 WAV）",
                    429 => "请求过于频繁或额度不足，请稍后重试",
                    400 => "请求被拒绝，常见原因是模型名不对或参数不被支持",
                    _ => "识别服务返回错误",
                };
                let detail = if message.trim().is_empty() {
                    String::new()
                } else {
                    format!("：{}", truncate_chars(message.trim(), 300))
                };
                AppError::asr(format!("{hint}（HTTP {code}）{detail}"))
            }
            AsrError::Transport(e) => e,
        }
    }
}

fn map_reqwest_error(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::asr("识别请求超时：可调大超时时间，或换更快的服务（如 Groq）")
    } else if e.is_connect() {
        AppError::asr(format!(
            "无法连接到识别服务：{e}（本地服务请确认已启动、端口正确）"
        ))
    } else {
        AppError::asr(format!("网络请求失败：{e}"))
    }
}

/// 从响应里取出「转写文本 + 可选语言」。
///
/// 兼容三种返回形态：
///   1. `{"text": "..."}`（OpenAI 的 json 格式 / whisper.cpp server）
///   2. `{"text": "...", "language": "chinese", "segments": [...]}`（verbose_json）
///   3. 纯文本（response_format=text，或网关直接返回字符串）
fn extract_result(body: &str, content_type: &str) -> Option<(String, Option<String>)> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let language = value
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty());
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            return Some((text.trim().to_string(), language));
        }
        // 少数网关把结果包一层
        for key in ["result", "data", "output"] {
            if let Some(inner) = value.get(key) {
                if let Some(text) = inner.get("text").and_then(|v| v.as_str()) {
                    return Some((text.trim().to_string(), language));
                }
                if let Some(text) = inner.as_str() {
                    return Some((text.trim().to_string(), language));
                }
            }
        }
        return None;
    }

    // 非 JSON：只有当它看起来确实不是 HTML 错误页时才当作文本结果
    if content_type.contains("html") || trimmed.starts_with('<') {
        return None;
    }
    Some((trimmed.to_string(), None))
}

/// 供测试使用的便捷包装
#[cfg(test)]
fn extract_text(body: &str, content_type: &str) -> Option<String> {
    extract_result(body, content_type).map(|(t, _)| t)
}

fn extract_api_error(body: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        for ptr in ["/error/message", "/error/code", "/message", "/detail", "/error"] {
            if let Some(s) = v.pointer(ptr).and_then(|x| x.as_str()) {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    let trimmed = body.trim();
    if !trimmed.is_empty() && trimmed.len() < 300 {
        return Some(trimmed.to_string());
    }
    None
}

/// 本机地址一律绕过代理
fn is_local_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    match parsed.host_str() {
        Some("localhost") | Some("127.0.0.1") | Some("0.0.0.0") | Some("[::1]") | Some("::1") => true,
        Some(host) => host.starts_with("127.") || host.ends_with(".local"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_urls_bypass_proxy() {
        assert!(is_local_url("http://localhost:8080/inference"));
        assert!(is_local_url("http://127.0.0.1:8000/v1/audio/transcriptions"));
        assert!(!is_local_url("https://api.groq.com/openai/v1/audio/transcriptions"));
        assert!(!is_local_url("https://localhost.evil.com/x"));
    }

    #[test]
    fn parses_openai_json_response() {
        let body = r#"{"text":" 你好世界 "}"#;
        assert_eq!(extract_text(body, "application/json").unwrap(), "你好世界");
    }

    #[test]
    fn parses_verbose_json_response() {
        let body = r#"{"task":"transcribe","language":"zh","duration":3.2,"text":"会议开始","segments":[{"id":0,"text":"会议开始"}]}"#;
        let (text, lang) = extract_result(body, "application/json").unwrap();
        assert_eq!(text, "会议开始");
        assert_eq!(lang.as_deref(), Some("zh"));
    }

    #[test]
    fn json_format_has_no_language_field() {
        let (text, lang) = extract_result(r#"{"text":"你好"}"#, "application/json").unwrap();
        assert_eq!(text, "你好");
        assert!(lang.is_none());
    }

    #[test]
    fn parses_wrapped_response() {
        let body = r#"{"result":{"text":"嵌套结果"}}"#;
        assert_eq!(extract_text(body, "application/json").unwrap(), "嵌套结果");
    }

    #[test]
    fn parses_plain_text_response() {
        // response_format=text 时服务直接返回纯文本
        assert_eq!(extract_text("纯文本结果", "text/plain").unwrap(), "纯文本结果");
        // whisper.cpp 在 text 格式下也会返回裸文本
        assert_eq!(extract_text("Hello world\n", "text/plain").unwrap(), "Hello world");
    }

    #[test]
    fn rejects_html_error_pages() {
        let html = "<!DOCTYPE html><html><body>502 Bad Gateway</body></html>";
        assert!(extract_text(html, "text/html").is_none());
        assert!(extract_text(html, "text/plain").is_none());
    }

    #[test]
    fn empty_text_is_not_a_valid_result() {
        assert!(extract_text("", "application/json").is_none());
        assert!(extract_text("   ", "text/plain").is_none());
        // 服务对纯静音可能返回空 text，此时文本为空字符串而不是 None
        assert_eq!(extract_text(r#"{"text":""}"#, "application/json").unwrap(), "");
    }

    #[test]
    fn extracts_error_messages_from_json_and_plain() {
        assert_eq!(
            extract_api_error(r#"{"error":{"message":"Invalid API key"}}"#).unwrap(),
            "Invalid API key"
        );
        assert!(extract_api_error("<html>502</html>").unwrap().contains("502"));
        assert!(extract_api_error("").is_none());
    }

    #[test]
    fn friendly_error_hints() {
        let e = AsrError::Status { code: 401, message: "unauthorized".into() };
        assert!(e.to_app_error().to_string().contains("API Key"));
        let e = AsrError::Status { code: 404, message: String::new() };
        assert!(e.to_app_error().to_string().contains("模型名"));
        let e = AsrError::Status { code: 429, message: String::new() };
        assert!(e.to_app_error().to_string().contains("频繁"));
    }

    #[test]
    fn retryable_statuses() {
        let mk = |code| AsrError::Status { code, message: String::new() };
        assert!(mk(429).is_retryable());
        assert!(mk(503).is_retryable());
        assert!(!mk(400).is_retryable());
        assert!(!mk(401).is_retryable());
    }

    #[test]
    fn client_constructs() {
        assert!(AsrClient::new().is_ok());
    }
}
