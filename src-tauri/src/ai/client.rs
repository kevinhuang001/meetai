//! OpenAI 兼容接口客户端。
//!
//! 设计要点：
//!  - 只依赖 `/chat/completions` 与 `/models` 两个标准端点，兼容 DeepSeek / OpenAI /
//!    DashScope / 智谱 / Ollama / vLLM 等一切 OpenAI 兼容服务。
//!  - 本机可能配置了 HTTP 代理（企业网络很常见），而本地大模型走代理必然失败。
//!    因此内置两个 reqwest Client：访问 localhost 时用 `no_proxy()` 的那个。
//!  - 部分服务商不支持 `response_format: json_object`，遇到 400 会自动去掉该字段重试一次。

use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::settings::AiProvider;
use crate::util::truncate_chars;

const USER_AGENT: &str = concat!("MeetingHear/", env!("CARGO_PKG_VERSION"));
/// 单次请求最多尝试几次（含首次）。模型「处理不过来」时全靠它兜住。
const MAX_ATTEMPTS: u32 = 4;
/// 指数退避：1s → 2s → 4s，最长等 8 秒
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64 << (attempt.saturating_sub(1).min(3));
    Duration::from_secs(secs.min(8))
}

/* ==========================================================================
 * 数据类型
 * ========================================================================== */

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(other.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub content: String,
    pub model: String,
    pub usage: TokenUsage,
    pub latency_ms: u64,
    /// 是否因为服务商不支持 json_object 而退化为普通模式
    pub json_degraded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTest {
    pub ok: bool,
    pub latency_ms: u64,
    pub model: String,
    pub sample: String,
    pub error: Option<String>,
    pub models: Vec<String>,
}

/* ==========================================================================
 * 内部错误
 * ========================================================================== */

#[derive(Debug)]
enum ChatError {
    /// HTTP 层错误（非 2xx）
    Status { code: u16, message: String, raw: String },
    /// 网络/超时/解析
    Transport(AppError),
}

impl ChatError {
    fn is_json_mode_rejection(&self) -> bool {
        match self {
            ChatError::Status { code, raw, message } => {
                *code == 400
                    && (raw.contains("response_format")
                        || raw.contains("json_object")
                        || message.contains("response_format")
                        || message.contains("json_object"))
            }
            _ => false,
        }
    }

    fn is_retryable(&self) -> bool {
        match self {
            ChatError::Status { code, .. } => *code == 429 || *code >= 500,
            ChatError::Transport(_) => true,
        }
    }

    fn to_app_error(self) -> AppError {
        match self {
            ChatError::Status { code, message, .. } => {
                let hint = match code {
                    401 | 403 => "鉴权失败，请检查 API Key 是否正确、是否已过期",
                    404 => {
                        "接口不存在，请检查 Base URL 是否填写正确（应形如 https://api.deepseek.com/v1，不要带 /chat/completions）"
                    }
                    429 => "请求过于频繁或额度不足，请稍后重试或检查账户余额",
                    413 => "内容过长被拒绝，可调小「上下文上限」",
                    _ => "服务返回错误",
                };
                let detail = if message.trim().is_empty() {
                    String::new()
                } else {
                    format!("：{}", truncate_chars(message.trim(), 300))
                };
                AppError::ai(format!("{hint}（HTTP {code}）{detail}"))
            }
            ChatError::Transport(e) => e,
        }
    }
}

/* ==========================================================================
 * 客户端
 * ========================================================================== */

pub struct AiClient {
    proxied: Client,
    direct: Client,
}

impl AiClient {
    pub fn new() -> AppResult<Self> {
        let proxied = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| AppError::ai(format!("初始化 HTTP 客户端失败：{e}")))?;
        // 本地模型服务绝不能走代理
        let direct = Client::builder()
            .user_agent(USER_AGENT)
            .no_proxy()
            .build()
            .map_err(|e| AppError::ai(format!("初始化 HTTP 客户端失败：{e}")))?;
        Ok(Self { proxied, direct })
    }

    fn client_for(&self, url: &str) -> &Client {
        if is_local_url(url) {
            &self.direct
        } else {
            &self.proxied
        }
    }

    /* ---------------------------- 对话补全 ---------------------------- */

    pub async fn chat(&self, provider: &AiProvider, messages: &[ChatMessage]) -> AppResult<ChatOutcome> {
        self.chat_with_limit(provider, messages, None).await
    }

    pub async fn chat_with_limit(
        &self,
        provider: &AiProvider,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
    ) -> AppResult<ChatOutcome> {
        let provider = provider.clone().sanitized();
        if provider.root_url().is_empty() {
            return Err(AppError::ai("尚未配置 AI 服务的 Base URL"));
        }
        if provider.model.trim().is_empty() {
            return Err(AppError::ai("尚未配置模型名称"));
        }

        let url = provider.chat_url();
        let timeout = Duration::from_secs(provider.timeout_secs as u64);
        let started = Instant::now();
        let mut use_json = provider.json_mode;
        let mut attempt = 0u32;
        let mut json_degraded = false;

        loop {
            attempt += 1;
            let body = build_body(&provider, messages, max_tokens, use_json);
            match self.post_once(&url, &provider, &body, timeout).await {
                Ok(mut outcome) => {
                    outcome.latency_ms = started.elapsed().as_millis() as u64;
                    outcome.json_degraded = json_degraded;
                    return Ok(outcome);
                }
                Err(err) => {
                    // 服务商不支持 json_object → 去掉重试
                    if attempt == 1 && use_json && err.is_json_mode_rejection() {
                        tracing::info!("服务商不支持 response_format=json_object，降级重试");
                        use_json = false;
                        json_degraded = true;
                        continue;
                    }
                    // 限流 / 5xx / 超时 / 网络抖动 → 指数退避重试。
                    //
                    // 本地小模型（Ollama 等）在会议进行中很容易被压住而超时，
                    // 这种失败几乎都能靠重试消化掉。**重试比丢内容重要得多**：
                    // 漏掉的那一段会直接消失在纪要里，而多等几秒没人会注意到。
                    if attempt <= MAX_ATTEMPTS && err.is_retryable() {
                        let wait = backoff_delay(attempt);
                        tracing::warn!(
                            "AI 请求失败（第 {attempt}/{MAX_ATTEMPTS} 次），{} 秒后重试：{err:?}",
                            wait.as_secs_f32()
                        );
                        tokio::time::sleep(wait).await;
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
        provider: &AiProvider,
        body: &Value,
        timeout: Duration,
    ) -> Result<ChatOutcome, ChatError> {
        let mut req = self
            .client_for(url)
            .post(url)
            .timeout(timeout)
            .header("content-type", "application/json");

        if !provider.api_key.trim().is_empty() {
            req = req.bearer_auth(provider.api_key.trim());
        }
        for (k, v) in &provider.extra_headers {
            if !k.trim().is_empty() {
                req = req.header(k.trim(), v.trim());
            }
        }

        let resp = req
            .json(body)
            .send()
            .await
            .map_err(|e| ChatError::Transport(map_reqwest_error(e)))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| ChatError::Transport(map_reqwest_error(e)))?;

        if !status.is_success() {
            return Err(ChatError::Status {
                code: status.as_u16(),
                message: extract_api_error(&text).unwrap_or_default(),
                raw: text,
            });
        }

        parse_chat_response(&text).map_err(ChatError::Transport)
    }

    /* ---------------------------- 模型列表 ---------------------------- */

    pub async fn list_models(&self, provider: &AiProvider) -> AppResult<Vec<String>> {
        let provider = provider.clone().sanitized();
        let root = provider.root_url();
        if root.is_empty() {
            return Err(AppError::ai("尚未配置 AI 服务的 Base URL"));
        }
        let url = provider.models_url();
        let mut req = self
            .client_for(&url)
            .get(&url)
            .timeout(Duration::from_secs(15))
            .header("content-type", "application/json");
        if !provider.api_key.trim().is_empty() {
            req = req.bearer_auth(provider.api_key.trim());
        }
        for (k, v) in &provider.extra_headers {
            if !k.trim().is_empty() {
                req = req.header(k.trim(), v.trim());
            }
        }

        let resp = req.send().await.map_err(|e| map_reqwest_error(e))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| map_reqwest_error(e))?;
        if !status.is_success() {
            let msg = extract_api_error(&text).unwrap_or_default();
            return Err(AppError::ai(format!(
                "拉取模型列表失败（HTTP {}）{}",
                status.as_u16(),
                if msg.is_empty() { String::new() } else { format!("：{msg}") }
            )));
        }

        let v: Value = serde_json::from_str(&text)
            .map_err(|e| AppError::ai(format!("模型列表返回不是合法 JSON：{e}")))?;
        let mut out = Vec::new();
        // OpenAI 兼容：{"data":[{"id":"..."}]}；Ollama 原生：{"models":[{"name":"..."}]}
        for key in ["data", "models"] {
            if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
                for item in arr {
                    let id = item
                        .get("id")
                        .or_else(|| item.get("name"))
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if !id.is_empty() {
                        out.push(id);
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /* ---------------------------- 连通性测试 ---------------------------- */

    pub async fn test_connection(&self, provider: &AiProvider) -> ConnectionTest {
        let model = provider.model.clone();
        // 模型列表失败不影响结论，属于可选信息
        let models = match self.list_models(provider).await {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("拉取模型列表失败（忽略）：{e}");
                Vec::new()
            }
        };

        let mut probe = provider.clone().sanitized();
        probe.timeout_secs = probe.timeout_secs.min(30);
        probe.json_mode = false;
        let messages = vec![
            ChatMessage::system("你是一个连通性测试端点，只按用户要求回复。"),
            ChatMessage::user("请只回复两个字：正常"),
        ];

        let started = Instant::now();
        match self.chat_with_limit(&probe, &messages, Some(32)).await {
            Ok(out) => ConnectionTest {
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                model: if out.model.is_empty() { model } else { out.model },
                sample: truncate_chars(out.content.trim(), 200),
                error: None,
                models,
            },
            Err(e) => ConnectionTest {
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                model,
                sample: String::new(),
                error: Some(e.to_string()),
                models,
            },
        }
    }
}

/* ==========================================================================
 * 辅助函数
 * ========================================================================== */

fn build_body(
    provider: &AiProvider,
    messages: &[ChatMessage],
    max_tokens: Option<u32>,
    json_mode: bool,
) -> Value {
    let mut body = json!({
        "model": provider.model.trim(),
        "messages": messages,
        "temperature": provider.temperature,
        "stream": false,
    });
    let limit = max_tokens.unwrap_or(provider.max_tokens);
    if limit > 0 {
        body["max_tokens"] = json!(limit);
    }
    if json_mode {
        body["response_format"] = json!({"type": "json_object"});
    }
    body
}

fn parse_chat_response(text: &str) -> AppResult<ChatOutcome> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| AppError::ai(format!("接口返回不是合法 JSON：{e}")))?;

    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| err.as_str().unwrap_or("未知错误"));
        return Err(AppError::ai(format!("接口返回错误：{msg}")));
    }

    let choice = v
        .get("choices")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| AppError::ai("接口返回中没有 choices 字段，可能不是 OpenAI 兼容接口"))?;

    let message = choice.get("message");
    let content = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();

    // 部分推理模型把正文放在 reasoning_content，content 为空时兜底
    let content = if content.trim().is_empty() {
        message
            .and_then(|m| m.get("reasoning_content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string()
    } else {
        content
    };

    if content.trim().is_empty() {
        let reason = choice.get("finish_reason").and_then(|x| x.as_str()).unwrap_or("");
        return Err(AppError::ai(if reason == "length" {
            "模型输出被 max_tokens 截断且内容为空，请调大 max_tokens".to_string()
        } else {
            "模型返回内容为空".to_string()
        }));
    }

    let usage = v
        .get("usage")
        .map(|u| TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            total_tokens: u.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        })
        .unwrap_or_default();

    Ok(ChatOutcome {
        content,
        model: v.get("model").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        usage,
        latency_ms: 0,
        json_degraded: false,
    })
}

fn extract_api_error(body: &str) -> Option<String> {
    // 注意：这里不能用 `?` 提前返回，否则非 JSON 的错误响应（网关 HTML 页）
    // 会绕过下面的兜底分支，用户只能看到光秃秃的 HTTP 状态码。
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        for ptr in ["/error/message", "/error/code", "/message", "/detail", "/error"] {
            if let Some(s) = v.pointer(ptr).and_then(|x| x.as_str()) {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    // 非 JSON 的错误页（例如网关返回 HTML）
    let trimmed = body.trim();
    if !trimmed.is_empty() && trimmed.len() < 300 {
        return Some(trimmed.to_string());
    }
    None
}

fn map_reqwest_error(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::ai("请求超时：模型响应太慢，可调大超时时间或换更小的模型")
    } else if e.is_connect() {
        AppError::ai(format!("无法连接到 AI 服务：{e}（本地模型请确认服务已启动）"))
    } else {
        AppError::ai(format!("网络请求失败：{e}"))
    }
}

/// 本机/局域网/内网穿透地址一律绕过系统代理（判定逻辑见 [`crate::util::should_bypass_proxy`]）
fn is_local_url(url: &str) -> bool {
    crate::util::should_bypass_proxy(url)
}

#[allow(dead_code)]
fn status_is_retryable(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}

/* ==========================================================================
 * 测试
 * ========================================================================== */

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AiProvider {
        AiProvider {
            base_url: "https://api.example.com/v1".into(),
            model: "test-model".into(),
            json_mode: true,
            ..Default::default()
        }
    }

    #[test]
    fn local_urls_bypass_proxy() {
        assert!(is_local_url("http://localhost:11434/v1/chat/completions"));
        assert!(is_local_url("http://127.0.0.1:8000/v1/chat/completions"));
        assert!(is_local_url("http://127.1.2.3:8000/v1/models"));
        assert!(!is_local_url("https://api.deepseek.com/v1/chat/completions"));
        assert!(!is_local_url("https://localhost.evil.com/v1"));
    }

    #[test]
    fn body_includes_json_mode_and_limits() {
        let p = provider();
        let msgs = vec![ChatMessage::user("你好")];
        let body = build_body(&p, &msgs, Some(64), true);
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "test-model");

        let body = build_body(&p, &msgs, None, false);
        assert!(body.get("response_format").is_none());
        assert_eq!(body["max_tokens"], 1200);
    }

    #[test]
    fn parses_normal_response() {
        let raw = r#"{"model":"deepseek-chat","choices":[{"message":{"content":"正常"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
        let out = parse_chat_response(raw).unwrap();
        assert_eq!(out.content, "正常");
        assert_eq!(out.usage.total_tokens, 12);
        assert_eq!(out.model, "deepseek-chat");
    }

    #[test]
    fn falls_back_to_reasoning_content() {
        let raw = r#"{"choices":[{"message":{"content":"","reasoning_content":"思考结果"}}]}"#;
        assert_eq!(parse_chat_response(raw).unwrap().content, "思考结果");
    }

    #[test]
    fn empty_content_reports_length_truncation() {
        let raw = r#"{"choices":[{"message":{"content":""},"finish_reason":"length"}]}"#;
        let err = parse_chat_response(raw).unwrap_err().to_string();
        assert!(err.contains("max_tokens"), "实际错误：{err}");
    }

    #[test]
    fn extracts_error_messages() {
        assert_eq!(
            extract_api_error(r#"{"error":{"message":"Invalid API key","type":"auth"}}"#).unwrap(),
            "Invalid API key"
        );
        assert!(extract_api_error("<html>502 Bad Gateway</html>").unwrap().contains("502"));
    }

    #[test]
    fn json_mode_rejection_detection() {
        let e = ChatError::Status {
            code: 400,
            message: "response_format is not supported".into(),
            raw: String::new(),
        };
        assert!(e.is_json_mode_rejection());
        let e2 = ChatError::Status { code: 400, message: "bad model".into(), raw: String::new() };
        assert!(!e2.is_json_mode_rejection());
    }

    #[test]
    fn retryable_statuses() {
        let mk = |code| ChatError::Status { code, message: String::new(), raw: String::new() };
        assert!(mk(429).is_retryable());
        assert!(mk(503).is_retryable());
        assert!(!mk(400).is_retryable());
        assert!(!mk(401).is_retryable());
    }

    #[test]
    fn friendly_error_messages() {
        let e = ChatError::Status { code: 401, message: "unauthorized".into(), raw: String::new() };
        let msg = e.to_app_error().to_string();
        assert!(msg.contains("API Key"), "实际：{msg}");

        let e = ChatError::Status { code: 404, message: String::new(), raw: String::new() };
        assert!(e.to_app_error().to_string().contains("Base URL"));
    }

    #[test]
    fn client_constructs() {
        assert!(AiClient::new().is_ok());
    }
}
