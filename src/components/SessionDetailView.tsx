/** 会话详情：完整转写 + 纪要面板 + 完整纪要（markdown）+ 导出 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import { emptySummary, formatDuration, type ExportFormat, type SessionDetail } from "../lib/contract";
import { formatDateTime } from "../lib/util";
import { guard, useStore } from "../store";
import { exportSession, generateReport, summarizeNow } from "../actions";
import { TranscriptList } from "./TranscriptList";
import { SummaryPanel } from "./SummaryPanel";
import { Markdown } from "../lib/markdown";
import { aiReadiness, asrServiceLabel } from "../lib/settings";
import { Button } from "./ui";

const FORMATS: ExportFormat[] = ["md", "txt", "srt", "json"];

/**
 * 以接口返回为准；但若后端因为持久化延迟返回了空的转写，
 * 而本地刚拿到过这场会议的完整详情（停止录音时的返回值），就用本地缓存补全，
 * 避免「刚结束的会议点进详情是空白」。
 */
function pickDetail(remote: SessionDetail, cached: SessionDetail | null): SessionDetail {
  if (!cached || cached.id !== remote.id) return remote;
  if (remote.segments.length >= cached.segments.length) return remote;
  return {
    ...remote,
    segments: cached.segments,
    summary: remote.summary.calls > 0 ? remote.summary : cached.summary,
    stats: remote.stats.chars > 0 ? remote.stats : cached.stats,
  };
}

export function SessionDetailView() {
  const detailId = useStore((s) => s.detailId);
  const setView = useStore((s) => s.setView);
  const settings = useStore((s) => s.settings);
  const toast = useStore((s) => s.toast);

  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [report, setReport] = useState<string | null>(null);
  const [reportBusy, setReportBusy] = useState(false);

  const liveSessionId = useStore((s) => s.session?.id ?? null);
  const liveSegments = useStore((s) => s.segments);
  const liveSummary = useStore((s) => s.summary);
  const summarizing = useStore((s) => s.summarizing);

  const load = useCallback(async () => {
    if (!detailId) return;
    setLoading(true);
    const cached = useStore.getState().lastDetail;
    if (cached && cached.id === detailId) setDetail(cached);
    const d = await guard("读取会话详情", () => api.getSession(detailId));
    if (d) {
      const merged = pickDetail(d, cached);
      setDetail(merged);
      setReport(merged.reportMd);
    }
    setLoading(false);
  }, [detailId]);

  useEffect(() => {
    void load();
  }, [load]);

  const isLive = liveSessionId !== null && liveSessionId === detailId;
  const segments = isLive ? liveSegments : (detail?.segments ?? []);
  const summary = isLive ? liveSummary : (detail?.summary ?? emptySummary());
  const latestEndMs = segments.length ? segments[segments.length - 1].endMs : 0;
  const readiness = useMemo(() => aiReadiness(settings), [settings]);

  const makeReport = async () => {
    if (!detailId) return;
    setReportBusy(true);
    const md = await generateReport(detailId);
    if (md) setReport(md);
    setReportBusy(false);
  };

  if (!detailId) {
    return (
      <div className="history">
        <div className="empty-inline">没有选中的会话</div>
      </div>
    );
  }

  return (
    <>
      <div className="stage detail">
        <header className="detail-head">
          <div className="detail-head-left">
            <Button variant="ghost" onClick={() => setView("history")} testId="detail-back" ariaLabel="返回历史会话">
              ← 返回
            </Button>
            <div className="detail-title">
              <h1 data-testid="detail-title">{detail?.title ?? "加载中…"}</h1>
              <p className="dim small">
                {detail ? `${formatDateTime(detail.createdAt)} · 时长 ${formatDuration(detail.durationMs)} · ${segments.length} 段 · ${detail.stats.chars} 字` : ""}
                {isLive ? " · 进行中" : ""}
              </p>
              <p className="dim tiny" data-testid="detail-asr-service">
                识别服务：<span className="mono">{detail?.config.modelId ?? asrServiceLabel(settings)}</span>
              </p>
            </div>
          </div>
          <div className="detail-head-right">
            <Button variant="default" onClick={() => void makeReport()} disabled={reportBusy} testId="generate-report" ariaLabel="生成完整纪要">
              {reportBusy ? "生成中…" : "✦ 生成完整纪要"}
            </Button>
            <div className="export-group" role="group" aria-label="导出会议记录">
              {FORMATS.map((f) => (
                <Button
                  key={f}
                  variant="ghost"
                  onClick={() => void exportSession(detailId, f, detail?.title ?? "meeting")}
                  testId={`export-${f}`}
                  ariaLabel={`导出 ${f.toUpperCase()}`}
                >
                  {f.toUpperCase()}
                </Button>
              ))}
            </div>
          </div>
        </header>

        <div className="detail-split">
          <section className="detail-transcript">
            <h2 className="split-title">完整转写</h2>
            <TranscriptList
              segments={segments}
              empty={<div className="empty-inline">这次会话没有转写内容</div>}
              autoScrollEnabled={false}
              onNotify={(m) => toast("success", "复制", m)}
              testId="detail-transcript-list"
            />
          </section>

          <section className="detail-report">
            <h2 className="split-title">完整会议纪要</h2>
            <div className="report-body" data-testid="report-body">
              {loading ? (
                <p className="dim small">正在加载…</p>
              ) : report ? (
                <Markdown text={report} />
              ) : (
                <div className="empty-inline">
                  还没有完整纪要，点击右上角「生成完整纪要」让 AI 汇总整场会议。
                </div>
              )}
            </div>
          </section>
        </div>
      </div>

      <SummaryPanel
        summary={summary}
        aiStatus="idle"
        aiMessage={null}
        latestEndMs={latestEndMs}
        ready={readiness.ready}
        notReadyReason={readiness.reason}
        onSummarizeNow={() => void summarizeNow(detailId).then(() => window.setTimeout(() => void load(), 2500))}
        onOpenAiSettings={() => useStore.getState().openSettings("ai")}
        busy={summarizing}
      />
    </>
  );
}
