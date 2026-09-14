/**
 * 浏览器 mock 后端。
 *
 * 用途：
 *  1. `pnpm dev` 直接开浏览器就能开发/调试界面，不需要跑 Rust；
 *  2. 自动化 UI 测试可以在没有麦克风、没有识别服务的机器上验证完整交互。
 *
 * 它模拟的时序与真实后端一致：partial 高频刷新 → 定稿成 segment → AI 周期性产出纪要。
 * 注意：App 本身不含识别模型，语音识别一律走 HTTP 识别服务，mock 里也只模拟服务商配置。
 */
import {
  EV,
  emptyStats,
  emptySummary,
  type AiPreset,
  type AiProvider,
  type AppInfo,
  type AppErrorEvent,
  type AsrConnectionTestResult,
  type AsrLevelEvent,
  type AsrPartialEvent,
  type AsrSegmentEvent,
  type AsrStateEvent,
  type AiStatusEvent,
  type AiSummaryEvent,
  type AudioSourceInfo,
  type ExportFormat,
  type AsrPreset,
  type AsrProvider,
  type SessionDetail,
  type SessionInfo,
  type SessionListItem,
  type SessionUpdatedEvent,
  type Settings,
  type StartSessionRequest,
  type SummaryState,
  type TranscriptSegment,
} from "./contract";

declare global {
  interface Window {
    /** 冒烟测试用：mock 下 open_url 的调用记录（真实后端会调系统浏览器） */
    __MEETING_HEAR_OPENED_URLS__?: string[];
  }
}

/* ------------------------------- 测试开关 ------------------------------- */

/**
 * 浏览器 mock 的少量「环境模拟」开关，通过 URL 查询参数控制，仅测试使用：
 *   ?mockPlatform=macos|windows|linux  —— 伪造 appInfo.platform（用于验证各平台的内录引导）
 *   ?mockLoopback=none                 —— 模拟「没有检测到系统内录设备」
 *   ?mockOnboarding=1                  —— 让 get_settings 直接返回「已完成向导」
 */
function queryParam(name: string): string | null {
  if (typeof window === "undefined") return null;
  try {
    return new URL(window.location.href).searchParams.get(name);
  } catch {
    return null;
  }
}

const PLATFORM_OVERRIDE = queryParam("mockPlatform");
const NO_LOOPBACK = queryParam("mockLoopback") === "none";
const FORCE_ONBOARDED = queryParam("mockOnboarding") === "1";

/* ------ 向导完成标记的持久化（浏览器 mock 没有配置文件，用 localStorage 代替） ------ */

const LS_ONBOARDING = "meeting-hear:mock:onboardingCompleted";

function readOnboardingFlag(): boolean {
  if (FORCE_ONBOARDED) return true;
  if (typeof localStorage === "undefined") return false;
  try {
    return localStorage.getItem(LS_ONBOARDING) === "1";
  } catch {
    return false;
  }
}

function writeOnboardingFlag(v: boolean) {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(LS_ONBOARDING, v ? "1" : "0");
  } catch {
    /* 隐私模式下写入失败可以忽略 */
  }
}

/* ------------------------------- 事件总线 ------------------------------- */

type Handler = (payload: unknown) => void;
const listeners = new Map<string, Set<Handler>>();

export function mockListen<T>(event: string, handler: (payload: T) => void): () => void {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(handler as Handler);
  return () => {
    set?.delete(handler as Handler);
  };
}

function emit(event: string, payload: unknown) {
  const set = listeners.get(event);
  if (!set) return;
  for (const h of [...set]) {
    try {
      h(payload);
    } catch (err) {
      console.error("[mock] 事件处理异常", event, err);
    }
  }
}

/* ------------------------------- 静态数据 ------------------------------- */

