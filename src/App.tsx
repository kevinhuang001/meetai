/** 应用外壳：全局事件订阅、快捷键、主题应用、视图切换 */
import { useEffect } from "react";
import { api, subscribe } from "./lib/api";
import {
  EV,
  type AiStatusEvent,
  type AiSummaryEvent,
  type AppErrorEvent,
  type AsrLevelEvent,
  type AsrPartialEvent,
  type AsrSegmentEvent,
  type AsrStateEvent,
  type SessionUpdatedEvent,
} from "./lib/contract";
import { applyAppearance, watchSystemTheme } from "./lib/appearance";
import { guard, useLevelStore, usePartialStore, useStore } from "./store";
import { startRecording, stopRecording } from "./actions";
import { TopBar } from "./components/TopBar";
import { RecordingView } from "./components/RecordingView";
import { HistoryView } from "./components/HistoryView";
import { SessionDetailView } from "./components/SessionDetailView";
import { ControlBar } from "./components/ControlBar";
import { Toasts } from "./components/Toasts";
import { SettingsDialog } from "./components/settings/SettingsDialog";

async function bootstrap(): Promise<void> {
  const st = useStore.getState();
  const [settings, appInfo, sources, active] = await Promise.all([
    guard("读取设置", () => api.getSettings()),
    guard("读取应用信息", () => api.getAppInfo()),
    guard("读取音频设备", () => api.listAudioSources()),
    guard("读取进行中的会话", () => api.activeSession()),
  ]);

  if (settings) st.setSettings(settings);
  if (appInfo) st.setAppInfo(appInfo);
  if (sources) st.setAudioSources(sources);

  if (active) {
    st.setSession(active);
    st.setAsrState(active.status === "paused" ? "paused" : "listening", null, active.language);
    const detail = await guard("恢复会话内容", () => api.getSession(active.id));
    if (detail) {
      st.setSegments(detail.segments);
      st.setSummary(detail.summary);
    }
  }
  st.setReady(true);
}

export default function App() {
  const ready = useStore((s) => s.ready);
  const view = useStore((s) => s.view);
  const settings = useStore((s) => s.settings);

  /* 启动：拉取设置、音频设备与进行中的会话（识别服务全部走 HTTP，无需本地模型） */
  useEffect(() => {
    void bootstrap();
  }, []);

  /* 全局事件订阅（卸载时逐个退订） */
  useEffect(() => {
    let disposed = false;
    let unsubs: (() => void)[] = [];

    const wire = async () => {
      const list = await Promise.all([
        subscribe<AsrStateEvent>(EV.state, (e) => {
          const st = useStore.getState();
          const language = e.language ?? st.language;
          // mock / 真实后端会在每个 partial 周期都发一次 state，
          // 这里做去重，避免 4Hz 的无效 state 更新把无关组件一起重渲染。
          const same =
            st.asrState === e.state && st.asrMessage === e.message && st.language === language;
          if (!same) st.setAsrState(e.state, e.message, language);
          if (st.session && st.session.id === e.sessionId) {
            const status =
              e.state === "paused"
                ? "paused"
                : e.state === "error"
                  ? "error"
                  : e.state === "idle"
                    ? "finished"
                    : "recording";
            if (st.session.status !== status || st.session.language !== language) {
              st.patchSession({ status, language });
            }
          }
        }),
        subscribe<AsrSegmentEvent>(EV.segment, (e) => {
          useStore.getState().pushSegment(e.segment);
          usePartialStore.getState().apply(null);
        }),
        subscribe<AsrPartialEvent>(EV.partial, (e) => usePartialStore.getState().apply(e)),
        subscribe<AsrLevelEvent>(EV.level, (e) => useLevelStore.getState().apply(e)),
        subscribe<AiStatusEvent>(EV.aiStatus, (e) => {
          const st = useStore.getState();
          st.setAiStatus(e.state, e.message);
          st.setSummarizing(e.state === "thinking");
          if (e.state === "error" && e.message) st.toast("error", "AI 总结", e.message);
        }),
        subscribe<AiSummaryEvent>(EV.summary, (e) => useStore.getState().setSummary(e.summary)),
        subscribe<SessionUpdatedEvent>(EV.sessionUpdated, (e) => {
          const st = useStore.getState();
          if (st.session && st.session.id === e.session.id) st.setSession(e.session);
        }),
        subscribe<AppErrorEvent>(EV.error, (e) => useStore.getState().toast("error", e.scope, e.message)),
      ]);
      if (disposed) {
        for (const u of list) u();
        return;
      }
      unsubs = list;
    };

    void wire();
    return () => {
      disposed = true;
      for (const u of unsubs) u();
      unsubs = [];
    };
  }, []);

  /* 主题与字号 */
  useEffect(() => {
    applyAppearance(settings);
  }, [settings]);

  useEffect(() => {
    if ((settings?.general.theme ?? "dark") !== "system") return;
    return watchSystemTheme(() => applyAppearance(useStore.getState().settings));
  }, [settings?.general.theme]);

  /* 快捷键 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      const st = useStore.getState();

      if (mod && e.key === "Enter") {
        e.preventDefault();
        if (st.session) void stopRecording();
        else void startRecording();
        return;
      }
      if (mod && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        st.setView(st.view === "history" ? "recording" : "history");
        return;
      }
      if (e.key === "Escape" && st.settingsOpen) {
        st.closeSettings();
        applyAppearance(useStore.getState().settings);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="app">
      <TopBar />
      <main className="main">
        {!ready ? (
          <div className="boot">
            <span className="spinner" aria-hidden="true" />
            正在启动…
          </div>
        ) : view === "history" ? (
          <HistoryView />
        ) : view === "detail" ? (
          <SessionDetailView />
        ) : (
          <RecordingView />
        )}
      </main>
      {view === "recording" ? <ControlBar /> : null}
      <Toasts />
      <SettingsDialog />
    </div>
  );
}
