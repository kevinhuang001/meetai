/**
 * 首次运行配置向导（全屏弹窗，5 步）。
 *
 * 为什么存在：应用不含本地识别模型，不配好「识别服务 + AI 接口」就完全没法用。
 * 所以向导的核心价值是把这两件事讲清楚，并让用户当场点「测试连接」验证。
 *
 * 关闭/跳过都会把 onboardingCompleted 置为 true，避免每次启动都弹；
 * 想再走一遍可以走「设置 → 通用 → 重新运行配置向导」。
 */
import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import type { Settings } from "../../lib/contract";
import { applyAppearance } from "../../lib/appearance";
import { guard, useStore } from "../../store";
import { Button } from "../ui";
import { StepWelcome } from "./StepWelcome";
import { StepAsr } from "./StepAsr";
import { StepAi } from "./StepAi";
import { StepAudio } from "./StepAudio";
import { StepDone } from "./StepDone";

const STEPS = ["欢迎", "语音识别服务", "AI 接口", "音频设备", "完成"] as const;

export function OnboardingWizard() {
  const storeSettings = useStore((s) => s.settings);
  const setSettings = useStore((s) => s.setSettings);
  const closeOnboarding = useStore((s) => s.closeOnboarding);
  const toast = useStore((s) => s.toast);

  const [draft, setDraft] = useState<Settings | null>(storeSettings);
  const [step, setStep] = useState(0);
  const [saving, setSaving] = useState(false);
  /** Esc 关闭用的最新 finish 引用（避免每次渲染重新绑定键盘监听） */
  const finishRef = useRef<(kind: "done" | "skip" | "close") => Promise<void>>(async () => {});

  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
  };

  /** 结束向导：把 onboardingCompleted 写回 true 并保存（关闭/跳过/完成都走这里） */
  const finish = async (kind: "done" | "skip" | "close") => {
    if (!draft) {
      closeOnboarding();
      return;
    }
    const next: Settings = { ...draft, general: { ...draft.general, onboardingCompleted: true } };
    setSaving(true);
    const saved = await guard("保存向导配置", () => api.saveSettings(next));
    setSaving(false);
    // 后端保存失败也要先让本地标记生效，否则一关就再弹一次
    setSettings(saved ?? next);
    applyAppearance(saved ?? next);
    closeOnboarding();
    if (saved) {
      toast(
        "success",
        "配置向导",
        kind === "done"
          ? "配置已保存，随时可以在「设置」里修改"
          : "已跳过向导；之后可以在「设置 → 通用 → 重新运行配置向导」里再来一次",
      );
    } else {
      toast("error", "配置向导", "设置保存失败，下次启动会再次显示向导");
    }
  };
  finishRef.current = finish;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void finishRef.current("close");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (!draft) {
    return (
      <div className="ob-backdrop" data-testid="onboarding-wizard">
        <div className="ob-panel boot">
          <span className="spinner" aria-hidden="true" />
          正在读取设置…
        </div>
      </div>
    );
  }

  const last = step === STEPS.length - 1;

  return (
    <div className="ob-backdrop" data-testid="onboarding-wizard">
      <div className="ob-panel" role="dialog" aria-modal="true" aria-label="首次运行配置向导">
        <header className="ob-top">
          <div>
            <h1 data-testid="onboarding-title">首次运行配置向导</h1>
            <p className="dim small">
              第 {step + 1} / {STEPS.length} 步 · {STEPS[step]}
            </p>
          </div>
          <span className="spacer" />
          <button
            className="icon-btn"
            aria-label="关闭配置向导"
            title="关闭向导（会记为已完成，可在设置里重新打开）"
            data-testid="onboarding-close"
            onClick={() => void finish("close")}
          >
            ✕
          </button>
        </header>

        <ol className="ob-steps" data-testid="onboarding-step-indicator">
          {STEPS.map((label, i) => (
            <li
              key={label}
              className={i === step ? "ob-step active" : i < step ? "ob-step done" : "ob-step"}
              data-testid={`onboarding-step-${i + 1}`}
              aria-current={i === step ? "step" : undefined}
            >
              <span className="ob-step-idx mono">{i < step ? "✓" : i + 1}</span>
              <span className="ob-step-label">{label}</span>
            </li>
          ))}
        </ol>

        <div className="ob-body" data-testid={`onboarding-body-${step + 1}`}>
          {step === 0 ? <StepWelcome /> : null}
          {step === 1 ? <StepAsr draft={draft} set={set} /> : null}
          {step === 2 ? <StepAi draft={draft} set={set} /> : null}
          {step === 3 ? <StepAudio draft={draft} set={set} /> : null}
          {step === 4 ? <StepDone draft={draft} /> : null}
        </div>

        <footer className="ob-foot">
          <span className="dim tiny ob-foot-hint">
            向导期间不会开始录音；音频只会发送到你在第 2 步配置的识别服务。
          </span>
          <div className="ob-foot-btns">
            <Button variant="ghost" onClick={() => void finish("skip")} testId="onboarding-skip-all" disabled={saving}>
              跳过向导，直接进入
            </Button>
            {step > 0 ? (
              <Button variant="default" onClick={() => setStep((s) => s - 1)} testId="onboarding-prev" disabled={saving}>
                ← 上一步
              </Button>
            ) : null}
            {step >= 1 && step <= 3 ? (
              <Button variant="ghost" onClick={() => setStep((s) => s + 1)} testId="onboarding-skip-step" disabled={saving}>
                跳过这一步
              </Button>
            ) : null}
            {last ? (
              <Button variant="primary" onClick={() => void finish("done")} testId="onboarding-finish" disabled={saving}>
                {saving ? "保存中…" : "✓ 完成，进入主界面"}
              </Button>
            ) : (
              <Button variant="primary" onClick={() => setStep((s) => s + 1)} testId="onboarding-next" disabled={saving}>
                下一步 →
              </Button>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}