const ASR_PRESETS: AsrPreset[] = [
  {
    id: "groq",
    name: "Groq（推荐）",
    baseUrl: "https://api.groq.com/openai/v1",
    transcriptionPath: "/audio/transcriptions",
    model: "whisper-large-v3-turbo",
    note: "速度极快、价格低，OpenAI 兼容；中文效果也好",
    needsKey: true,
    local: false,
    responseFormat: "json",
  },
  {
    id: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    transcriptionPath: "/audio/transcriptions",
    model: "whisper-1",
    note: "官方接口，稳定；需要能访问 openai.com 的网络",
    needsKey: true,
    local: false,
    responseFormat: "json",
  },
  {
    id: "siliconflow",
    name: "硅基流动 SiliconFlow",
    baseUrl: "https://api.siliconflow.cn/v1",
    transcriptionPath: "/audio/transcriptions",
    model: "FunAudioLLM/SenseVoiceSmall",
    note: "国内直连，中文识别好，有免费额度",
    needsKey: true,
    local: false,
    responseFormat: "json",
  },
  {
    id: "local-whispercpp",
    name: "本地 whisper.cpp server",
    baseUrl: "http://localhost:8080",
    transcriptionPath: "/inference",
    model: "whisper-1",
    note: "本地完全离线：whisper-server -m ggml-large-v3-turbo.bin --port 8080",
    needsKey: false,
    local: true,
    responseFormat: "json",
  },
  {
    id: "local-faster-whisper",
    name: "本地 faster-whisper-server",
    baseUrl: "http://localhost:8000/v1",
    transcriptionPath: "/audio/transcriptions",
    model: "Systran/faster-whisper-large-v3",
    note: "本地完全离线：docker run -p 8000:8000 fedirz/faster-whisper-server",
    needsKey: false,
    local: true,
    responseFormat: "json",
  },
  {
    id: "local-lmstudio",
    name: "本地 LM Studio / 其它",
    baseUrl: "http://localhost:1234/v1",
    transcriptionPath: "/audio/transcriptions",
    model: "whisper-1",
    note: "任何提供 OpenAI 兼容转写接口的本地服务",
    needsKey: false,
    local: true,
    responseFormat: "json",
  },
  {
    id: "custom",
    name: "自定义（OpenAI 兼容）",
    baseUrl: "https://",
    transcriptionPath: "/audio/transcriptions",
    model: "",
    note: "任何实现了 /audio/transcriptions 的服务，包括自建网关",
    needsKey: true,
    local: false,
    responseFormat: "json",
  },
];

function providerFromPreset(preset: AsrPreset, id = preset.id): AsrProvider {
  return {
    id,
    name: preset.name,
    baseUrl: preset.baseUrl,
    transcriptionPath: preset.transcriptionPath,
    apiKey: "",
    model: preset.model,
    responseFormat: preset.responseFormat,
    timeoutSecs: 120,
    extraHeaders: [],
  };
}

/** 「服务商 · 模型」描述，与 Rust 侧 SessionConfig.model_id 的语义一致 */
function asrServiceLabel(s: Settings): string {
  const p = s.asr.providers.find((x) => x.id === s.asr.activeProviderId) ?? s.asr.providers[0];
  if (!p) return "未配置语音识别服务";
  return p.model.trim() ? `${p.name} · ${p.model}` : p.name;
}

const ALL_SOURCES: AudioSourceInfo[] = [
  {
    id: "mic:default",
    kind: "microphone",
    label: "默认麦克风（MacBook Pro 麦克风）",
    detail: "48000 Hz · 2 声道",
    isDefault: true,
    available: true,
    note: null,
  },
  {
    id: "mic:usb",
    kind: "microphone",
    label: "USB 会议全向麦",
    detail: "16000 Hz · 1 声道",
    isDefault: false,
    available: true,
    note: null,
  },
  {
    id: "loopback:default",
    kind: "loopback",
    label: "系统声音（扬声器输出）",
    detail: "48000 Hz · 2 声道",
    isDefault: true,
    available: true,
    note: null,
  },
];

/** ?mockLoopback=none 时模拟「没有任何可用的系统内录源」，用于验证各平台的安装引导 */
function listSources(): AudioSourceInfo[] {
  if (!NO_LOOPBACK) return ALL_SOURCES;
  return ALL_SOURCES.filter((s) => s.kind !== "loopback");
}

function sourceById(id: string): AudioSourceInfo | undefined {
  return ALL_SOURCES.find((s) => s.id === id);
}

