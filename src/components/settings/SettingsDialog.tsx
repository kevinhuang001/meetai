/** 设置对话框：本地草稿态 + 保存/取消/恢复默认 */
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { Settings } from "../../lib/contract";
import { applyAppearance } from "../../lib/appearance";
import { guard, useStore, type SettingsTab } from "../../store";
import { Button, Modal } from "../ui";
import { AiTab } from "./AiTab";
import { AsrTab } from "./AsrTab";
import { VadTab } from "./VadTab";
import { AudioTab } from "./AudioTab";
import { GeneralTab } from "./GeneralTab";

const TABS: { id: SettingsTab; label: string; icon: string }[] = [
  { id: "ai", label: "AI 接口", icon: "✦" },
  { id: "asr", label: "语音识别", icon: "◈" },
  { id: "audio", label: "音频设备", icon: "◎" },
  { id: "vad", label: "断句", icon: "⌘" },
  { id: "general", label: "通用", icon: "⚙" },
];

export interface TabProps {
  draft: Settings;
  set: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}

export function SettingsDialog() {
  const open = useStore((s) => s.settingsOpen);
  const tab = useStore((s) => s.settingsTab);
  const setTab = useStore((s) => s.setSettingsTab);
  const storeSettings = useStore((s) => s.settings);
  const setSettings = useStore((s) => s.setSettings);
  const close = useStore((s) => s.closeSettings);
  const toast = useStore((s) => s.toast);

  const [draft, setDraft] = useState<Settings | null>(storeSettings);
  const [dirty, setDirty] = useState(false);

  // 每次打开时把草稿重置为当前设置
  useEffect(() => {
    if (open) {
      setDraft(useStore.getState().settings);
      setDirty(false);
    }
  }, [open]);

  if (!open) return null;

  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setDraft((d) => (d ? { ...d, [key]: value } : d));
    setDirty(true);
  };

  const cancel = () => {
    applyAppearance(useStore.getState().settings);
    close();
  };

  const save = async () => {
    if (!draft) return;
    const saved = await guard("保存设置", () => api.saveSettings(draft));
    if (!saved) return;
    setSettings(saved);
    applyAppearance(saved);
    setDirty(false);
    toast("success", "设置", "已保存");
    close();
  };

  const reset = async () => {
    const def = await guard("恢复默认设置", () => api.resetSettings());
    if (!def) return;
    setDraft(def);
    setSettings(def);
    applyAppearance(def);
    setDirty(false);
    toast("info", "设置", "已恢复默认值");
  };

  return (
    <Modal
      open={open}
      onClose={cancel}
      title="设置"
      testId="settings-dialog"
      width={980}
      footer={
        <>
          <span className="dim small">{dirty ? "有未保存的修改" : "所有修改已保存"}</span>
          <span className="spacer" />
          <Button variant="ghost" onClick={() => void reset()} testId="settings-reset">
            恢复默认
          </Button>
          <Button variant="default" onClick={cancel} testId="settings-cancel">
            取消
          </Button>
          <Button variant="primary" onClick={() => void save()} testId="settings-save">
            保存
          </Button>
        </>
      }
    >
      <div className="settings-layout">
        <nav className="settings-tabs" role="tablist" aria-label="设置分类">
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              className={tab === t.id ? "settings-tab active" : "settings-tab"}
              data-testid={`tab-${t.id}`}
              onClick={() => setTab(t.id)}
            >
              <span className="tab-icon" aria-hidden="true">
                {t.icon}
              </span>
              {t.label}
            </button>
          ))}
        </nav>

        <div className="settings-content" data-testid={`settings-panel-${tab}`}>
          {!draft ? (
            <p className="dim">正在读取设置…</p>
          ) : tab === "ai" ? (
            <AiTab draft={draft} set={set} />
          ) : tab === "asr" ? (
            <AsrTab draft={draft} set={set} />
          ) : tab === "audio" ? (
            <AudioTab draft={draft} set={set} />
          ) : tab === "vad" ? (
            <VadTab draft={draft} set={set} />
          ) : (
            <GeneralTab draft={draft} set={set} />
          )}
        </div>
      </div>
    </Modal>
  );
}
