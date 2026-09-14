/** 录制主视图：转写区 + AI 纪要面板 */
import { useMemo } from "react";
import { useStore } from "../store";
import { aiReadiness } from "../lib/settings";
import { summarizeNow } from "../actions";
import { TranscriptList } from "./TranscriptList";
import { SummaryPanel } from "./SummaryPanel";
import { Markdown } from "../lib/markdown";
import { Button } from "./ui";
import { IconMic } from "./icons";

function EmptyTranscript() {
  return (
    <div className="empty-state" data-testid="transcript-empty">
      <div className="empty-icon" aria-hidden="true">
        <IconMic size={44} />
      </div>
      <h3>还没有转写内容</h3>
      <p>点「开始录音」，或导入一个音频文件。</p>
      {/* 隐私与使用说明只在首次配置向导里讲一次，主界面不再重复三行提示 */}
      <div className="empty-actions">
        <span className="kbd-hint">
          <kbd>Ctrl</kbd>/<kbd>⌘</kbd> + <kbd>Enter</kbd> 开始录音
        </span>
      </div>
    </div>
  );
}

export function RecordingView() {
  const segments = useStore((s) => s.segments);
  const summary = useStore((s) => s.summary);
  const aiStatus = useStore((s) => s.aiStatus);
  const aiMessage = useStore((s) => s.aiMessage);
  const settings = useStore((s) => s.settings);
  const session = useStore((s) => s.session);
  const asrMessage = useStore((s) => s.asrMessage);
  const finalReport = useStore((s) => s.finalReport);
  const lastSessionId = useStore((s) => s.lastSessionId);
  const summarizing = useStore((s) => s.summarizing);
  const setView = useStore((s) => s.setView);
  const toast = useStore((s) => s.toast);

  const readiness = useMemo(() => aiReadiness(settings), [settings]);
  const latestEndMs = segments.length ? segments[segments.length - 1].endMs : 0;
  const autoScroll = settings?.general.autoScroll ?? true;

  return (
    <>
      <div className="stage">
        {asrMessage ? (
          <div className="status-banner" data-testid="asr-banner">
            <span className="spinner" aria-hidden="true" />
            <span>{asrMessage}</span>
          </div>
        ) : null}

        {!session && segments.length > 0 ? (
          <div className="done-banner" data-testid="finished-banner">
            <span>本次录音已结束，共 {segments.length} 条转写。</span>
            {lastSessionId ? (
              <Button variant="ghost" onClick={() => setView("detail", lastSessionId)} testId="goto-detail">
                查看会议详情 →
              </Button>
            ) : null}
          </div>
        ) : null}

        <TranscriptList
          segments={segments}
          empty={<EmptyTranscript />}
          autoScrollEnabled={autoScroll}
          onNotify={(m) => toast("success", "复制", m)}
        />

        {finalReport ? (
          <section className="report-card" data-testid="final-report">
            <header>
              <h3>完整会议纪要</h3>
              <button
                className="icon-btn"
                aria-label="关闭完整纪要"
                onClick={() => useStore.getState().setFinalReport(null)}
              >
                ✕
              </button>
            </header>
            <div className="report-body">
              <Markdown text={finalReport} />
            </div>
          </section>
        ) : null}
      </div>

      <SummaryPanel
        summary={summary}
        aiStatus={aiStatus}
        aiMessage={aiMessage}
        latestEndMs={latestEndMs}
        ready={readiness.ready}
        notReadyReason={readiness.reason}
        onSummarizeNow={() => void summarizeNow(session?.id ?? lastSessionId ?? null)}
        onOpenAiSettings={() => useStore.getState().openSettings("ai")}
        busy={summarizing}
      />
    </>
  );
}