const PRESETS: AiPreset[] = [
  {
    id: "deepseek",
    name: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    model: "deepseek-chat",
    note: "中文总结质量好，价格低",
    needsKey: true,
    jsonMode: true,
  },
  {
    id: "openai",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    note: "通用能力强",
    needsKey: true,
    jsonMode: true,
  },
  {
    id: "dashscope",
    name: "阿里通义千问",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "qwen-plus",
    note: "国内直连，OpenAI 兼容",
    needsKey: true,
    jsonMode: true,
  },
  {
    id: "zhipu",
    name: "智谱 GLM",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4-flash",
    note: "有免费档位",
    needsKey: true,
    jsonMode: true,
  },
  {
    id: "ollama",
    name: "本地 Ollama",
    baseUrl: "http://localhost:11434/v1",
    model: "qwen3.5:2b",
    note: "完全离线，无需 api key",
    needsKey: false,
    jsonMode: true,
  },
  {
    id: "custom",
    name: "自定义（OpenAI 兼容）",
    baseUrl: "https://",
    model: "",
    note: "任何兼容 /chat/completions 的服务",
    needsKey: true,
    jsonMode: false,
  },
];

function defaultProvider(id: string): AiProvider {
  const p = PRESETS.find((x) => x.id === id) ?? PRESETS[0];
  return {
    id: p.id,
    name: p.name,
    baseUrl: p.baseUrl,
    apiKey: "",
    model: p.model,
    temperature: 0.2,
    maxTokens: 1200,
    jsonMode: p.jsonMode,
    timeoutSecs: 60,
    extraHeaders: [],
  };
}

function defaultSettings(): Settings {
  const groq = providerFromPreset(ASR_PRESETS[0]);
  return {
    version: 1,
    asr: {
      enabled: true,
      providers: [groq],
      activeProviderId: groq.id,
      contextPrompt: true,
      temperature: 0,
      livePreview: false,
      maxChunkSecs: 25,
    },
    vad: {
      energyThresholdDb: 9,
      minSpeechMs: 250,
      minSilenceMs: 600,
      maxUtteranceMs: 25000,
      speechPadMs: 200,
    },
    ai: {
      enabled: true,
      providers: [defaultProvider("deepseek")],
      activeProviderId: "deepseek",
      summaryMode: "meeting",
      autoSummary: true,
      intervalSecs: 20,
      minNewChars: 60,
      liveWindowSecs: 90,
      maxContextChars: 4000,
      finalReportOnStop: false,
    },
    audio: {
      enableMic: true,
      micDeviceId: null,
      enableLoopback: true,
      loopbackDeviceId: null,
      micGain: 1,
      loopbackGain: 1,
      saveAudio: true,
    },
    general: {
      theme: "dark",
      autoScroll: true,
      fontScale: 1,
      dataDir: null,
      persistApiKey: true,
      // 首次运行：默认未完成向导；测试可用 ?mockOnboarding=1 或 localStorage 开关跳过
      onboardingCompleted: readOnboardingFlag(),
    },
  };
}

const APP_INFO: AppInfo = {
  name: "MeetingHear",
  version: "0.1.0 (mock)",
  platform: PLATFORM_OVERRIDE ?? "browser",
  arch: "wasm",
  dataDir: "/mock/data",
  sessionsDir: "/mock/data/sessions",
  recordingsDir: "/mock/data/recordings",
  asrService: "Groq（推荐） · whisper-large-v3-turbo · https://api.groq.com/openai/v1/audio/transcriptions",
  vadEngine: "energy（自适应能量 VAD）",
};

/* ------------------------------- 会话模拟 ------------------------------- */

