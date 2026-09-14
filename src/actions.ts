/**
 * 会话相关的动作（开始/暂停/停止/导入/立即总结/导出）。
 * 所有 api 调用都经过 guard()，失败会弹 toast，不会把异常抛到 UI 事件里。
 */
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, isTauri } from "./lib/api";
import { emptySummary, type ExportFormat, type Settings, type StartSessionRequest } from "./lib/contract";
import { asrReadiness } from "./lib/settings";
import { guard, useLevelStore, usePartialStore, useStore } from "./store";

/**
 * 识别服务未配置时给出明确引导：弹错误说明 + 直接把设置打开到「语音识别」页，
 * 避免用户点了「开始录音」却什么都没发生。
 */
function requireAsrService(scope: string): boolean {
  const st = useStore.getState();
  const readiness = asrReadiness(st.settings);
  if (readiness.ready) return true;
  st.toast("error", scope, `${readiness.reason} —— 已为你打开「设置 → 语音识别」，请先配置识别服务商`);
  st.openSettings("asr");
  return false;
}

function currentRequest(): StartSessionRequest | null {
  const st = useStore.getState();
  const settings = st.settings;
  if (!settings) {
    st.toast("error", "开始录音", "设置尚未加载完成，请稍后再试");
    return null;
  }
  if (!settings.audio.enableMic && !settings.audio.enableLoopback) {
    st.toast("error", "开始录音", "请先在「设置 → 音频设备」里开启麦克风或系统内录");
    return null;
  }
  if (!requireAsrService("开始录音")) return null;
  return {
    title: st.pendingTitle.trim() || null,
    enableMic: settings.audio.enableMic,
    enableLoopback: settings.audio.enableLoopback,
    micDeviceId: settings.audio.micDeviceId,
    loopbackDeviceId: settings.audio.loopbackDeviceId,
    saveAudio: settings.audio.saveAudio,
  };
}

export async function startRecording(): Promise<void> {
  const st = useStore.getState();
  if (st.session) return;
  // 首次配置向导期间不允许开始录音：识别服务与 AI 接口都还没确认，
  // 这时候开录只会每个请求都失败。
  if (st.onboardingOpen) return;
  const req = currentRequest();
  if (!req) return;

  const info = await guard("开始录音", () => api.startSession(req));
  if (!info) return;

  st.setSession(info);
  st.setSegments([]);
  st.setSummary(emptySummary());
  st.setAiStatus("idle", null);
  st.setFinalReport(null);
  st.setLastSessionId(null);
  useLevelStore.getState().reset();
  usePartialStore.getState().apply(null);
  st.setAsrState("starting", "正在准备音频设备…");
  st.toast("success", "开始录音", "正在连接识别服务，请稍候…");
}

export async function pauseOrResume(): Promise<void> {
  const st = useStore.getState();
  if (!st.session) return;
  const paused = st.session.status === "paused";
  const info = await guard(paused ? "继续录音" : "暂停录音", () =>
    paused ? api.resumeSession() : api.pauseSession(),
  );
  if (info) st.setSession(info);
}

export async function stopRecording(): Promise<void> {
  const st = useStore.getState();
  if (!st.session) return;
  st.setAsrState("stopping", "正在收尾…");

  const detail = await guard("停止录音", () => api.stopSession());
  useLevelStore.getState().reset();
  usePartialStore.getState().apply(null);
  if (!detail) {
    st.setSession(null);
    st.setAsrState("error", "停止录音失败");
    return;
  }

  st.setSession(null);
  st.setSegments(detail.segments);
  st.setSummary(detail.summary);
  st.setLastSessionId(detail.id);
  st.setLastDetail(detail);
  st.setAsrState("idle", null);

  const finalReportOnStop = st.settings?.ai.finalReportOnStop ?? false;
  if (finalReportOnStop) {
    st.toast("info", "完整纪要", "正在生成完整会议纪要…");
    const md = await guard("生成完整纪要", () => api.generateReport(detail.id));
    if (md) {
      st.setFinalReport(md);
      st.toast("success", "完整纪要", "会议纪要已生成");
    }
  }
}

export async function importAudioFile(): Promise<void> {
  const st = useStore.getState();
  if (!requireAsrService("导入音频")) return;
  let path = "mock.wav";

  if (isTauri) {
    const picked = await guard("选择音频文件", () =>
      open({
        multiple: false,
        directory: false,
        title: "选择要转写的音频文件",
        filters: [{ name: "音频", extensions: ["wav", "mp3", "m4a", "flac", "ogg", "opus", "aac"] }],
      }),
    );
    if (typeof picked !== "string" || !picked) return;
    path = picked;
  }

  const info = await guard("导入音频", () => api.transcribeFile(path));
  if (!info) return;

  st.setSession(info);
  st.setSegments([]);
  st.setSummary(emptySummary());
  st.setFinalReport(null);
  st.setLastSessionId(null);
  st.setAsrState("starting", "正在解码音频…");
  useLevelStore.getState().reset();
  usePartialStore.getState().apply(null);
  st.toast("success", "导入音频", `开始转写：${path}`);
}

/**
 * 「设置 → 通用 → 重新运行配置向导」：
 * 把 onboardingCompleted 设回 false 并立刻保存，然后重新打开向导。
 */
export async function restartOnboarding(): Promise<void> {
  const st = useStore.getState();
  const current = st.settings;
  if (!current) {
    st.toast("error", "配置向导", "设置尚未加载完成，请稍后再试");
    return;
  }
  const next: Settings = {
    ...current,
    general: { ...current.general, onboardingCompleted: false },
  };
  const saved = await guard("重新运行配置向导", () => api.saveSettings(next));
  st.setSettings(saved ?? next);
  st.closeSettings();
  st.openOnboarding();
  st.toast("info", "配置向导", "已重新打开首次配置向导");
}

export async function summarizeNow(sessionId: string | null): Promise<void> {
  const st = useStore.getState();
  if (!sessionId) {
    st.toast("info", "立即总结", "当前没有进行中的会话");
    return;
  }
  st.setSummarizing(true);
  st.setAiStatus("thinking", null);
  const r = await guard("立即总结", () => api.summarizeNow(sessionId));
  if (r === undefined) {
    st.setSummarizing(false);
    st.setAiStatus("idle", null);
    return;
  }
  st.toast("info", "立即总结", "已请求 AI 总结，稍后更新");
  window.setTimeout(() => useStore.getState().setSummarizing(false), 1500);
}

export async function generateReport(sessionId: string): Promise<string | null> {
  const st = useStore.getState();
  const md = await guard("生成完整纪要", () => api.generateReport(sessionId));
  if (md !== undefined) {
    st.toast("success", "完整纪要", "会议纪要已生成");
    return md;
  }
  return null;
}

export async function exportSession(
  sessionId: string,
  format: ExportFormat,
  suggestedName: string,
): Promise<void> {
  const st = useStore.getState();
  const ext = format;
  let target = `/mock/export/${suggestedName || "meeting"}.${ext}`;

  if (isTauri) {
    const picked = await guard("选择导出位置", () =>
      save({
        title: "导出会议记录",
        defaultPath: `${suggestedName || "meeting"}.${ext}`,
        filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
      }),
    );
    if (!picked) return;
    target = picked;
  }

  const done = await guard("导出会议记录", () => api.exportSession(sessionId, format, target));
  if (done) st.toast("success", "导出成功", `${format.toUpperCase()} → ${done}`);
}
