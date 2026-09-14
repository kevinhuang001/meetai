/** 右侧「AI 实时纪要」面板：录制视图与会话详情视图共用 */
import { useMemo, useState } from "react";
import type { SummaryState } from "../lib/contract";
import { formatTimeOfDay } from "../lib/util";
import { Badge, Button } from "./ui";
import { IconChevron, IconSparkle } from "./icons";

export interface SummaryPanelProps {
  summary: SummaryState;
  aiStatus: "idle" | "thinking" | "error";
  aiMessage: string | null;
  /** 最新转写结束位置，用于算“纪要落后多少” */
  latestEndMs: number;
  ready: boolean;
  notReadyReason: string;
  onSummarizeNow: () => void;
  onOpenAiSettings: () => void;
  busy?: boolean;
}

function ThinkingSkeleton() {
  return (
    <div className="skeleton" data-testid="summary-thinking" aria-hidden="true">
      <span className="sk-line w90" />
      <span className="sk-line w70" />
      <span className="sk-line w50" />
    </div>
  );
}

/**
 * 纪要要点：优先渲染模型给的 markdown 列表，没有时退回关键要点数组。
 *
 * 之前「关键要点 / 纪要要点 / 已达成的决定」是三块独立分区，内容大量重叠
 * （模型本来就会把结论与决定写进要点列表），所以合并成一块。
 */
function MarkdownList({ text, fallback }: { text: string; fallback: string[] }) {
  const items = useMemo(() => {
    const lines = text
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => l.replace(/^[-*•]\s*/, "").trim())
      .filter(Boolean);
    return lines.length ? lines : fallback;
  }, [text, fallback]);

  if (!items.length) return null;
  return (
    <ul className="kv-list" data-testid="summary-keypoints">
      {items.map((k, i) => (
        <li key={`${i}-${k}`}>{k}</li>
      ))}
    </ul>
  );
}

export function SummaryPanel({
  summary,
  aiStatus,
  // 保留在 props 里以兼容调用方；错误信息统一走 summary.error，避免两处重复展示
  aiMessage: _aiMessage,
  latestEndMs,
  ready,
  notReadyReason,
  onSummarizeNow,
  onOpenAiSettings,
  busy = false,
}: SummaryPanelProps) {
  const [collapsed, setCollapsed] = useState(false);
  const [checked, setChecked] = useState<Record<string, boolean>>({});

  const lagSecs = useMemo(() => {
    if (!latestEndMs || !summary.coveredUntilMs) return 0;
    return Math.max(0, Math.round((latestEndMs - summary.coveredUntilMs) / 1000));
  }, [latestEndMs, summary.coveredUntilMs]);

  const thinking = aiStatus === "thinking";

  if (collapsed) {
    return (
      <aside className="summary-panel collapsed" data-testid="summary-panel" aria-label="AI 实时纪要">
        <button
          className="icon-btn collapse-btn"
          aria-label="展开 AI 纪要面板"
          title="展开 AI 纪要"
          onClick={() => setCollapsed(false)}
        >
          <IconChevron dir="left" />
        </button>
        <span className="collapsed-label">AI 纪要</span>
      </aside>
    );
  }

  return (
    <aside className="summary-panel" data-testid="summary-panel" aria-label="AI 实时纪要">
      <header className="panel-head">
        <h2>AI 实时纪要</h2>
        <div className="panel-head-right">
          {summary.model ? <span className="dim tiny mono">{summary.model}</span> : null}
          <button
            className="icon-btn"
            aria-label="折叠 AI 纪要面板"
            title="折叠"
            onClick={() => setCollapsed(true)}
          >
            <IconChevron dir="right" />
          </button>
        </div>
      </header>

      {!ready ? (
        <div className="panel-scroll">
          <div className="empty-card" data-testid="summary-not-configured">
            <div className="empty-icon"><IconSparkle size={30} /></div>
            <h4>未配置 AI 接口</h4>
            <p>{notReadyReason || "配置后这里会实时生成会议纪要。"}</p>
            <Button variant="primary" onClick={onOpenAiSettings} testId="summary-open-ai-settings">
              去配置 AI 接口
            </Button>
          </div>
        </div>
      ) : (
        <>
          <div className="panel-scroll">
            {summary.error ? (
              <div className="warn-bar" data-testid="summary-error" role="alert">
                <strong>纪要出错</strong>
                <span>{summary.error}</span>
              </div>
            ) : null}

            <section className="live-card" data-testid="summary-live-card">
              <div className="live-head">
                <span className="live-title">刚刚说到</span>
                {thinking ? (
                  <span className="thinking-dot">
                    <i />
                    <i />
                    <i />
                  </span>
                ) : null}
              </div>
              {thinking ? (
                <ThinkingSkeleton />
              ) : (
                <p className="live-text" data-testid="summary-live">
                  {summary.live || "开始说话后这里会实时更新。"}
                </p>
              )}
            </section>

            {/* 面板只留「边开会边要看」的三块；关键结论/已达成决定/主题等
                仍然照常生成，并且都会出现在导出的会议纪要里，不在这里重复展示。 */}
            {summary.overview.trim() ? (
              <section className="block">
                <h4>会议总览</h4>
                <p className="overview" data-testid="summary-overview">
                  {summary.overview}
                </p>
              </section>
            ) : null}

            {summary.summary.trim() || summary.keyPoints.length ? (
              <section className="block">
                <h4>纪要要点</h4>
                <MarkdownList text={summary.summary} fallback={summary.keyPoints} />
              </section>
            ) : null}

            {summary.actionItems.length ? (
              <section className="block">
                <h4>
                  待办事项 <Badge tone="accent">{summary.actionItems.length}</Badge>
                </h4>
                <ul className="todo-list" data-testid="summary-actions">
                  {summary.actionItems.map((a, i) => {
                    const key = `${i}-${a.text}`;
                    const done = Boolean(checked[key]);
                    return (
                      <li key={key} className={done ? "done" : ""}>
                        <label className="todo-item">
                          <input
                            type="checkbox"
                            checked={done}
                            aria-label={`标记待办完成：${a.text}`}
                            onChange={(e) => setChecked((c) => ({ ...c, [key]: e.target.checked }))}
                          />
                          <span className="todo-text">{a.text}</span>
                        </label>
                        <span className="todo-meta">
                          {a.owner ? `@${a.owner}` : ""}
                          {a.owner && a.due ? " · " : ""}
                          {a.due}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </section>
            ) : null}
          </div>

          <footer className="panel-foot">
            {/* 只在真的落后时才提示；平时不占一行 */}
            {lagSecs > 20 ? (
              <div className="lag warn small" data-testid="summary-lag">
                纪要落后转写 {lagSecs} 秒
              </div>
            ) : null}
            <Button
              variant="primary"
              onClick={onSummarizeNow}
              disabled={busy || thinking}
              testId="summarize-now"
              className="block-btn"
              title={`共总结 ${summary.calls} 次${
                summary.updatedAt ? ` · 更新于 ${formatTimeOfDay(summary.updatedAt)}` : ""
              }`}
            >
              {busy || thinking ? "总结中…" : "立即总结"}
            </Button>
          </footer>
        </>
      )}
    </aside>
  );
}