/** 一段像真实会议的中文对话，用于驱动 UI */
const SCRIPT: { text: string; suspect?: string }[] = [
  { text: "各位，我们今天主要过三件事：上季度的增长复盘、下季度的目标，还有新版本的上线节奏。" },
  { text: "我先说复盘。三季度整体营收环比增长了百分之十八，主要来自企业版订阅。" },
  { text: "不过客户流失率也比二季度高了两个点，我觉得这块需要单独看一下。" },
  // 演示「可疑但照常显示」：识别服务确实返回了内容，只是看起来不对。
  // 它会标灰、不进纪要，但**不会被丢掉** —— 否则用户只会看到一个空白界面。
  { text: "Thank you for watching!", suspect: "疑似模型幻听短语" },
  { text: "对，流失主要集中在中小客户，原因统计里排第一的是上手成本太高。" },
  { text: "那下季度的目标我建议定在环比增长百分之十五，把留存放在更重要的位置。" },
  { text: "同意。另外新版本我们计划十一月十号发灰度，十一月二十四号全量。" },
  { text: "这个节奏可以，但前提是引导流程的重构要在十月底之前完成。" },
  { text: "我来负责引导流程，十月二十八号之前给到可测试的版本。" },
  { text: "好，那数据和埋点方案谁来出？这个直接影响我们怎么衡量留存改善。" },
  { text: "数据这边我来跟，下周三之前把指标口径和看板初稿发出来。" },
  { text: "另外提醒一下，市场侧的预算审批还没走完，可能会影响十一月的投放。" },
  { text: "这个风险记一下，我明天去找财务确认审批进度。" },
  { text: "如果没有别的问题，今天就到这里，辛苦大家。" },
];

let seq = 0;
function nextId() {
  return ++seq;
}

interface MockSession {
  info: SessionInfo;
  segments: TranscriptSegment[];
  summary: SummaryState;
  partial: AsrPartialEvent | null;
  scriptIndex: number;
  charsInScript: number;
  stats: ReturnType<typeof emptyStats>;
  timers: number[];
  startedAt: number;
  accumulatedMs: number;
}

let settings = defaultSettings();
let active: MockSession | null = null;
let sessionHistory: SessionListItem[] = [];
/** mock 下 open_url 的调用记录（供冒烟测试断言，真实后端不会有这个数组） */
const openedUrls: string[] = [];

function makeSummary(rev: number, scriptIndex: number, live: string): SummaryState {
  const reached = (n: number) => scriptIndex >= n;
  return {
    live,
    overview: reached(0)
      ? "会议围绕三季度增长复盘、下季度目标和新版本上线节奏展开，明确了留存优先的策略方向与关键时间点。"
      : "会议刚刚开始。",
    summary: [
      "- 三季度营收环比 +18%，主要来自企业版订阅",
      "- 客户流失率环比上升 2 个百分点，集中在中小客户，首因是上手成本高",
      reached(4) ? "- 下季度目标定为环比 +15%，重心从拉新转向留存" : "",
      reached(5) ? "- 新版本：11/10 灰度、11/24 全量" : "",
      reached(6) ? "- 前置依赖：引导流程重构需在 10 月底前完成" : "",
      reached(9) ? "- 数据埋点方案需先统一指标口径" : "",
      reached(10) ? "- 风险：市场预算审批未完成，可能影响 11 月投放" : "",
    ]
      .filter(Boolean)
      .join("\n"),
    sections: [
      {
        title: "三季度复盘",
        points: [
          "营收环比 +18%，主要来自企业版订阅",
          "客户流失率环比上升 2 个百分点，集中在中小客户，首因是上手成本高",
        ],
        untilMs: 9_000,
      },
      ...(reached(4)
        ? [
            {
              title: "下季度目标",
              points: [
                "目标定为环比 +15%，重心从拉新转向留存",
                "前置依赖：引导流程重构需在 10 月底前完成",
              ],
              untilMs: 24_000,
            },
          ]
        : []),
      ...(reached(5)
        ? [
            {
              title: "上线节奏与风险",
              points: [
                "新版本：11/10 灰度、11/24 全量",
                reached(9) ? "数据埋点方案需先统一指标口径" : "",
                reached(10) ? "市场预算审批未完成，可能影响 11 月投放" : "",
              ].filter(Boolean),
              untilMs: 42_000,
            },
          ]
        : []),
    ],
    keyPoints: [
      "企业版订阅是增长主引擎",
      reached(3) ? "中小客户上手成本高是流失主因" : "",
      reached(4) ? "下季度指标：环比 +15%，留存优先" : "",
    ].filter(Boolean),
    decisions: [reached(5) ? "确定新版本 11/10 灰度、11/24 全量" : ""].filter(Boolean),
    actionItems: [
      // 负责人写「谁认领的」这件事本身没变，但不再假设是「我 / 对方」
      reached(7) ? { text: "完成引导流程重构并可测试", owner: "", due: "10/28" } : null,
      reached(9) ? { text: "输出指标口径与数据看板初稿", owner: "", due: "下周三" } : null,
      reached(11) ? { text: "找财务确认市场预算审批进度", owner: "", due: "明天" } : null,
    ].filter((x): x is { text: string; owner: string; due: string } => x !== null),
    topics: scriptIndex >= 5 ? ["增长复盘", "留存", "版本节奏", "埋点口径"] : ["增长复盘"],
    updatedAt: Date.now(),
    coveredUntilMs: Math.round(scriptIndex * 6500),
    revision: rev,
    model: "deepseek-chat (mock)",
    error: null,
    promptTokens: 420 * rev,
    completionTokens: 180 * rev,
    calls: rev,
  };
}

