/** 设置 · 通用 */
import { api } from "../../lib/api";
import type { GeneralSettings } from "../../lib/contract";
import { applyAppearance } from "../../lib/appearance";
import { guard, useStore } from "../../store";
import { Button, Field, Select, Slider, SwitchRow, TextInput } from "../ui";
import type { TabProps } from "./SettingsDialog";

export function GeneralTab({ draft, set }: TabProps) {
  const appInfo = useStore((s) => s.appInfo);
  const toast = useStore((s) => s.toast);
  const general: GeneralSettings = draft.general;

  const patch = (p: Partial<GeneralSettings>) => {
    const next = { ...general, ...p };
    set("general", next);
    // 主题与字号即时预览
    applyAppearance({ ...draft, general: next });
  };

  const envText = appInfo
    ? [
        `版本 ${appInfo.version}`,
        `${appInfo.platform} / ${appInfo.arch}`,
        `识别服务 ${appInfo.asrService}`,
        `断句 ${appInfo.vadEngine}`,
        `数据目录 ${appInfo.dataDir}`,
      ].join("\n")
    : "";

  return (
    <div className="tab-body">
      <section className="card">
        <h3>外观</h3>
        <Field label="主题">
          <Select
            value={general.theme}
            options={[
              { value: "dark", label: "深色（默认）" },
              { value: "light", label: "浅色" },
              { value: "system", label: "跟随系统" },
            ]}
            onChange={(v) => patch({ theme: v })}
            ariaLabel="主题"
            testId="theme-select"
          />
        </Field>
        <Field label="字号缩放" hint="作用于整个界面，0.8 ~ 1.6">
          <Slider
            value={general.fontScale}
            min={0.8}
            max={1.6}
            step={0.05}
            onChange={(v) => patch({ fontScale: v })}
            ariaLabel="字号缩放"
            testId="font-scale"
            format={(v) => `${Math.round(v * 100)}%`}
          />
        </Field>
        <SwitchRow
          label="转写自动滚动"
          hint="关闭后新内容不会自动吸底，需要手动滚动"
          checked={general.autoScroll}
          onChange={(v) => patch({ autoScroll: v })}
          testId="auto-scroll"
        />
      </section>

      <section className="card">
        <h3>数据目录</h3>
        <p className="dim small mono">{appInfo?.dataDir ?? "—"}</p>
        <div className="row">
          <Button variant="ghost" onClick={() => void guard("打开数据目录", () => api.openPath(appInfo?.dataDir ?? ""))}>
            在文件管理器中打开
          </Button>
          <span className="dim tiny">录音 {appInfo?.recordingsDir ?? "—"}</span>
        </div>
        <Field label="数据目录覆盖" hint="留空使用系统默认目录（需重启生效）">
          <TextInput
            value={general.dataDir ?? ""}
            onChange={(v) => patch({ dataDir: v.trim() ? v : null })}
            placeholder="系统默认"
            mono
          />
        </Field>
        <p className="dim tiny">
          应用不再下载或保存任何识别模型，配置目录里只有设置、会话记录与（可选的）录音文件。
        </p>
      </section>

      <section className="card">
        <h3>安全与隐私</h3>
        <SwitchRow
          label="把 API Key 保存到本地"
          hint="关闭后每次启动都需要重新填写（内存中保存）"
          checked={general.persistApiKey}
          onChange={(v) => patch({ persistApiKey: v })}
          testId="persist-apikey"
        />
        <p className="dim small">
          音频采集在本机进行；只有开启语音识别时，音频才会发送到你配置的识别服务商（本地服务不出本机），
          只有开启 AI 纪要时才会把文本发送给 AI 服务商。
        </p>
      </section>

      <section className="card">
        <h3>快捷键</h3>
        <ul className="shortcut-list">
          <li>
            <kbd>Ctrl</kbd> / <kbd>⌘</kbd> + <kbd>Enter</kbd> <span>开始 / 停止录音</span>
          </li>
          <li>
            <kbd>Ctrl</kbd> / <kbd>⌘</kbd> + <kbd>K</kbd> <span>打开历史会话</span>
          </li>
          <li>
            <kbd>Esc</kbd> <span>关闭设置 / 取消编辑</span>
          </li>
        </ul>
        <p className="dim tiny">
          应用版本 {appInfo?.version ?? "—"} · {appInfo?.platform ?? "—"} / {appInfo?.arch ?? "—"}
        </p>
        <p className="dim tiny" data-testid="about-asr-service">
          识别服务：<span className="mono">{appInfo?.asrService ?? "—"}</span>
        </p>
        <p className="dim tiny">
          断句方式：<span className="mono">{appInfo?.vadEngine ?? "—"}</span>
        </p>
        <Button
          variant="ghost"
          onClick={() => {
            void guard("复制系统信息", async () => navigator.clipboard.writeText(envText)).then((r) => {
              if (r !== undefined) toast("success", "系统信息", "已复制到剪贴板");
            });
          }}
        >
          复制系统信息
        </Button>
      </section>
    </div>
  );
}
