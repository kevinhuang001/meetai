/**
 * MeetingHear 前后端契约（冻结版本）
 *
 * 规则：
 *  - 这里的类型与 Rust 侧 `src-tauri/src/settings.rs`、`events.rs`、`session/mod.rs` 一一对应。
 *  - Rust 结构体统一 `#[serde(rename_all = "camelCase")]`，因此 TS 侧也用 camelCase。
 *  - 改这个文件必须同步改 Rust 侧，反之亦然。
 */

/* ============================================================================
 * 1. 设置
 * ========================================================================== */

export type LanguageCode = "auto" | "zh" | "en" | "ja" | "ko" | "yue" | "de" | "fr" | "es" | "ru";

export interface AsrProvider {
  id: string;
  name: string;
  /** 服务根地址，例如 https://api.groq.com/openai/v1 或 http://localhost:8090 */
  baseUrl: string;
  /** 转写接口路径：/audio/transcriptions（OpenAI 兼容）或 /inference（whisper.cpp server） */
  transcriptionPath: string;
  apiKey: string;
  /** 模型名，例如 whisper-large-v3-turbo / whisper-1 / large-v3 */
  model: string;
  /** 请求格式：json（最兼容）或 verbose_json（能拿到语言） */
  responseFormat: string;
  timeoutSecs: number;
  extraHeaders: [string, string][];
}

export interface AsrSettings {
  enabled: boolean;
  providers: AsrProvider[];
  activeProviderId: string;
  /** 识别语言，auto = 自动检测 */
  language: LanguageCode;
  translateToEnglish: boolean;
  /** 把上一句文本作为 prompt 传给服务，提升人名/术语一致性 */
  contextPrompt: boolean;
  temperature: number;
  /** 边说边出字：对当前这句话做增量识别。会增加 API 调用次数，本地服务建议开启 */
  livePreview: boolean;
  /** 单次请求最多上传多少秒音频 */
  maxChunkSecs: number;
}

export interface VadSettings {
  /** 高于自适应噪声底多少 dB 判定为语音 */
  energyThresholdDb: number;
  /** 最短语音时长，短于此值的片段不触发识别 */
  minSpeechMs: number;
  /** 静音多久判定一句话结束 —— 决定「按句定稿」的时机 */
  minSilenceMs: number;
  /** 单句最长时长，超时强制断句，避免延迟无限增长 */
  maxUtteranceMs: number;
  /** 断句前后保留的音频留白，避免吃字 */
  speechPadMs: number;
}

export interface AiProvider {
  id: string;
  name: string;
  /** OpenAI 兼容 base url，例如 https://api.deepseek.com/v1 */
  baseUrl: string;
  apiKey: string;
  model: string;
  temperature: number;
  maxTokens: number;
  /** 是否发送 response_format={"type":"json_object"}，不支持的服务商会自动降级重试 */
  jsonMode: boolean;
  timeoutSecs: number;
  extraHeaders: [string, string][];
}

export interface AiSettings {
  enabled: boolean;
  providers: AiProvider[];
  activeProviderId: string;
  /** 录音过程中自动滚动总结 */
  autoSummary: boolean;
  /** 自动总结间隔（秒） */
  intervalSecs: number;
  /** 距上次总结新增多少字才值得再总结一次 */
  minNewChars: number;
  /** “刚刚说了什么”面板覆盖的最近时长（秒） */
  liveWindowSecs: number;
  /** 送给模型的转写上下文上限（字符） */
  maxContextChars: number;
  /** 停止录音后自动生成完整会议纪要 */
  finalReportOnStop: boolean;
}

export interface AudioSettings {
  enableMic: boolean;
  micDeviceId: string | null;
  /** 系统内录（Windows WASAPI loopback / macOS BlackHole / Linux Pulse monitor） */
  enableLoopback: boolean;
  loopbackDeviceId: string | null;
  micGain: number;
  loopbackGain: number;
  /** 录制原始音频到 wav，便于回听与重新转写 */
  saveAudio: boolean;
}

export interface GeneralSettings {
  theme: "dark" | "light" | "system";
  autoScroll: boolean;
  fontScale: number;
  /** 数据目录，null = 使用系统默认 app data 目录 */
  dataDir: string | null;
  /** 保存 API Key 到本地配置文件（0600 权限） */
  persistApiKey: boolean;
}