function sessionInfoFrom(s: MockSession): SessionInfo {
  return { ...s.info, durationMs: s.accumulatedMs + (Date.now() - s.startedAt) };
}

function emitUpdated(s: MockSession) {
  emit(EV.sessionUpdated, { session: sessionInfoFrom(s) } satisfies SessionUpdatedEvent);
}

function clearTimers(s: MockSession) {
  for (const t of s.timers) {
    window.clearInterval(t);
    window.clearTimeout(t);
  }
  s.timers = [];
}

function startMockSession(req: StartSessionRequest): SessionInfo {
  if (active) stopMockSession();

  const id = `mock-${Date.now()}`;
  const now = Date.now();
  const info: SessionInfo = {
    id,
    title:
      req.title?.trim() ||
      `会议 ${new Date(now).toLocaleString("zh-CN", { hour12: false }).slice(5, 16)}`,
    status: "recording",
    createdAt: now,
    durationMs: 0,
    config: {
      modelId: asrServiceLabel(settings),
      enableMic: req.enableMic,
      enableLoopback: req.enableLoopback,
      micLabel: req.micDeviceId
        ? (sourceById(req.micDeviceId)?.label ?? req.micDeviceId)
        : (listSources().find((s) => s.kind === "microphone")?.label ?? null),
      loopbackLabel: req.loopbackDeviceId
        ? (sourceById(req.loopbackDeviceId)?.label ?? req.loopbackDeviceId)
        : (listSources().find((s) => s.kind === "loopback")?.label ?? null),
    },
    error: null,
  };

  const s: MockSession = {
    info,
    segments: [],
    summary: emptySummary(),
    partial: null,
    scriptIndex: 0,
    charsInScript: 0,
    stats: emptyStats(),
    timers: [],
    startedAt: now,
    accumulatedMs: 0,
  };
  active = s;
  sessionHistory = [
    { id, title: info.title, status: "recording", createdAt: now, durationMs: 0, segments: 0, chars: 0, hasSummary: false, audioPath: null },
    ...sessionHistory.filter((x) => x.id !== id),
  ];

  // App 不含本地模型，启动阶段只是打开音频设备 + 与识别服务握手，没有下载/加载进度
  emit(EV.state, {
    sessionId: id,
    state: "starting",
    message: "正在准备音频设备…",
  } satisfies AsrStateEvent);

  const startTimer = window.setTimeout(() => {
    if (s.info.status !== "recording") return;
    emit(EV.state, {
      sessionId: id,
      state: "listening",
      message: null,
    } satisfies AsrStateEvent);
    startStreaming(s);
  }, 420);
  s.timers.push(startTimer);
  return info;
}

