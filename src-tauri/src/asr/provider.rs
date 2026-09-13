//! 语音识别服务商预设。
//!
//! 全部走 HTTP 接口，App 本身不包含任何识别模型：
//!   * **OpenAI 兼容**：`POST {base}/audio/transcriptions`，multipart 上传音频文件。
//!     用这套接口的服务很多 —— OpenAI、Groq、硅基流动、faster-whisper-server、LM Studio 等。
//!   * **whisper.cpp server**：`POST {base}/inference`，同样是 multipart，
//!     响应结构与 OpenAI 的 `json` 格式一致（`{"text": "..."}`）。
//!
//! 也就是说「本地识别」依然可以做，只是以**本地服务**的形式存在：
//! 起一个 whisper.cpp server 或 faster-whisper-server，把 Base URL 指向它即可。

use serde::Serialize;

use crate::settings::AsrProvider;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub transcription_path: String,
    pub model: String,
    pub note: String,
    /// 是否需要 api key（本地服务不需要）
    pub needs_key: bool,
    /// 是否本地/自建（UI 会提示可以开启「边说边出字」）
    pub local: bool,
    pub response_format: String,
}

/// (id, 名称, base_url, 路径, 模型, 说明, 需要 key, 是否本地)
const PRESETS: &[(&str, &str, &str, &str, &str, &str, bool, bool)] = &[
    (
        "groq",
        "Groq（推荐）",
        "https://api.groq.com/openai/v1",
        "/audio/transcriptions",
        "whisper-large-v3-turbo",
        "速度极快、价格低，OpenAI 兼容；中文效果也好",
        true,
        false,
    ),
    (
        "openai",
        "OpenAI",
        "https://api.openai.com/v1",
        "/audio/transcriptions",
        "whisper-1",
        "官方接口，稳定；需要能访问 openai.com 的网络",
        true,
        false,
    ),
    (
        "siliconflow",
        "硅基流动 SiliconFlow",
        "https://api.siliconflow.cn/v1",
        "/audio/transcriptions",
        "FunAudioLLM/SenseVoiceSmall",
        "国内直连，中文识别好，有免费额度",
        true,
        false,
    ),
    (
        "local-whispercpp",
        "本地 whisper.cpp server",
        "http://localhost:8080",
        "/inference",
        "whisper-1",
        "本地完全离线：whisper-server -m ggml-large-v3-turbo.bin --port 8080",
        false,
        true,
    ),
    (
        "local-faster-whisper",
        "本地 faster-whisper-server",
        "http://localhost:8000/v1",
        "/audio/transcriptions",
        "Systran/faster-whisper-large-v3",
        "本地完全离线：docker run -p 8000:8000 fedirz/faster-whisper-server",
        false,
        true,
    ),
    (
        "local-lmstudio",
        "本地 LM Studio / 其它",
        "http://localhost:1234/v1",
        "/audio/transcriptions",
        "whisper-1",
        "任何提供 OpenAI 兼容转写接口的本地服务",
        false,
        true,
    ),
    (
        "custom",
        "自定义（OpenAI 兼容）",
        "https://",
        "/audio/transcriptions",
        "",
        "任何实现了 /audio/transcriptions 的服务，包括自建网关",
        true,
        false,
    ),
];

pub fn presets() -> Vec<AsrPreset> {
    PRESETS
        .iter()
        .map(
            |(id, name, base_url, path, model, note, needs_key, local)| AsrPreset {
                id: (*id).to_string(),
                name: (*name).to_string(),
                base_url: (*base_url).to_string(),
                transcription_path: (*path).to_string(),
                model: (*model).to_string(),
                note: (*note).to_string(),
                needs_key: *needs_key,
                local: *local,
                response_format: "json".into(),
            },
        )
        .collect()
}

pub fn provider_from_preset(id: &str) -> AsrProvider {
    let found = PRESETS.iter().find(|p| p.0 == id).copied().unwrap_or(PRESETS[0]);
    let (id, name, base_url, path, model, _note, _needs_key, _local) = found;
    AsrProvider {
        id: id.to_string(),
        name: name.to_string(),
        base_url: base_url.to_string(),
        transcription_path: path.to_string(),
        api_key: String::new(),
        model: model.to_string(),
        response_format: "json".into(),
        timeout_secs: 120,
        extra_headers: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_unique_and_usable() {
        let list = presets();
        assert!(list.len() >= 5);
        let mut ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "预设 id 不应重复");

        for p in &list {
            assert!(!p.name.is_empty());
            assert!(p.base_url.starts_with("http"), "{} 的 base_url 不合法", p.id);
            assert!(p.transcription_path.starts_with('/'), "{} 的路径不合法", p.id);
            assert!(!p.note.is_empty());
        }
    }

    #[test]
    fn local_presets_need_no_key() {
        for p in presets().iter().filter(|p| p.local) {
            assert!(!p.needs_key, "本地服务不应要求 api key：{}", p.id);
        }
    }

    #[test]
    fn every_preset_builds_a_valid_endpoint() {
        for preset in presets() {
            let provider = provider_from_preset(&preset.id);
            let endpoint = provider.endpoint();
            assert!(endpoint.starts_with("http"), "{} 生成了非法地址 {endpoint}", preset.id);
            assert!(
                endpoint.ends_with(&preset.transcription_path),
                "{} 的地址没带上路径：{endpoint}",
                preset.id
            );
        }
    }

    #[test]
    fn unknown_preset_falls_back_to_default() {
        let p = provider_from_preset("不存在的服务商");
        assert_eq!(p.id, "groq");
        assert!(p.is_usable());
    }

    #[test]
    fn provider_is_usable_requires_base_url_and_model() {
        let mut p = provider_from_preset("openai");
        assert!(p.is_usable());
        p.model.clear();
        assert!(!p.is_usable());
        p.model = "whisper-1".into();
        p.base_url.clear();
        assert!(!p.is_usable());
    }
}
