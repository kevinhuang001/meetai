/** 右侧「AI 实时纪要」面板：录制视图与会话详情视图共用 */
import { useMemo, useState } from "react";
import { formatClock, type SummaryState } from "../lib/contract";
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

export function SummaryPanel({
  summary,
  aiStatus,
  aiMessage,
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
            <p>{notReadyReason || "配置一个 OpenAI 兼容的服务商后，这里会实时生成会议纪要。"}</p>
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
                <>
                  <div className="live-thinking">AI 正在总结…</div>
                  <ThinkingSkeleton />
                </>
              ) : (
                <p className="live-text" data-testid="summary-live">
                  {summary.live || "还没有可以总结的内容，开始说话后这里会实时更新。"}
                </p>
              )}
            </section>

            <section className="block">
              <h4>会议总览</h4>
              <p className="overview" data-testid="summary-overview">
                {summary.overview || "暂无"}
              </p>
            </section>

            <section className="block">
              <h4>
                关键要点 <Badge>{summary.keyPoints.length}</Badge>
              </h4>
              {summary.keyPoints.length ? (
                <ul className="kv-list" data-testid="summary-keypoints">
                  {summary.keyPoints.map((k, i) => (
                    <li key={`${i}-${k}`}>{k}</li>
                  ))}
                </ul>
              ) : (
                <p className="dim small">暂无</p>
              )}
            </section>

            <section className="block">
              <h4>
                待办事项 <Badge tone="accent">{summary.actionItems.length}</Badge>
              </h4>
              {summary.actionItems.length ? (
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
              ) : (
                <p className="dim small">暂无</p>
              )}
            </section>

            <section className="block">
              <h4>已达成的决定</h4>
              {summary.decisions.length ? (
                <ul className="kv-list decision" data-testid="summary-decisions">
                  {summary.decisions.map((d, i) => (
                    <li key={`${i}-${d}`}>{d}</li>
                  ))}
                </ul>
              ) : (
                <p className="dim small">暂无</p>
              )}
            </section>

            <section className="block">
              <h4>当前主题</h4>
              {summary.topics.length ? (
                <div className="tags" data-testid="summary-topics">
                  {summary.topics.map((t, i) => (
                    <span className="tag" key={`${i}-${t}`}>
                      {t}
                    </span>
                  ))}
                </div>
              ) : (
                <p className="dim small">暂无</p>
              )}
            </section>
          </div>

          <footer className="panel-foot">
            <div className="foot-line">
              <span className="dim small" data-testid="summary-updated">
                更新时间 {summary.updatedAt ? formatTimeOfDay(summary.updatedAt) : "—"}
              </span>
              <span className="dim small">
                覆盖至 {summary.coveredUntilMs ? formatClock(summary.coveredUntilMs) : "—"}
              </span>
            </div>
            <div className="foot-line">
              <span className={lagSecs > 20 ? "lag warn small" : "lag small"} data-testid="summary-lag">
                {lagSecs > 0 ? `纪要进度落后转写 ${lagSecs} 秒` : "纪要已跟上转写"}
              </span>
              {summary.calls > 0 ? (
                <span className="dim tiny">第 {summary.calls} 次总结</span>
              ) : null}
            </div>
            {aiMessage ? <div className="dim tiny">{aiMessage}</div> : null}
            <Button
              variant="primary"
              onClick={onSummarizeNow}
              disabled={busy || thinking}
              testId="summarize-now"
              className="block-btn"
            >
              {busy || thinking ? "总结中…" : "立即总结"}
            </Button>
          </footer>
        </>
      )}
    </aside>
  );
}
