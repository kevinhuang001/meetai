/** 右侧「AI 实时纪要」面板：录制视图与会话详情视图共用 */
import { useMemo, useState } from "react";
import type { SummaryState } from "../lib/contract";
import { formatClock } from "../lib/contract";
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
  /** 已不再渲染按钮，保留参数以免调用方大改 */
  onSummarizeNow?: () => void;
  onOpenAiSettings: () => void;
  busy?: boolean;
  /**
   * 由外层统一控制折叠。
   * 录制视图里纪要和转写是**同一个面板**，折叠按钮应当长在共享头部上，
   * 而不是每块各长一个（用户会分不清哪个管哪块）。
   */
  collapsed?: boolean;
  onToggleCollapse?: () => void;
  /** 外层已有共享头部时置 false，避免出现两个标题栏 */
  showHeader?: boolean;
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
  onOpenAiSettings,
  busy = false,
  collapsed: collapsedProp,
  onToggleCollapse,
  showHeader = true,
}: SummaryPanelProps) {
  const [collapsedLocal, setCollapsedLocal] = useState(false);
  const collapsed = collapsedProp ?? collapsedLocal;
  const setCollapsed = (v: boolean) => {
    if (onToggleCollapse) onToggleCollapse();
    else setCollapsedLocal(v);
  };
  const [checked, setChecked] = useState<Record<string, boolean>>({});

  const lagSecs = useMemo(() => {
    if (!latestEndMs || !summary.coveredUntilMs) return 0;
    return Math.max(0, Math.round((latestEndMs - summary.coveredUntilMs) / 1000));
  }, [latestEndMs, summary.coveredUntilMs]);

  const thinking = aiStatus === "thinking";
  // 让人一眼看出纪要覆盖的是「已经过去的那一段」，而不是实时字幕
  const covered = summary.coveredUntilMs > 0 ? formatClock(summary.coveredUntilMs) : "";

  if (collapsed) {
    return (
      <aside className="summary-panel collapsed" data-testid="summary-panel" aria-label="会议纪要">
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
    <aside className="summary-panel" data-testid="summary-panel" aria-label="会议纪要">
      {showHeader ? (
        <header className="panel-head">
          <h2>会议纪要</h2>
          <div className="panel-head-right">
            {covered ? <span className="dim tiny" data-testid="summary-covered">已总结至 {covered}</span> : null}
            {summary.model ? <span className="dim tiny mono">{summary.model}</span> : null}
            <button
              className="icon-btn"
              aria-label="折叠纪要"
              title="折叠"
              onClick={() => setCollapsed(true)}
            >
              <IconChevron dir="right" />
            </button>
          </div>
        </header>
      ) : null}

      {!ready ? (
        <div className="panel-scroll">
          <div className="empty-card" data-testid="summary-not-configured">
            <div className="empty-icon"><IconSparkle size={30} /></div>
            <h4>未配置 AI 接口</h4>
            <p>{notReadyReason || "配置后这里会滚动生成会议纪要。"}</p>
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

            {/* 纪要展示的是**从会议开始到现在的全部内容**，按议题分段。
                之前只有一个「最近一段」，等于把前面讲的全丢了。 */}
            {summary.sections.length ? (
              <section className="block" data-testid="summary-sections">
                {summary.sections.map((sec, i) => (
                  <div className="sum-section" key={`${i}-${sec.title}`}>
                    <h4 className="sum-section-title">
                      {sec.title}
                      {sec.untilMs > 0 ? (
                        <span className="dim tiny mono">{formatClock(sec.untilMs)}</span>
                      ) : null}
                    </h4>
                    <ul className="kv-list">
                      {sec.points.map((p, j) => (
                        <li key={`${j}-${p}`}>{p}</li>
                      ))}
                    </ul>
                  </div>
                ))}
              </section>
            ) : thinking ? (
              <ThinkingSkeleton />
            ) : null}

            {/* 面板只留「边开会边要看」的内容；关键结论/已达成决定/主题等
                仍然照常生成，并且都会出现在导出的会议纪要里，不在这里重复展示。 */}
            {summary.overview.trim() ? (
              <section className="block">
                <h4>会议总览</h4>
                <p className="overview" data-testid="summary-overview">
                  {summary.overview}
                </p>
              </section>
            ) : null}

            {/* 只有在没有分段时才退回平铺要点（老数据/模型没给分段） */}
            {!summary.sections.length && (summary.summary.trim() || summary.keyPoints.length) ? (
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

          {/* 「立即总结」按钮已移除：自动滚动总结本来就够用，
              手动按钮只会让人怀疑「是不是不点就不总结」。
              状态改为一行只读信息，需要时能看到总结进度。 */}
          <footer className="panel-foot">
            <div className="panel-foot-line">
              {lagSecs > 20 ? (
                <span className="lag warn small" data-testid="summary-lag">
                  落后转写 {lagSecs} 秒
                </span>
              ) : null}
              <span className="dim tiny" data-testid="summary-meta">
                {thinking || busy ? "更新中…" : `共更新 ${summary.calls} 次`}
                {summary.updatedAt && !thinking
                  ? ` · ${formatTimeOfDay(summary.updatedAt)}`
                  : ""}
              </span>
            </div>
          </footer>
        </>
      )}
    </aside>
  );
}
