/**
 * 服务商控制台 / 下载页链接。
 *
 * 向导与设置里都用 `api.openUrl` 打开（Rust 侧只放行 http/https）。
 * 用关键字匹配而不是精确的 preset id：服务商可能被复制成 `groq-2` 之类的 id，
 * 用户也可能自己改名字，按 id + 名称 + Base URL 一起匹配更稳。
 */
export interface ConsoleLink {
  label: string;
  url: string;
}

interface LinkRule {
  match: RegExp;
  link: ConsoleLink;
}

const RULES: LinkRule[] = [
  { match: /groq/i, link: { label: "打开 Groq 控制台申请 Key", url: "https://console.groq.com/keys" } },
  {
    match: /openai/i,
    link: { label: "打开 OpenAI 平台申请 Key", url: "https://platform.openai.com/api-keys" },
  },
  {
    match: /siliconflow|硅基/i,
    link: { label: "打开硅基流动申请 Key", url: "https://cloud.siliconflow.cn/account/ak" },
  },
  {
    match: /deepseek/i,
    link: { label: "打开 DeepSeek 开放平台申请 Key", url: "https://platform.deepseek.com/api_keys" },
  },
  {
    match: /dashscope|通义|qwen\.aliyun|阿里/i,
    link: { label: "打开阿里云百炼申请 Key", url: "https://bailian.console.aliyun.com/" },
  },
  {
    match: /zhipu|glm|智谱|bigmodel/i,
    link: { label: "打开智谱开放平台申请 Key", url: "https://open.bigmodel.cn/usercenter/apikeys" },
  },
  {
    match: /whisper\.cpp|whispercpp/i,
    link: { label: "whisper.cpp 文档与下载", url: "https://github.com/ggml-org/whisper.cpp" },
  },
  {
    match: /faster-whisper/i,
    link: { label: "faster-whisper-server 文档", url: "https://github.com/fedirz/faster-whisper-server" },
  },
  { match: /lm ?studio/i, link: { label: "下载 LM Studio", url: "https://lmstudio.ai/" } },
  { match: /ollama/i, link: { label: "下载 Ollama（本地大模型）", url: "https://ollama.com/download" } },
];

/** 根据服务商信息推断可打开的官方页面；识别不出来时返回空数组 */
export function consoleLinks(...hints: (string | null | undefined)[]): ConsoleLink[] {
  const text = hints.filter(Boolean).join(" ");
  if (!text) return [];
  const found = RULES.filter((r) => r.match.test(text)).map((r) => r.link);
  // 去重（例如名称与 baseUrl 同时命中同一条规则）
  const seen = new Set<string>();
  return found.filter((l) => (seen.has(l.url) ? false : (seen.add(l.url), true)));
}

export const BLACKHOLE_PAGE: ConsoleLink = {
  label: "打开 BlackHole 下载页",
  url: "https://existential.audio/blackhole/",
};

export const OLLAMA_PAGE: ConsoleLink = {
  label: "下载 Ollama",
  url: "https://ollama.com/download",
};

/** 本机 whisper.cpp server 的启动命令示例（向导第 2 步与设置里共用） */
export const WHISPER_CPP_CMD = "./build/bin/whisper-server -m models/ggml-base.bin --port 8080";
