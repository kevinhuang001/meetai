/** 向导第 4 步：音频设备（麦克风 + 系统内录） */
import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import type { AudioSettings, AudioSourceInfo } from "../../lib/contract";
import { guard, useStore } from "../../store";
import { LoopbackGuide } from "../LoopbackGuide";
import { SwitchRow } from "../ui";
import { StepHead, type StepProps } from "./parts";

function pickDefault(list: AudioSourceInfo[], kind: "microphone" | "loopback"): AudioSourceInfo | null {
  const of = list.filter((s) => s.kind === kind && s.available);
  return of.find((s) => s.isDefault) ?? of[0] ?? null;
}

function DeviceRows({
  sources,
  current,
  onPick,
  testIdPrefix,
}: {
  sources: AudioSourceInfo[];
  current: string | null;
  onPick: (id: string | null) => void;
  testIdPrefix: string;
}) {
  return (
    <ul className="device-list pick-list" data-testid={`${testIdPrefix}-list`}>
      <li>
        <button
          type="button"
          className={current === null ? "pick-row active" : "pick-row"}
          aria-pressed={current === null}
          data-testid={`${testIdPrefix}-system-default`}
          onClick={() => onPick(null)}
        >
          <span className="pick-dot" aria-hidden="true" />
          <span className="device-label">系统默认设备</span>
          <span className="dim tiny">让系统在设备变化时自动切换</span>
        </button>
      </li>
      {sources.map((s) => (
        <li key={s.id} className={s.available ? "" : "dim"}>
          <button
            type="button"
            className={current === s.id ? "pick-row active" : "pick-row"}
            aria-pressed={current === s.id}
            disabled={!s.available}
            data-testid={`${testIdPrefix}-${s.id}`}
            onClick={() => onPick(s.id)}
          >
            <span className="pick-dot" aria-hidden="true" />
            <span className="device-label">{s.label}</span>
            <span className="device-detail mono">{s.detail}</span>
            {s.isDefault ? <span className="dim tiny">默认</span> : null}
            {!s.available ? <span className="warn tiny">{s.note ?? "不可用"}</span> : null}
          </button>
        </li>
      ))}
      {sources.length === 0 ? <li className="dim">没有检测到这一类设备</li> : null}
    </ul>
  );
}

export function StepAudio({ draft, set }: StepProps) {
  const sources = useStore((s) => s.audioSources);
  const setAudioSources = useStore((s) => s.setAudioSources);
  const appInfo = useStore((s) => s.appInfo);
  const toast = useStore((s) => s.toast);
  const [refreshing, setRefreshing] = useState(false);
  const audio: AudioSettings = draft.audio;

  const patch = (p: Partial<AudioSettings>) => set("audio", { ...audio, ...p });

  const mics = sources.filter((s) => s.kind === "microphone");
  const loops = sources.filter((s) => s.kind === "loopback");
  const hasLoopback = loops.some((s) => s.available);

  const refresh = async () => {
    setRefreshing(true);
    const list = await guard("重新检测音频设备", () => api.listAudioSources());
    setRefreshing(false);
    if (!list) return;
    setAudioSources(list);
    const ok = list.some((s) => s.kind === "loopback" && s.available);
    if (ok) toast("success", "音频设备", `已检测到 ${list.length} 个设备，含系统内录`);
    else toast("info", "音频设备", `已重新检测（${list.length} 个设备），仍未发现系统内录`);
  };

  /* 首次进入这一步时，把系统默认的麦克风/内录设备显式写进设置（只做一次，不跟用户的选择打架） */
  const prefilled = useRef(false);
  useEffect(() => {
    if (prefilled.current || sources.length === 0) return;
    prefilled.current = true;
    const mic = pickDefault(sources, "microphone");
    const loop = pickDefault(sources, "loopback");
    const next: Partial<AudioSettings> = {};
    if (audio.micDeviceId === null && mic) next.micDeviceId = mic.id;
    if (audio.loopbackDeviceId === null && loop) next.loopbackDeviceId = loop.id;
    if (Object.keys(next).length > 0) patch(next);
  }, [sources]);

  return (
    <div className="ob-body-inner">
      <StepHead
        title="音频设备"
        desc="麦克风采集你的声音，系统内录采集会议软件播放出来的对方声音。两个都开才能得到完整的会议记录。"
      />

      <section className="ob-section" data-testid="onboarding-audio-list">
        <SwitchRow
          label="录制麦克风"
          hint="采集本机说话人的声音"
          checked={audio.enableMic}
          onChange={(v) => patch({ enableMic: v })}
          testId="onboarding-enable-mic"
        />
        {audio.enableMic ? (
          <DeviceRows
            sources={mics}
            current={audio.micDeviceId}
            onPick={(id) => patch({ micDeviceId: id })}
            testIdPrefix="onboarding-mic"
          />
        ) : null}
      </section>

      <section className="ob-section">
        <SwitchRow
          label="系统内录（对方声音）"
          hint="把会议软件播放的声音也转写进来"
          checked={audio.enableLoopback}
          onChange={(v) => patch({ enableLoopback: v })}
          testId="onboarding-enable-loopback"
        />
        {audio.enableLoopback ? (
          <DeviceRows
            sources={loops}
            current={audio.loopbackDeviceId}
            onPick={(id) => patch({ loopbackDeviceId: id })}
            testIdPrefix="onboarding-loopback"
          />
        ) : null}
      </section>

      {!hasLoopback ? (
        <LoopbackGuide platform={appInfo?.platform} onRefresh={() => void refresh()} refreshing={refreshing} />
      ) : (
        <p className="dim tiny">
          检测到系统内录设备了。真实会议里建议麦克风与内录同时开启，AI 会自动把两路声音合并成一份记录。
        </p>
      )}
    </div>
  );
}