function startStreaming(s: MockSession) {
  const id = s.info.id;

  // 电平表 10Hz
  s.timers.push(
    window.setInterval(() => {
      if (s.info.status !== "recording") return;
      s.stats.audioMs += 100;
      s.stats.rtf = 0.28 + Math.random() * 0.1;
      s.stats.latencyMs = 380 + Math.round(Math.random() * 160);
      const speechish = s.partial !== null || Math.random() > 0.35;
      const mk = (base: number) => (speechish ? base * (0.45 + Math.random() * 0.55) : Math.random() * 0.06);
      emit(EV.level, {
        sessionId: id,
        levels: [
          { sourceId: "mic:default", kind: "microphone", peak: mk(0.9), rms: mk(0.5) },
          { sourceId: "loopback:default", kind: "loopback", peak: mk(0.7), rms: mk(0.35) },
        ],
        stats: s.stats,
        durationMs: s.accumulatedMs + (Date.now() - s.startedAt),
      } satisfies AsrLevelEvent);
      // 每 3 秒同步一次会话元信息（真实后端也是节流上报）
      if (Math.round(s.stats.audioMs) % 3000 === 0) emitUpdated(s);
    }, 100),
  );

  // 说话 → 逐字增长（partial）
  s.timers.push(
    window.setInterval(() => {
      if (s.info.status !== "recording") return;
      const line = SCRIPT[s.scriptIndex % SCRIPT.length];
      if (s.charsInScript >= line.text.length) return;
      s.charsInScript = Math.min(line.text.length, s.charsInScript + 2 + Math.floor(Math.random() * 3));
      const committedLen = Math.max(0, s.charsInScript - 4);
      const startMs = s.segments.length === 0 ? 0 : s.segments[s.segments.length - 1].endMs + 300;
      s.partial = {
        sessionId: id,
        committed: line.text.slice(0, committedLen),
        tentative: line.text.slice(committedLen, s.charsInScript),
        startMs,
      };
      emit(EV.partial, s.partial);
      emit(EV.state, {
        sessionId: id,
        state: "speech",
        message: null,
      } satisfies AsrStateEvent);
    }, 260),
  );

  // 定稿成 segment（按句）
  s.timers.push(
    window.setInterval(() => {
      if (s.info.status !== "recording") return;
      const line = SCRIPT[s.scriptIndex % SCRIPT.length];
      if (s.charsInScript < line.text.length) return;

      const prevEnd = s.segments.length ? s.segments[s.segments.length - 1].endMs : 0;
      const startMs = prevEnd + 300;
      const endMs = startMs + 1800 + Math.round(line.text.length * 145);
      const seg: TranscriptSegment = {
        id: nextId(),
        text: line.text,
        startMs,
        endMs,
        confidence: 0.88 + Math.random() * 0.1,
        suspect: line.suspect ?? null,
      };
      s.segments.push(seg);
      s.stats.segments = s.segments.length;
      s.stats.chars += line.text.length;
      s.stats.speechMs += endMs - startMs;
      s.partial = null;
      emit(EV.segment, { sessionId: id, segment: seg } satisfies AsrSegmentEvent);
      emit(EV.state, {
        sessionId: id,
        state: "listening",
        message: null,
      } satisfies AsrStateEvent);

      s.scriptIndex += 1;
      s.charsInScript = 0;

      const item = sessionHistory.find((x) => x.id === id);
      if (item) {
        item.segments = s.segments.length;
        item.chars = s.stats.chars;
        item.hasSummary = s.summary.calls > 0;
      }
    }, 1400),
  );

  // AI 周期总结
  s.timers.push(
    window.setInterval(() => {
      if (s.info.status !== "recording") return;
      if (s.segments.length === 0) return;
      if (!settings.ai.enabled || !settings.ai.autoSummary) return;
      emit(EV.aiStatus, {
        sessionId: id,
        state: "thinking",
        message: null,
        calls: s.summary.calls + 1,
      } satisfies AiStatusEvent);
      window.setTimeout(() => {
        const recent = s.segments.slice(-2).map((x) => x.text).join("");
        s.summary = makeSummary(
          s.summary.calls + 1,
          s.scriptIndex,
          recent ? `刚刚在讨论：${recent.slice(0, 40)}…` : "",
        );
        emit(EV.summary, { sessionId: id, summary: s.summary } satisfies AiSummaryEvent);
        emit(EV.aiStatus, { sessionId: id, state: "idle", message: null, calls: s.summary.calls } satisfies AiStatusEvent);
      }, 900);
    }, 7000),
  );
}

function stopMockSession(): SessionDetail {
  const s = active;
  if (!s) throw new Error("当前没有进行中的会话");
  clearTimers(s);
  s.accumulatedMs += Date.now() - s.startedAt;
  s.info.status = "finished";
  s.partial = null;
  const item = sessionHistory.find((x) => x.id === s.info.id);
  if (item) {
    item.status = "finished";
    item.durationMs = s.accumulatedMs;
  }
  emit(EV.state, {
    sessionId: s.info.id,
    state: "idle",
    message: null,
  } satisfies AsrStateEvent);
  const detail = detailFrom(s);
  active = null;
  return detail;
}