export interface Settings {
  version: number;
  asr: AsrSettings;
  vad: VadSettings;
  ai: AiSettings;
  audio: AudioSettings;
  general: GeneralSettings;
}

/* ============================================================================
 * 2. 语音识别服务
 * ========================================================================== */

export interface AsrPreset {
  id: string;
  name: string;
  baseUrl: string;
  transcriptionPath: string;
  model: string;
  note: string;
  /** 是否需要 api key（本地服务不需要） */
  needsKey: boolean;
  /** 是否本地/自建服务（UI 会提示可以开启「边说边出字」） */
  local: boolean;
  responseFormat: string;
}

export interface AsrConnectionTestResult {
  ok: boolean;
  latencyMs: number;
  model: string;
  /** 服务返回的文本（静音测试时通常为空，属正常） */
  sample: string;
  error: string | null;
}

/* ============================================================================
 * 3. 音频设备
 * ========================================================================== */

export interface AudioSourceInfo {
  id: string;
  kind: "microphone" | "loopback";
  label: string;
  detail: string;
  isDefault: boolean;
  /** false 时 UI 要显示不可用原因 */
  available: boolean;
  note: string | null;
}

/* ============================================================================
 * 4. 转写与纪要
 * ========================================================================== */

/** 说话人归属：混合采集时按各声源能量占比自动判定 */
export type Speaker = "me" | "others" | "mixed" | "unknown";

export interface TranscriptSegment {
  id: number;
  text: string;
  startMs: number;
  endMs: number;
  speaker: Speaker;
  language: string | null;
  confidence: number | null;
}

export interface ActionItem {
  text: string;
  owner: string;
  due: string;
}

export interface SummaryState {
  /** 刚刚说了什么（最近 liveWindowSecs 秒） */
  live: string;
  /** 整场会议 2~4 句总览 */
  overview: string;
  /** markdown 无序列表形式的完整纪要 */
  summary: string;
  keyPoints: string[];
  decisions: string[];
  actionItems: ActionItem[];
  topics: string[];
  updatedAt: number;
  /** 已纳入总结的转写结束位置（毫秒），用于 UI 显示“纪要落后转写多少” */
  coveredUntilMs: number;
  revision: number;
  model: string | null;
  error: string | null;
  /** 摘要累计消耗 */
  promptTokens: number;
  completionTokens: number;
  calls: number;
}

export interface SessionStats {
  /** 已识别的语音总时长 */
  speechMs: number;
  /** 已采集的音频总时长 */
  audioMs: number;
  chars: number;
  /** 实时率：识别耗时 / 音频时长，越小越好 */
  rtf: number;
  /** 从说话到出字的中位延迟（毫秒） */
  latencyMs: number;
  segments: number;
}

export type SessionStatus = "recording" | "paused" | "finished" | "error";

export interface SessionConfig {
  /** 识别服务描述（服务商 · 模型） */
  modelId: string;
  language: string;
  enableMic: boolean;
  enableLoopback: boolean;
  micLabel: string | null;
  loopbackLabel: string | null;
}

export interface SessionInfo {
  id: string;
  title: string;
  status: SessionStatus;
  createdAt: number;
  /** 录音已进行的时长（毫秒，不含暂停） */
  durationMs: number;
  config: SessionConfig;
  /** 检测/指定的识别语言 */
  language: string | null;
  error: string | null;
}

export interface SessionDetail extends SessionInfo {
  segments: TranscriptSegment[];
  summary: SummaryState;
  reportMd: string | null;
  audioPath: string | null;
  stats: SessionStats;
}

export interface SessionListItem {
  id: string;
  title: string;
  status: SessionStatus;
  createdAt: number;
  durationMs: number;
  segments: number;
  chars: number;
  hasSummary: boolean;
  audioPath: string | null;
}

/* ============================================================================
 * 5. 命令入参 / 出参
 * ========================================================================== */

export interface StartSessionRequest {
  title?: string | null;
  enableMic: boolean;
  enableLoopback: boolean;
  micDeviceId?: string | null;
  loopbackDeviceId?: string | null;
  language?: string | null;
  saveAudio?: boolean | null;
}

