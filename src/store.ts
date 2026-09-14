/**
 * 全局状态（zustand）。
 *
 * 性能分层是这个文件最重要的设计：
 *   useStore        —— 低频状态：会话、已定稿转写、AI 纪要、弹窗、toast
 *   useLevelStore   —— 10Hz（EV.level）：电平、RTF/延迟/字数、计时器
 *   usePartialStore —— 高频（EV.partial）：正在说的那一句
 *
 * 这样 10Hz 的电平事件只让电平条 + 指标条 + 计时器重渲染，
 * 转写列表与纪要面板完全不受影响。
 * （识别服务商配置属于低频的设置草稿态，直接放在设置弹窗的本地 state 里。）
 */
import { create } from "zustand";
import {
  emptyStats,
  emptySummary,
  type AiStatusEvent,
  type AppInfo,
  type AsrLevelEvent,
  type AsrPartialEvent,
  type AsrStateKind,
  type AudioSourceInfo,
  type SessionDetail,
  type SessionInfo,
  type SessionStats,
  type Settings,
  type SummaryState,
  type TranscriptSegment,
} from "./lib/contract";

/* ------------------------------------------------------------------ 工具 */

export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object") {
    const m = (e as { message?: unknown }).message;
    if (typeof m === "string") return m;
  }
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/* ------------------------------------------------------------------ 主 store */

export type ViewKind = "recording" | "detail";
export type SettingsTab = "ai" | "asr" | "audio" | "vad" | "general";
export type ToastKind = "error" | "info" | "success";

export interface Toast {
  id: number;
  kind: ToastKind;
  scope: string;
  message: string;
}

export interface AppState {
  ready: boolean;
  view: ViewKind;
  detailId: string | null;

  settings: Settings | null;
  appInfo: AppInfo | null;
  audioSources: AudioSourceInfo[];

  session: SessionInfo | null;
  /** 会话还没开始时的标题草稿 */
  pendingTitle: string;
  asrState: AsrStateKind;
  asrMessage: string | null;

  segments: TranscriptSegment[];
  summary: SummaryState;
  aiStatus: AiStatusEvent["state"];
  aiMessage: string | null;

  settingsOpen: boolean;
  settingsTab: SettingsTab;
  toasts: Toast[];

  /** 首次运行配置向导是否打开（settings.general.onboardingCompleted === false 时为 true） */
  onboardingOpen: boolean;
  /** 左侧历史栏：用户偏好（记忆在 localStorage） */
  sidebarCollapsed: boolean;
  /** 左侧历史栏：窗口过窄（< 1100px）时的自动折叠 */
  sidebarNarrow: boolean;

  /** 停止录音后由 ai.finalReportOnStop 自动生成的完整纪要 */
  finalReport: string | null;
  /** 最近一次结束的会话 id（用于「查看详情」） */
  lastSessionId: string | null;
  /** 最近一次停止录音返回的完整详情（本地缓存，详情页可立刻展示） */
  lastDetail: SessionDetail | null;
  summarizing: boolean;

  setReady: (v: boolean) => void;
  setView: (view: ViewKind, detailId?: string | null) => void;
  setSettings: (s: Settings) => void;
  setAppInfo: (i: AppInfo) => void;
  setAudioSources: (s: AudioSourceInfo[]) => void;
  setSession: (s: SessionInfo | null) => void;
  patchSession: (patch: Partial<SessionInfo>) => void;
  setPendingTitle: (t: string) => void;
  setAsrState: (state: AsrStateKind, message: string | null) => void;
  pushSegment: (seg: TranscriptSegment) => void;
  setSegments: (segs: TranscriptSegment[]) => void;
  setSummary: (s: SummaryState) => void;
  setAiStatus: (state: AiStatusEvent["state"], message: string | null) => void;
  openSettings: (tab?: SettingsTab) => void;
  closeSettings: () => void;
  setSettingsTab: (tab: SettingsTab) => void;
  openOnboarding: () => void;
  closeOnboarding: () => void;
  setSidebarCollapsed: (v: boolean) => void;
  toggleSidebar: () => void;
  setSidebarNarrow: (v: boolean) => void;
  toast: (kind: ToastKind, scope: string, message: string) => void;
  dismissToast: (id: number) => void;
  resetSessionState: () => void;
  setFinalReport: (md: string | null) => void;
  setLastSessionId: (id: string | null) => void;
  setLastDetail: (d: SessionDetail | null) => void;
  setSummarizing: (v: boolean) => void;
}

let toastSeq = 0;

/* ------------------------------------------------------- 左侧历史栏折叠状态 */

const SIDEBAR_KEY = "meeting-hear:sidebar-collapsed";
/** 窗口窄于该宽度时自动折叠成窄条 */
export const SIDEBAR_NARROW_PX = 1100;

function readSidebarCollapsed(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_KEY) === "1";
  } catch {
    return false;
  }
}

function writeSidebarCollapsed(v: boolean) {
  try {
    localStorage.setItem(SIDEBAR_KEY, v ? "1" : "0");
  } catch {
    /* 忽略写入失败 */
  }
}

