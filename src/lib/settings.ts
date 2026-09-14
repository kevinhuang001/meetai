/** 设置相关的派生判断 */
import type { AiProvider, AsrProvider, Settings } from "./contract";

export interface AiReadiness {
  ready: boolean;
  reason: string;
  provider: AiProvider | null;
}

export function activeProvider(settings: Settings | null): AiProvider | null {
  if (!settings) return null;
  const list = settings.ai.providers;
  if (list.length === 0) return null;
  return list.find((p) => p.id === settings.ai.activeProviderId) ?? list[0] ?? null;
}

export function aiReadiness(settings: Settings | null): AiReadiness {
  if (!settings) return { ready: false, reason: "设置尚未加载", provider: null };
  if (!settings.ai.enabled) {
    return { ready: false, reason: "AI 实时纪要在设置里被关闭了", provider: null };
  }
  const p = activeProvider(settings);
  if (!p) return { ready: false, reason: "还没有添加任何 AI 服务商", provider: null };
  if (!p.baseUrl.trim()) {
    return { ready: false, reason: `服务商「${p.name}」还没有填写 Base URL`, provider: p };
  }
  if (!p.model.trim()) {
    return { ready: false, reason: `服务商「${p.name}」还没有填写模型名`, provider: p };
  }
  return { ready: true, reason: "", provider: p };
}

/* ------------------------------------------------------------------ 语音识别服务 */

export interface AsrReadiness {
  ready: boolean;
  reason: string;
  provider: AsrProvider | null;
}

/** 当前选中的识别服务商（activeProviderId 失配时退回第一个） */
export function activeAsrProvider(settings: Settings | null): AsrProvider | null {
  if (!settings) return null;
  const list = settings.asr.providers;
  if (list.length === 0) return null;
  return list.find((p) => p.id === settings.asr.activeProviderId) ?? list[0] ?? null;
}

/**
 * 能否开始转写。App 不含识别模型，所以「没有可用的识别服务」必须提前拦住并引导用户去设置，
 * 而不是等录音开始后每个请求都失败。
 */
export function asrReadiness(settings: Settings | null): AsrReadiness {
  if (!settings) return { ready: false, reason: "设置尚未加载", provider: null };
  if (!settings.asr.enabled) {
    return { ready: false, reason: "语音识别在设置里被关闭了", provider: null };
  }
  const p = activeAsrProvider(settings);
  if (!p) return { ready: false, reason: "还没有配置任何识别服务商", provider: null };
  if (!p.baseUrl.trim() || p.baseUrl.trim() === "https://") {
    return { ready: false, reason: `识别服务商「${p.name}」还没有填写 Base URL`, provider: p };
  }
  if (!p.model.trim()) {
    return { ready: false, reason: `识别服务商「${p.name}」还没有填写模型名`, provider: p };
  }
  return { ready: true, reason: "", provider: p };
}

/** 「服务商 · 模型」描述，用于指标条、会话详情等只读展示 */
export function asrServiceLabel(settings: Settings | null): string {
  if (!settings) return "—";
  const p = activeAsrProvider(settings);
  if (!p) return "未配置识别服务";
  return p.model.trim() ? `${p.name} · ${p.model}` : p.name;
}

/**
 * 改一项设置并立刻落盘。
 *
 * 主界面上的快捷设置（比如录音时切「会议 / 讲座」模式）不该逼用户
 * 先打开设置对话框再点保存 —— 那样这个开关就没人会用。
 */
export async function patchAndSaveSettings(patch: Partial<Settings>): Promise<Settings | null> {
  const { api } = await import("./api");
  const { useStore } = await import("../store");
  const current = useStore.getState().settings;
  if (!current) return null;
  const next: Settings = { ...current, ...patch };
  try {
    const saved = await api.saveSettings(next);
    useStore.getState().setSettings(saved);
    return saved;
  } catch (e) {
    useStore.getState().toast("error", "保存设置", String(e));
    return null;
  }
}
