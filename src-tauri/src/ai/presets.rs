//! 常见 AI 服务商预设。前端设置页用它一键填充 base_url / model / jsonMode。

use serde::Serialize;

use crate::settings::AiProvider;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub note: String,
    /// 是否必须填 api key（本地 ollama 不需要）
    pub needs_key: bool,
    pub json_mode: bool,
}

const PRESETS: &[(&str, &str, &str, &str, &str, bool, bool)] = &[
    (
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com/v1",
        "deepseek-chat",
        "中文总结质量好、价格低，推荐首选",
        true,
        true,
    ),
    (
        "openai",
        "OpenAI",
        "https://api.openai.com/v1",
        "gpt-4o-mini",
        "通用能力强，需要能访问 openai.com 的网络",
        true,
        true,
    ),
    (
        "dashscope",
        "阿里通义千问",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "qwen-plus",
        "国内直连，OpenAI 兼容模式",
        true,
        true,
    ),
    (
        "zhipu",
        "智谱 GLM",
        "https://open.bigmodel.cn/api/paas/v4",
        "glm-4-flash",
        "有免费额度，速度快",
        true,
        true,
    ),
    (
        "moonshot",
        "月之暗面 Kimi",
        "https://api.moonshot.cn/v1",
        "moonshot-v1-8k",
        "长文本能力好",
        true,
        true,
    ),
    (
        "siliconflow",
        "硅基流动",
        "https://api.siliconflow.cn/v1",
        "Qwen/Qwen2.5-7B-Instruct",
        "聚合多家开源模型，有免费模型",
        true,
        true,
    ),
    (
        "ollama",
        "本地 Ollama",
        "http://localhost:11434/v1",
        "qwen3.5:2b",
        "完全离线，数据不出本机；需先 ollama pull 模型",
        false,
        true,
    ),
    (
        "vllm",
        "本地 vLLM / LM Studio",
        "http://localhost:8000/v1",
        "Qwen/Qwen2.5-7B-Instruct",
        "本地自建 OpenAI 兼容服务",
        false,
        true,
    ),
    (
        "custom",
        "自定义（OpenAI 兼容）",
        "https://",
        "",
        "任何兼容 /chat/completions 的服务",
        true,
        false,
    ),
];

pub fn presets() -> Vec<AiPreset> {
    PRESETS
        .iter()
        .map(|(id, name, base_url, model, note, needs_key, json_mode)| AiPreset {
            id: (*id).to_string(),
            name: (*name).to_string(),
            base_url: (*base_url).to_string(),
            model: (*model).to_string(),
            note: (*note).to_string(),
            needs_key: *needs_key,
            json_mode: *json_mode,
        })
        .collect()
}

/// 用预设 id 生成一个可用的服务商配置
pub fn provider_from_preset(id: &str) -> AiProvider {
    let found = PRESETS.iter().find(|p| p.0 == id).copied().unwrap_or(PRESETS[0]);
    let (id, name, base_url, model, _note, _needs_key, json_mode) = found;
    AiProvider {
        id: id.to_string(),
        name: name.to_string(),
        base_url: base_url.to_string(),
        api_key: String::new(),
        model: model.to_string(),
        // 总结任务要稳定复现，温度压低
        temperature: 0.2,
        max_tokens: 1_200,
        json_mode,
        timeout_secs: 60,
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
        }
    }

    #[test]
    fn unknown_preset_falls_back() {
        let p = provider_from_preset("不存在的服务商");
        assert_eq!(p.id, "deepseek");
    }

    #[test]
    fn ollama_preset_needs_no_key() {
        let list = presets();
        let ollama = list.iter().find(|p| p.id == "ollama").unwrap();
        assert!(!ollama.needs_key);
    }
}