function detailFrom(s: MockSession): SessionDetail {
  return {
    ...sessionInfoFrom(s),
    segments: s.segments,
    summary: s.summary,
    reportMd: null,
    audioPath: "/mock/data/recordings/meeting.wav",
    stats: s.stats,
  };
}

/* ------------------------------- 命令分发 ------------------------------- */

function ok<T>(v: T): Promise<T> {
  return Promise.resolve(v);
}

function report(scope: string, message: string) {
  emit(EV.error, { scope, message } satisfies AppErrorEvent);
}

export async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const a = (args ?? {}) as Record<string, never>;
  switch (cmd) {
    /* ---- 设置 ---- */
    case "get_settings":
      return ok(settings as unknown as T);
    case "save_settings": {
      settings = a["settings"] as unknown as Settings;
      // 浏览器 mock 没有配置文件：把「向导已完成」这个标记持久化到 localStorage，
      // 这样刷新页面后不会再弹向导（真实后端写在设置文件里）。
      writeOnboardingFlag(settings.general.onboardingCompleted);
      return ok(settings as unknown as T);
    }
    case "reset_settings":
      settings = defaultSettings();
      return ok(settings as unknown as T);

    /* ---- AI ---- */
    case "list_ai_presets":
      return ok(PRESETS as unknown as T);
    case "test_ai_connection": {
      const p = a["provider"] as unknown as AiProvider;
      await new Promise((r) => setTimeout(r, 700));
      if (!p.baseUrl) {
        return ok({
          ok: false,
          latencyMs: 0,
          model: p.model,
          sample: "",
          error: "请先填写 Base URL",
          models: [],
        } as unknown as T);
      }
      return ok({
        ok: true,
        latencyMs: 420,
        model: p.model,
        sample: "连接正常（mock 响应）",
        error: null,
        models: ["deepseek-chat", "deepseek-reasoner"],
      } as unknown as T);
    }
    case "summarize_now":
    case "generate_report": {
      await new Promise((r) => setTimeout(r, 800));
      if (cmd === "generate_report") {
        const s = active;
        const segs = s ? s.segments : [];
        return ok(
          [
            "# 会议纪要",
            "",
            "## 一、会议主题",
            "季度复盘与下季度规划",
            "",
            "## 二、核心结论",
            "- 三季度营收环比增长 18%，企业版订阅为主要驱动",
            "- 下季度目标：环比 +15%，留存优先",
            "",
            "## 三、讨论要点",
            ...segs.slice(0, 6).map((x) => `- [${(x.startMs / 1000).toFixed(1)}s] ${x.text}`),
            "",
            "## 四、待办事项",
            "| 事项 | 负责人 | 时间 |",
            "| --- | --- | --- |",
            "| 完成引导流程重构 | 对方 | 10/28 |",
            "",
            "（mock 生成，非真实模型输出）",
          ].join("\n") as unknown as T,
        );
      }
      return ok(undefined as unknown as T);
    }

    /* ---- 语音识别服务 ---- */
    case "list_asr_presets":
      return ok(ASR_PRESETS as unknown as T);
    case "test_asr_connection": {
      const p = a["provider"] as unknown as AsrProvider;
      const model = (p.model ?? "").trim();
      // 与 Rust 侧 AsrClient::transcribe 的前置校验保持一致
      if (!(p.baseUrl ?? "").trim() || (p.baseUrl ?? "").trim() === "https://") {
        return ok({
          ok: false,
          latencyMs: 0,
          model,
          sample: "",
          error: "尚未配置语音识别服务的 Base URL",
        } satisfies AsrConnectionTestResult as unknown as T);
      }
      if (!model) {
        return ok({
          ok: false,
          latencyMs: 0,
          model,
          sample: "",
          error: "尚未配置语音识别模型名",
        } satisfies AsrConnectionTestResult as unknown as T);
      }
      const started = Date.now();
      // 真实实现会往服务发 0.6 秒静音，这里只模拟往返耗时
      await new Promise((r) => setTimeout(r, 600));
      return ok({
        ok: true,
        latencyMs: Date.now() - started,
        model,
        sample: "（静音测试，返回空文本属正常）",
        error: null,
      } satisfies AsrConnectionTestResult as unknown as T);
    }

    /* ---- 设备 ---- */
    case "list_audio_sources":
      return ok(listSources() as unknown as T);

    /* ---- 会话 ---- */
    case "start_session":
      return ok(startMockSession(a["req"] as unknown as StartSessionRequest) as unknown as T);
    case "pause_session": {
      if (!active) throw new Error("当前没有进行中的会话");
      active.accumulatedMs += Date.now() - active.startedAt;
      active.info.status = "paused";
      emit(EV.state, {
        sessionId: active.info.id,
        state: "paused",
        message: null,
      } satisfies AsrStateEvent);
      return ok(sessionInfoFrom(active) as unknown as T);
    }
    case "resume_session": {
      if (!active) throw new Error("当前没有进行中的会话");
      active.info.status = "recording";
      active.startedAt = Date.now();
      emit(EV.state, {
        sessionId: active.info.id,
        state: "listening",
        message: null,
      } satisfies AsrStateEvent);
      return ok(sessionInfoFrom(active) as unknown as T);
    }
    case "stop_session":
      return ok(stopMockSession() as unknown as T);
    case "active_session":
      return ok((active ? sessionInfoFrom(active) : null) as unknown as T);
    case "get_session": {
      const id = a["id"] as unknown as string;
      if (active && active.info.id === id) return ok(detailFrom(active) as unknown as T);
      const item = sessionHistory.find((x) => x.id === id);
      if (!item) throw new Error("会话不存在");
      return ok({
        id: item.id,
        title: item.title,
        status: item.status,
        createdAt: item.createdAt,
        durationMs: item.durationMs,
        config: {
          modelId: asrServiceLabel(settings),
          enableMic: true,
          enableLoopback: true,
          micLabel: listSources().find((s) => s.kind === "microphone")?.label ?? null,
          loopbackLabel: listSources().find((s) => s.kind === "loopback")?.label ?? null,
        },
        error: null,
        segments: [],
        summary: emptySummary(),
        reportMd: null,
        audioPath: null,
        stats: emptyStats(),
      } as unknown as T);
    }
    case "list_sessions":
      return ok(sessionHistory as unknown as T);
    case "delete_session": {
      const id = a["id"] as unknown as string;
      sessionHistory = sessionHistory.filter((x) => x.id !== id);
      return ok(undefined as unknown as T);
    }
    case "rename_session": {
      const id = a["id"] as unknown as string;
      const title = a["title"] as unknown as string;
      if (active && active.info.id === id) {
        active.info.title = title;
        emitUpdated(active);
        return ok(sessionInfoFrom(active) as unknown as T);
      }
      const item = sessionHistory.find((x) => x.id === id);
      if (item) item.title = title;
      return ok({ ...(item ?? { id, title }) } as unknown as T);
    }
    case "export_session": {
      const format = a["format"] as unknown as ExportFormat;
      void format;
      await new Promise((r) => setTimeout(r, 300));
      const target = (a["path"] as unknown as string) || "/mock/export.md";
      return target as unknown as T;
    }
    case "transcribe_file": {
      const info = startMockSession({ enableMic: false, enableLoopback: false });
      info.config.micLabel = null;
      info.config.loopbackLabel = null;
      info.title = "导入音频转写（mock）";
      return ok(info as unknown as T);
    }

    /* ---- 应用 ---- */
    case "get_app_info":
      return ok(APP_INFO as unknown as T);
    case "open_path":
      return ok(undefined as unknown as T);
    case "open_url": {
      const url = String(a["url"] ?? "").trim();
      // 与 Rust 侧 open_url 保持一致：只放行 http/https
      if (!url.startsWith("https://") && !url.startsWith("http://")) {
        throw new Error("只允许打开 http/https 链接");
      }
      openedUrls.push(url);
      window.__MEETING_HEAR_OPENED_URLS__ = [...openedUrls];
      return ok(undefined as unknown as T);
    }

    default:
      report("mock", `mock 后端未实现命令：${cmd}`);
      throw new Error(`mock 后端未实现命令：${cmd}`);
  }
}