export const useStore = create<AppState>((set, get) => ({
  ready: false,
  view: "recording",
  detailId: null,

  settings: null,
  appInfo: null,
  audioSources: [],

  session: null,
  pendingTitle: "",
  asrState: "idle",
  asrMessage: null,

  segments: [],
  summary: emptySummary(),
  aiStatus: "idle",
  aiMessage: null,

  settingsOpen: false,
  settingsTab: "ai",
  toasts: [],
  onboardingOpen: false,
  sidebarCollapsed: readSidebarCollapsed(),
  sidebarNarrow: false,
  finalReport: null,
  lastSessionId: null,
  lastDetail: null,
  summarizing: false,

  setReady: (v) => set({ ready: v }),
  setView: (view, detailId = null) => set({ view, detailId }),
  setSettings: (settings) => set({ settings }),
  setAppInfo: (appInfo) => set({ appInfo }),
  setAudioSources: (audioSources) => set({ audioSources }),
  setSession: (session) => set({ session }),
  patchSession: (patch) =>
    set((s) => (s.session ? { session: { ...s.session, ...patch } } : {})),
  setPendingTitle: (pendingTitle) => set({ pendingTitle }),
  setAsrState: (asrState, asrMessage) => set({ asrState, asrMessage }),
  pushSegment: (seg) => set((s) => ({ segments: [...s.segments, seg] })),
  setSegments: (segments) => set({ segments }),
  setSummary: (summary) => set({ summary }),
  setAiStatus: (aiStatus, aiMessage) => set({ aiStatus, aiMessage }),
  openSettings: (tab) => set({ settingsOpen: true, settingsTab: tab ?? get().settingsTab }),
  closeSettings: () => set({ settingsOpen: false }),
  setSettingsTab: (settingsTab) => set({ settingsTab }),

  openOnboarding: () => set({ onboardingOpen: true, settingsOpen: false }),
  closeOnboarding: () => set({ onboardingOpen: false }),
  setSidebarCollapsed: (v) => {
    writeSidebarCollapsed(v);
    set({ sidebarCollapsed: v });
  },
  toggleSidebar: () => {
    const next = !get().sidebarCollapsed;
    writeSidebarCollapsed(next);
    set({ sidebarCollapsed: next });
  },
  setSidebarNarrow: (v) => set({ sidebarNarrow: v }),

  toast: (kind, scope, message) => {
    const id = ++toastSeq;
    set((s) => ({ toasts: [...s.toasts, { id, kind, scope, message }].slice(-4) }));
    const ttl = kind === "error" ? 9000 : 4000;
    window.setTimeout(() => get().dismissToast(id), ttl);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  resetSessionState: () =>
    set({
      segments: [],
      summary: emptySummary(),
      aiStatus: "idle",
      aiMessage: null,
      asrState: "idle",
      asrMessage: null,
      finalReport: null,
    }),

  setFinalReport: (finalReport) => set({ finalReport }),
  setLastSessionId: (lastSessionId) => set({ lastSessionId }),
  setLastDetail: (lastDetail) => set({ lastDetail }),
  setSummarizing: (summarizing) => set({ summarizing }),
}));

/** 统一包裹 api 调用：失败弹 toast 并返回 undefined，绝不把异常抛到事件处理器里 */
export async function guard<T>(scope: string, fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    useStore.getState().toast("error", scope, errText(e));
    return undefined;
  }
}

/* ------------------------------------------------------------------ 电平 / 指标（10Hz） */

interface LevelState {
  mic: number;
  micRms: number;
  /** 峰值保持线，用于画“峰值刻度” */
  micHold: number;
  loop: number;
  loopRms: number;
  loopHold: number;
  /** 各声源原始电平，按 sourceId 索引（设备名变化时用） */
  byId: Record<string, number>;
  stats: SessionStats;
  durationMs: number;
  apply: (e: AsrLevelEvent) => void;
  reset: () => void;
}

const HOLD_DECAY = 0.012;

export const useLevelStore = create<LevelState>((set) => ({
  mic: 0,
  micRms: 0,
  micHold: 0,
  loop: 0,
  loopRms: 0,
  loopHold: 0,
  byId: {},
  stats: emptyStats(),
  durationMs: 0,
  apply: (e) =>
    set((s) => {
      let mic = 0;
      let micRms = 0;
      let loop = 0;
      let loopRms = 0;
      const byId: Record<string, number> = {};
      for (const l of e.levels) {
        byId[l.sourceId] = l.peak;
        if (l.kind === "microphone") {
          mic = Math.max(mic, l.peak);
          micRms = Math.max(micRms, l.rms);
        } else {
          loop = Math.max(loop, l.peak);
          loopRms = Math.max(loopRms, l.rms);
        }
      }
      return {
        mic,
        micRms,
        loop,
        loopRms,
        micHold: Math.max(mic, s.micHold - HOLD_DECAY),
        loopHold: Math.max(loop, s.loopHold - HOLD_DECAY),
        byId,
        stats: e.stats,
        durationMs: e.durationMs,
      };
    }),
  reset: () =>
    set({
      mic: 0,
      micRms: 0,
      micHold: 0,
      loop: 0,
      loopRms: 0,
      loopHold: 0,
      byId: {},
      stats: emptyStats(),
      durationMs: 0,
    }),
}));

/* ------------------------------------------------------------------ 未定稿文本（高频） */

interface PartialState {
  partial: AsrPartialEvent | null;
  apply: (p: AsrPartialEvent | null) => void;
}

export const usePartialStore = create<PartialState>((set) => ({
  partial: null,
  apply: (partial) => set({ partial }),
}));
