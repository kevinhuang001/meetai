/** 设置 · 音频设备 */
import { api } from "../../lib/api";
import type { AudioSettings } from "../../lib/contract";
import { guard, useStore } from "../../store";
import { LoopbackGuide } from "../LoopbackGuide";
import { Button, Field, Select, Slider, SwitchRow } from "../ui";
import type { TabProps } from "./SettingsDialog";

export function AudioTab({ draft, set }: TabProps) {
  const sources = useStore((s) => s.audioSources);
  const setAudioSources = useStore((s) => s.setAudioSources);
  const appInfo = useStore((s) => s.appInfo);
  const toast = useStore((s) => s.toast);
  const audio: AudioSettings = draft.audio;

  const mics = sources.filter((s) => s.kind === "microphone");
  const loops = sources.filter((s) => s.kind === "loopback");

  const patch = (p: Partial<AudioSettings>) => set("audio", { ...audio, ...p });

  const refresh = async () => {
    const list = await guard("刷新音频设备", () => api.listAudioSources());
    if (list) {
      setAudioSources(list);
      toast("success", "音频设备", `已刷新，共 ${list.length} 个设备`);
    }
  };

  const deviceOptions = (list: typeof sources, current: string | null) => {
    const opts = [{ value: "", label: "系统默认设备" }, ...list.map((s) => ({ value: s.id, label: `${s.label}${s.available ? "" : "（不可用）"}` }))];
    if (current && !list.some((s) => s.id === current)) opts.push({ value: current, label: `${current}（已失效）` });
    return opts;
  };

  return (
    <div className="tab-body">
      <section className="card">
        <div className="row">
          <h3>采集设备</h3>
          <span className="spacer" />
          <Button variant="ghost" onClick={() => void refresh()} testId="refresh-devices">
            ⟳ 刷新设备
          </Button>
        </div>

        <SwitchRow
          label="录制麦克风"
          hint="采集本地说话人的声音"
          checked={audio.enableMic}
          onChange={(v) => patch({ enableMic: v })}
          testId="enable-mic"
        />
        <Field label="麦克风设备">
          <Select
            value={audio.micDeviceId ?? ""}
            options={deviceOptions(mics, audio.micDeviceId)}
            onChange={(v) => patch({ micDeviceId: v || null })}
            ariaLabel="麦克风设备"
            testId="mic-device"
          />
        </Field>
        <Field label="麦克风增益">
          <Slider
            value={audio.micGain}
            min={0}
            max={4}
            step={0.1}
            onChange={(v) => patch({ micGain: v })}
            ariaLabel="麦克风增益"
            format={(v) => `${v.toFixed(1)}×`}
          />
        </Field>

        <hr className="sep" />

        <SwitchRow
          label="系统内录（对方声音）"
          hint="把会议软件播放出来的声音也转写进来"
          checked={audio.enableLoopback}
          onChange={(v) => patch({ enableLoopback: v })}
          testId="enable-loopback"
        />
        <Field label="系统内录设备">
          <Select
            value={audio.loopbackDeviceId ?? ""}
            options={deviceOptions(loops, audio.loopbackDeviceId)}
            onChange={(v) => patch({ loopbackDeviceId: v || null })}
            ariaLabel="系统内录设备"
            testId="loopback-device"
          />
        </Field>
        <Field label="内录增益">
          <Slider
            value={audio.loopbackGain}
            min={0}
            max={4}
            step={0.1}
            onChange={(v) => patch({ loopbackGain: v })}
            ariaLabel="内录增益"
            format={(v) => `${v.toFixed(1)}×`}
          />
        </Field>

        {loops.length === 0 ? (
          <LoopbackGuide platform={appInfo?.platform} onRefresh={() => void refresh()} />
        ) : null}

        <SwitchRow
          label="保存原始录音"
          hint="把采集到的音频保存成 wav，便于回听与重新转写"
          checked={audio.saveAudio}
          onChange={(v) => patch({ saveAudio: v })}
          testId="save-audio"
        />
      </section>

      <section className="card">
        <h3>检测到的设备</h3>
        <ul className="device-list">
          {sources.map((s) => (
            <li key={s.id} className={s.available ? "" : "dim"}>
              <span className="device-label">{s.label}</span>
              <span className="device-detail mono">{s.detail}</span>
              <span className="dim tiny">{s.kind === "microphone" ? "麦克风" : "系统内录"}</span>
              {s.isDefault ? <span className="dim tiny">默认</span> : null}
              {!s.available ? <span className="warn tiny">{s.note ?? "不可用"}</span> : null}
            </li>
          ))}
          {sources.length === 0 ? <li className="dim">正在读取设备列表…</li> : null}
        </ul>
      </section>
    </div>
  );
}
