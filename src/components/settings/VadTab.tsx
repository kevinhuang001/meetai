/** 设置 · 断句（自适应能量 VAD）：决定一句话什么时候算说完 */
import type { VadSettings } from "../../lib/contract";
import { Field, Slider } from "../ui";
import type { TabProps } from "./SettingsDialog";

export function VadTab({ draft, set }: TabProps) {
  const vad: VadSettings = draft.vad;
  const patchVad = (p: Partial<VadSettings>) => set("vad", { ...vad, ...p });

  return (
    <div className="tab-body">
      <section className="card">
        <h3>断句 / VAD</h3>
        <div className="warn-bar" data-testid="vad-engine-note">
          <strong>自适应能量 VAD，无需模型文件</strong>
          <span>
            断句完全由自适应噪声底 + 能量门限决定：不再有 silero / energy 之分，也不需要下载任何 VAD 模型。
            它会持续估计环境噪声底，安静房间自动更灵敏，嘈杂环境自动更保守。
          </span>
        </div>

        <Field label="能量门限" hint="高于自适应噪声底多少 dB 判定为语音（越大越不容易被噪声触发）">
          <Slider
            value={vad.energyThresholdDb}
            min={3}
            max={30}
            step={1}
            onChange={(v) => patchVad({ energyThresholdDb: v })}
            ariaLabel="能量门限"
            testId="vad-energy-threshold"
            format={(v) => `${v} dB`}
          />
        </Field>
        <Field label="最短语音" hint="短于该时长的片段不触发识别，过滤咳嗽、敲键盘等噪声">
          <Slider
            value={vad.minSpeechMs}
            min={50}
            max={2000}
            step={50}
            onChange={(v) => patchVad({ minSpeechMs: v })}
            ariaLabel="最短语音时长"
            testId="vad-min-speech"
            format={(v) => `${v} ms`}
          />
        </Field>
        <Field label="静音断句时长" hint="静音多久判定一句话结束 —— 决定按句定稿的时机">
          <Slider
            value={vad.minSilenceMs}
            min={100}
            max={3000}
            step={50}
            onChange={(v) => patchVad({ minSilenceMs: v })}
            ariaLabel="静音断句时长"
            testId="vad-min-silence"
            format={(v) => `${v} ms`}
          />
        </Field>
        <Field label="最长单句" hint="超时强制断句，避免延迟无限增长">
          <Slider
            value={vad.maxUtteranceMs}
            min={5000}
            max={60000}
            step={1000}
            onChange={(v) => patchVad({ maxUtteranceMs: v })}
            ariaLabel="最长单句时长"
            testId="vad-max-utterance"
            format={(v) => `${Math.round(v / 1000)} s`}
          />
        </Field>
        <Field label="断句留白" hint="断句前后保留的音频，避免吃字">
          <Slider
            value={vad.speechPadMs}
            min={0}
            max={1000}
            step={20}
            onChange={(v) => patchVad({ speechPadMs: v })}
            ariaLabel="断句留白"
            testId="vad-pad"
            format={(v) => `${v} ms`}
          />
        </Field>
      </section>
    </div>
  );
}
