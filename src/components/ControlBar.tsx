/** 底栏控制条：电平表 + 实时指标 + 录音控制按钮 */
import { useMemo } from "react";
import { formatDuration } from "../lib/contract";
import { asrServiceLabel } from "../lib/settings";
import { useLevelStore, useStore } from "../store";
import { importAudioFile, pauseOrResume, startRecording, stopRecording, summarizeNow } from "../actions";
import { Button } from "./ui";

function LevelMeter({ kind, label, hint }: { kind: "mic" | "loop"; label: string; hint: string }) {
  const level = useLevelStore((s) => (kind === "mic" ? s.mic : s.loop));
  const hold = useLevelStore((s) => (kind === "mic" ? s.micHold : s.loopHold));
  const pct = Math.round(Math.min(1, Math.max(0, level)) * 100);
  const holdPct = Math.round(Math.min(1, Math.max(0, hold)) * 100);
  const testId = kind === "mic" ? "level-mic" : "level-loop";

  return (
    <div className="level" data-testid={testId}>
      <div className="level-head">
        <span className="level-name">{kind === "mic" ? "麦克风" : "系统声音"}</span>
        <span className="level-device" title={label}>
          {label}
        </span>
      </div>
      <div className="level-track" role="meter" aria-label={`${hint}电平`} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <span className="level-ticks" aria-hidden="true" />
        <div
          className={kind === "mic" ? "level-fill mic" : "level-fill loop"}
          data-testid={`${testId}-fill`}
          style={{ width: `${pct}%` }}
        />
        <div className="level-hold" style={{ left: `${holdPct}%` }} />
      </div>
    </div>
  );
}

function Metrics() {
  const stats = useLevelStore((s) => s.stats);
  const durationMs = useLevelStore((s) => s.durationMs);
  const settings = useStore((s) => s.settings);
  const sessionService = useStore((s) => s.session?.config.modelId ?? null);
  // config.modelId 现在装的是「服务商 · 模型」描述（字段名沿用冻结契约）
  const service = sessionService ?? asrServiceLabel(settings);
  return (
    <div className="metrics" data-testid="metrics">
      <span className="metric">
        <em>识别服务</em>
        <b className="mono svc" data-testid="metric-asr-service" title={service}>
          {service}
        </b>
      </span>
      <span className="metric">
        <em>RTF</em>
        <b className="mono" data-testid="metric-rtf">
          {stats.rtf ? stats.rtf.toFixed(2) : "—"}
        </b>
      </span>
      <span className="metric">
        <em>延迟</em>
        <b className="mono" data-testid="metric-latency">
          {stats.latencyMs ? `${stats.latencyMs} ms` : "—"}
        </b>
      </span>
      <span className="metric">
        <em>字数</em>
        <b className="mono" data-testid="metric-chars">
          {stats.chars}
        </b>
      </span>
      <span className="metric">
        <em>句数</em>
        <b className="mono">{stats.segments}</b>
      </span>
      <span className="metric">
        <em>音频</em>
        <b className="mono">{formatDuration(durationMs)}</b>
      </span>
    </div>
  );
}

export function ControlBar() {
  const session = useStore((s) => s.session);
  const sources = useStore((s) => s.audioSources);
  const settings = useStore((s) => s.settings);
  const summarizing = useStore((s) => s.summarizing);

  const micLabel = useMemo(() => {
    if (session?.config.micLabel) return session.config.micLabel;
    const s = sources.find((x) => x.kind === "microphone" && x.isDefault) ?? sources.find((x) => x.kind === "microphone");
    return s?.label ?? "未启用";
  }, [session, sources]);

  const loopLabel = useMemo(() => {
    if (session?.config.loopbackLabel) return session.config.loopbackLabel;
    const s = sources.find((x) => x.kind === "loopback" && x.isDefault) ?? sources.find((x) => x.kind === "loopback");
    return s?.label ?? "未启用";
  }, [session, sources]);

  const paused = session?.status === "paused";
  const recording = Boolean(session);
  const micOn = settings?.audio.enableMic ?? true;
  const loopOn = settings?.audio.enableLoopback ?? false;

  return (
    <footer className="controlbar">
      <div className="levels">
        {micOn ? <LevelMeter kind="mic" label={micLabel} hint="麦克风" /> : <div className="level off">麦克风已关闭</div>}
        {loopOn ? (
          <LevelMeter kind="loop" label={loopLabel} hint="系统声音" />
        ) : (
          <div className="level off">系统内录已关闭</div>
        )}
      </div>

      <Metrics />

      <div className="controls">
        {!recording ? (
          <Button variant="record" onClick={() => void startRecording()} testId="start-recording" ariaLabel="开始录音">
            ● 开始录音
          </Button>
        ) : (
          <>
            <Button
              variant="default"
              onClick={() => void pauseOrResume()}
              testId="pause-recording"
              ariaLabel={paused ? "继续录音" : "暂停录音"}
            >
              {paused ? "▶ 继续" : "❚❚ 暂停"}
            </Button>
            <Button variant="danger" onClick={() => void stopRecording()} testId="stop-recording" ariaLabel="停止录音">
              ■ 停止
            </Button>
          </>
        )}
        <Button
          variant="default"
          onClick={() => void importAudioFile()}
          disabled={recording}
          testId="import-audio"
          ariaLabel="导入音频文件"
          title="导入本地音频文件，用当前识别服务转写"
        >
          ⤓ 导入音频
        </Button>
        <Button
          variant="ghost"
          onClick={() => void summarizeNow(session?.id ?? null)}
          disabled={summarizing}
          testId="summarize-now-bar"
          ariaLabel="立即总结"
        >
          ✦ 立即总结
        </Button>
        <Button
          variant="ghost"
          onClick={() => useStore.getState().openSettings("asr")}
          testId="open-asr-settings-bar"
          ariaLabel="识别服务设置"
          title="打开「设置 → 语音识别」，配置识别服务商"
        >
          ☁ 识别服务
        </Button>
      </div>
    </footer>
  );
}