export interface AiPreset {
  id: string;
  name: string;
  baseUrl: string;
  model: string;
  note: string;
  /** 是否需要 api key（本地 ollama 不需要） */
  needsKey: boolean;
  jsonMode: boolean;
}

export interface ConnectionTestResult {
  ok: boolean;
  latencyMs: number;
  model: string;
  /** 模型返回的样例内容 */
  sample: string;
  error: string | null;
  /** 该服务商可用的模型列表（能拉到才有） */
  models: string[];
}

export interface AppInfo {
  name: string;
  version: string;
  platform: string;
  arch: string;
  dataDir: string;
  sessionsDir: string;
  recordingsDir: string;
  /** 当前生效的识别服务描述 */
  asrService: string;
  /** 断句方式 */
  vadEngine: string;
}

export type ExportFormat = "md" | "txt" | "srt" | "json";

/* ============================================================================
 * 6. 事件
 * ========================================================================== */

export const EV = {
  /** 定稿的一句话 */
  segment: "asr:segment",
  /** 当前正在说的（未定稿，UI 用灰色显示） */
  partial: "asr:partial",
  /** 采集/识别状态 */
  state: "asr:state",
  /** 电平、实时率等高频状态（约 10Hz） */
  level: "asr:level",
  /** AI 总结状态 */
  aiStatus: "ai:status",
  /** AI 纪要更新 */
  summary: "ai:summary",
  /** 会话元信息变化（标题/时长/状态） */
  sessionUpdated: "session:updated",
  /** 全局错误提示 */
  error: "app:error",
} as const;

export type AsrStateKind =
  | "idle"
  | "starting"
  | "listening"
  | "speech"
  | "paused"
  | "stopping"
  | "error";

export interface AsrStateEvent {
  sessionId: string;
  state: AsrStateKind;
  message: string | null;
  /** 实际生效的识别语言（auto 时会回填检测结果） */
  language: string | null;
}

export interface AsrSegmentEvent {
  sessionId: string;
  segment: TranscriptSegment;
}

export interface AsrPartialEvent {
  sessionId: string;
  /** 已由两次假设共同确认的部分 */
  committed: string;
  /** 仍未确认的尾巴 */
  tentative: string;
  startMs: number;
}

export interface AsrLevelEvent {
  sessionId: string;
  /** 各声源电平 0~1，用于画音量条 */
  levels: { sourceId: string; kind: "microphone" | "loopback"; peak: number; rms: number }[];
  stats: SessionStats;
  durationMs: number;
}

export interface AiStatusEvent {
  sessionId: string;
  state: "idle" | "thinking" | "error";
  message: string | null;
  /** thinking 时是第几次调用 */
  calls: number;
}

export interface AiSummaryEvent {
  sessionId: string;
  summary: SummaryState;
}

export interface SessionUpdatedEvent {
  session: SessionInfo;
}

export interface AppErrorEvent {
  scope: string;
  message: string;
}

/* ============================================================================
 * 7. 工具
 * ========================================================================== */

export const LANGUAGE_OPTIONS: { value: LanguageCode; label: string }[] = [
  { value: "auto", label: "自动检测" },
  { value: "zh", label: "中文" },
  { value: "en", label: "English" },
  { value: "yue", label: "粤语" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "de", label: "Deutsch" },
  { value: "fr", label: "Français" },
  { value: "es", label: "Español" },
  { value: "ru", label: "Русский" },
];

export const SPEAKER_LABEL: Record<Speaker, string> = {
  me: "我",
  others: "对方",
  mixed: "双方",
  unknown: "未知",
};

export function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const v = bytes / Math.pow(1024, i);
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

export function emptySummary(): SummaryState {
  return {
    live: "",
    overview: "",
    summary: "",
    keyPoints: [],
    decisions: [],
    actionItems: [],
    topics: [],
    updatedAt: 0,
    coveredUntilMs: 0,
    revision: 0,
    model: null,
    error: null,
    promptTokens: 0,
    completionTokens: 0,
    calls: 0,
  };
}

export function emptyStats(): SessionStats {
  return { speechMs: 0, audioMs: 0, chars: 0, rtf: 0, latencyMs: 0, segments: 0 };
}
