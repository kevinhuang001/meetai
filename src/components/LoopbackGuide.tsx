/**
 * 系统内录（虚拟声卡）引导卡片。
 *
 * 诚实前提：虚拟声卡是系统级音频驱动，应用无法内置安装。
 * 所以这里只做「检测 + 手把手引导 + 一键打开下载页 + 重新检测」。
 * 向导第 4 步与「设置 → 音频设备」共用这一个组件。
 */
import { useState } from "react";
import { api } from "../lib/api";
import { BLACKHOLE_PAGE } from "../lib/links";
import { copyText } from "../lib/util";
import { guard, useStore } from "../store";
import { Button } from "./ui";
import { IconCopy } from "./icons";

function Command({ cmd, testId }: { cmd: string; testId?: string }) {
  const toast = useStore((s) => s.toast);
  const [copied, setCopied] = useState(false);

  return (
    <div className="cmd-row">
      <code className="notice-code grow" data-testid={testId}>
        {cmd}
      </code>
      <Button
        variant="ghost"
        ariaLabel="复制命令"
        title="复制命令"
        onClick={() => {
          void copyText(cmd).then((ok) => {
            setCopied(ok);
            if (ok) {
              toast("success", "复制", "命令已复制到剪贴板");
              window.setTimeout(() => setCopied(false), 2500);
            } else {
              toast("error", "复制", "剪贴板不可用，请手动选中复制");
            }
          });
        }}
      >
        <IconCopy />
        {copied ? "已复制" : "复制"}
      </Button>
    </div>
  );
}

function Steps({ items }: { items: string[] }) {
  return (
    <ol className="guide-steps">
      {items.map((s) => (
        <li key={s}>{s}</li>
      ))}
    </ol>
  );
}

export function LoopbackGuide({
  platform,
  onRefresh,
  refreshing = false,
}: {
  platform: string | undefined;
  onRefresh: () => void;
  refreshing?: boolean;
}) {
  const toast = useStore((s) => s.toast);

  const openPage = () => {
    void guard("打开下载页", () => api.openUrl(BLACKHOLE_PAGE.url)).then((r) => {
      if (r !== undefined) toast("info", "下载页", "已用系统默认浏览器打开 BlackHole 下载页");
    });
  };

  const kind = platform === "macos" || platform === "linux" || platform === "windows" ? platform : "generic";

  return (
    <div className="guide-card" data-testid="loopback-guide" data-platform={kind}>
      <div className="guide-head">
        <strong>
          {kind === "macos" ? "macOS 需要先装虚拟声卡才能内录系统声音" : "没有检测到系统内录设备"}
        </strong>
        <span className="dim tiny" data-testid="loopback-guide-platform">
          当前平台：{kind === "generic" ? "浏览器（mock）" : platform}
        </span>
      </div>

      {kind === "macos" ? (
        <>
          <p className="small">
            macOS 不允许应用直接录制系统声音，必须借助虚拟声卡（BlackHole 免费开源，或 Loopback 等）。
            装好之后还要把系统输出<strong>同时</strong>送到扬声器和 BlackHole，否则对方的声音进不来。
          </p>
          <Command cmd="brew install blackhole-2ch" testId="loopback-guide-cmd-brew" />
          <div className="row">
            <Button variant="default" onClick={openPage} testId="loopback-guide-open-page">
              打开 BlackHole 下载页
            </Button>
            <span className="dim tiny">没用 Homebrew 也可以在这个页面下载安装包</span>
          </div>
          <Steps
            items={[
              "打开「应用程序 → 实用工具 → 音频 MIDI 设置」",
              "点左下角「+」→「创建多输出设备」",
              "在右侧勾选你的扬声器（或耳机）和 BlackHole 2ch，然后把「多输出设备」设为系统输出",
            ]}
          />
        </>
      ) : kind === "linux" ? (
        <>
          <p className="small">
            系统内录需要 PulseAudio 或 PipeWire 的 monitor 源。deb 安装包已声明依赖{" "}
            <code className="mono">pulseaudio-utils</code>，若缺少可执行：
          </p>
          <Command cmd="sudo apt install pulseaudio-utils" testId="loopback-guide-cmd-apt" />
          <p className="dim small">
            装好后可以用下面的命令确认存在 monitor 源（PipeWire 用户把 pactl 换成 pw-cli / wpctl 亦可）：
          </p>
          <Command cmd="pactl list short sources | grep monitor" />
        </>
      ) : kind === "windows" ? (
        <>
          <p className="small">
            Windows 通过 WASAPI loopback 内录，一般开箱可用。若列表为空，请检查音频输出设备是否被禁用。
          </p>
          <Steps
            items={[
              "右键任务栏音量图标 →「声音设置」",
              "确认「输出」里有可用的播放设备，且没有被「禁用」",
              "插上耳机或外接显示器后重新检测；部分虚拟声卡（VB-Cable）需要重启音频服务",
            ]}
          />
        </>
      ) : (
        <>
          <p className="small">
            当前是浏览器 mock 环境，不会返回真实的系统内录设备，这属于正常现象。
            进入桌面端后，这里会按你的操作系统给出对应的安装步骤。
          </p>
          <Steps
            items={[
              "macOS：安装 BlackHole 等虚拟声卡，并创建多输出设备",
              "Linux：安装 pulseaudio-utils，使用 PulseAudio / PipeWire 的 monitor 源",
              "Windows：使用 WASAPI loopback，确认播放设备未被禁用",
            ]}
          />
        </>
      )}

      <div className="row">
        <Button variant="primary" onClick={onRefresh} disabled={refreshing} testId="loopback-guide-refresh">
          {refreshing ? "检测中…" : "⟳ 重新检测设备"}
        </Button>
        <span className="dim tiny">装好之后点这里重新扫描，不需要重启应用</span>
      </div>
    </div>
  );
}
