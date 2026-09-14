/** 向导第 5 步：确认并完成 */
import { useStore } from "../../store";
import type { Settings } from "../../lib/contract";
import { activeAsrProvider } from "../../lib/settings";
import { StepHead } from "./parts";

export function StepDone({ draft }: { draft: Settings }) {
  const sources = useStore((s) => s.audioSources);
  const ai = draft.ai.providers.find((p) => p.id === draft.ai.activeProviderId) ?? draft.ai.providers[0] ?? null;
  const asr = activeAsrProvider(draft);

  const label = (id: string | null) =>
    (id ? sources.find((s) => s.id === id)?.label : null) ?? "系统默认设备";

  const rows: [string, string][] = [
    ["语音识别服务", asr ? `${asr.name} · ${asr.model || "未填模型"}` : "未配置（无法开始录音）"],
    [
      "AI 接口",
      draft.ai.enabled && ai ? `${ai.name} · ${ai.model || "未填模型"}` : "已关闭（只转写，不做纪要）",
    ],
    [
      "音频来源",
      [
        draft.audio.enableMic ? `麦克风：${label(draft.audio.micDeviceId)}` : "麦克风：关闭",
        draft.audio.enableLoopback ? `系统内录：${label(draft.audio.loopbackDeviceId)}` : "系统内录：关闭",
      ].join("　"),
    ],
    ["原始录音", draft.audio.saveAudio ? "保存为 wav（便于回听与重新转写）" : "不保存"],
  ];

  return (
    <div className="ob-body-inner">
      <StepHead title="配置完成" desc="下面是即将保存的配置，确认无误就进入主界面。" />

      <dl className="ob-summary" data-testid="onboarding-done-summary">
        {rows.map(([k, v]) => (
          <div key={k} className="ob-summary-row">
            <dt>{k}</dt>
            <dd className="mono">{v}</dd>
          </div>
        ))}
      </dl>

      <div className="ob-tip">
        <strong>这些设置随时可以在「设置」里修改</strong>
        <span className="small">
          主界面左下角有「设置」入口：语音识别、AI 接口、音频设备都能重新配置；
          想再走一遍这个向导，去「设置 → 通用 → 重新运行配置向导」。
        </span>
        <span className="small">
          点「完成」后的第一件事建议是点左下/侧栏的「＋ 新建会议」开一场真会议试试；
          也可以用底部的「导入音频」拿现成的录音文件验证识别服务是否正常。
        </span>
      </div>
    </div>
  );
}
