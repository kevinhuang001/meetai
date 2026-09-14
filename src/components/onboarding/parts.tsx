/** 配置向导的公共小组件：步骤标题、测试结果框、外链按钮 */
import type { ReactNode } from "react";
import { api } from "../../lib/api";
import type { Settings } from "../../lib/contract";
import type { ConsoleLink } from "../../lib/links";
import { guard, useStore } from "../../store";
import { Button } from "../ui";

/** 每个步骤拿到的草稿设置与写回函数（与设置弹窗的 TabProps 同构） */
export interface StepProps {
  draft: Settings;
  set: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}

export interface ProbeResult {
  ok: boolean;
  latencyMs: number;
  error: string | null;
  sample: string;
  models?: string[];
}

export function StepHead({ title, desc }: { title: string; desc: ReactNode }) {
  return (
    <div className="ob-head">
      <h2>{title}</h2>
      <p className="dim small">{desc}</p>
    </div>
  );
}

/** 通用的「测试连接」结果框：成功给延迟 + 静音说明，失败给后端返回的中文错误 */
export function TestBox({
  result,
  testId,
  emptySampleNote,
}: {
  result: ProbeResult;
  testId: string;
  emptySampleNote?: string;
}) {
  return (
    <div className={result.ok ? "test-result ok" : "test-result bad"} data-testid={testId}>
      <div className="row">
        <strong>{result.ok ? "连接成功" : "连接失败"}</strong>
        <span className="spacer" />
        {result.ok ? <span className="mono small">延迟 {result.latencyMs} ms</span> : null}
      </div>
      {result.ok ? (
        <p className="small">
          返回样例：<span className="mono">{result.sample || "（空文本）"}</span>
        </p>
      ) : null}
      {result.ok && result.sample.trim().length === 0 ? (
        <p className="dim tiny">{emptySampleNote ?? "返回文本为空是正常的（测试用静音音频）。"}</p>
      ) : null}
      {!result.ok ? <p className="small">{result.error ?? "未知错误"}</p> : null}
    </div>
  );
}

/** 用系统默认浏览器打开服务商控制台 / 下载页 */
export function LinkButtons({ links, testId }: { links: ConsoleLink[]; testId?: string }) {
  const toast = useStore((s) => s.toast);
  if (links.length === 0) return null;
  return (
    <div className="row" data-testid={testId}>
      {links.map((l) => (
        <Button
          key={l.url}
          variant="ghost"
          ariaLabel={l.label}
          onClick={() => {
            void guard("打开链接", () => api.openUrl(l.url)).then((r) => {
              if (r !== undefined) toast("info", "已打开浏览器", l.url);
            });
          }}
        >
          ↗ {l.label}
        </Button>
      ))}
    </div>
  );
}
