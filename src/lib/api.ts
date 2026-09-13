/**
 * 后端调用封装。
 *
 * 在 Tauri 里走真正的 `invoke`；在浏览器里（`pnpm dev` 直接打开）
 * 自动切换到 `mock.ts`，这样 UI 可以脱离 Rust 侧独立开发与自动化测试。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AiPreset,
  AiProvider,
  AppInfo,
  AsrConnectionTestResult,
  AsrPreset,
  AsrProvider,
  AudioSourceInfo,
  ConnectionTestResult,
  ExportFormat,
  SessionDetail,
  SessionInfo,
  SessionListItem,
  Settings,
  StartSessionRequest,
} from "./contract";
import { mockInvoke, mockListen } from "./mock";

export const isTauri: boolean =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as object);

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) return mockInvoke<T>(cmd, args);
  return invoke<T>(cmd, args);
}

/** 订阅后端事件；返回取消订阅函数 */
export async function subscribe<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!isTauri) return mockListen(event, handler as (p: unknown) => void);
  return listen<T>(event, (e) => handler(e.payload));
}

export const api = {
  /* ---------------- 设置 ---------------- */
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  resetSettings: () => call<Settings>("reset_settings"),

  /* ---------------- AI ---------------- */
  listAiPresets: () => call<AiPreset[]>("list_ai_presets"),
  testAiConnection: (provider: AiProvider) =>
    call<ConnectionTestResult>("test_ai_connection", { provider }),
  summarizeNow: (sessionId: string) => call<void>("summarize_now", { sessionId }),
  generateReport: (sessionId: string) => call<string>("generate_report", { sessionId }),

  /* ---------------- 语音识别服务 ---------------- */
  listAsrPresets: () => call<AsrPreset[]>("list_asr_presets"),
  testAsrConnection: (provider: AsrProvider) =>
    call<AsrConnectionTestResult>("test_asr_connection", { provider }),

  /* ---------------- 音频设备 ---------------- */
  listAudioSources: () => call<AudioSourceInfo[]>("list_audio_sources"),

  /* ---------------- 会话 ---------------- */
  startSession: (req: StartSessionRequest) => call<SessionInfo>("start_session", { req }),
  pauseSession: () => call<SessionInfo>("pause_session"),
  resumeSession: () => call<SessionInfo>("resume_session"),
  stopSession: () => call<SessionDetail>("stop_session"),
  activeSession: () => call<SessionInfo | null>("active_session"),
  getSession: (id: string) => call<SessionDetail>("get_session", { id }),
  listSessions: () => call<SessionListItem[]>("list_sessions"),
  deleteSession: (id: string) => call<void>("delete_session", { id }),
  renameSession: (id: string, title: string) => call<SessionInfo>("rename_session", { id, title }),
  exportSession: (id: string, format: ExportFormat, path: string) =>
    call<string>("export_session", { id, format, path }),
  transcribeFile: (path: string, language?: string | null) =>
    call<SessionInfo>("transcribe_file", { path, language: language ?? null }),

  /* ---------------- 应用 ---------------- */
  getAppInfo: () => call<AppInfo>("get_app_info"),
  openPath: (path: string) => call<void>("open_path", { path }),
};
