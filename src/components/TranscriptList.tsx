/**
 * 转写列表。
 *
 * 性能关键：
 *  - 只有 <ActiveRow> 订阅 usePartialStore，未定稿文本的高频变化不会重渲染整个列表；
 *  - 活动行内容增长时通过 ref 回调请求父组件滚动，不触发父组件 setState；
 *  - 用户往上滚动即暂停自动吸底，右下角出现「回到最新」。
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { formatClock, type TranscriptSegment } from "../lib/contract";
import { usePartialStore } from "../store";
import { copyText } from "../lib/util";
import { Button } from "./ui";
import { IconCopy, IconSearch } from "./icons";

const NEAR_BOTTOM_PX = 48;

function ActiveRow({ onGrow }: { onGrow: () => void }) {
  const partial = usePartialStore((s) => s.partial);
  useEffect(() => {
    if (partial) onGrow();
  }, [partial, onGrow]);

  if (!partial) return null;
  const hasText = Boolean(partial.committed || partial.tentative);

  return (
    <div className="t-row active" data-testid="partial-row">
      <span className="t-time mono">{formatClock(partial.startMs)}</span>
      <span className="t-text">
        <span className="committed">{partial.committed}</span>
        <span className="tentative">{partial.tentative}</span>
        {hasText ? <span className="caret" aria-hidden="true" /> : null}
      </span>
    </div>
  );
}

function SegmentRow({ seg, onCopied }: { seg: TranscriptSegment; onCopied: () => void }) {
  // 可疑段（幻听 / 重复输出 / 服务没返回内容）**照常显示**，只是标灰并给一个标记。
  // 绝不静默丢弃：识别出问题的时候，用户更需要看到原始结果，而不是一片空白。
  const suspect = seg.suspect;
  return (
    <div
      className={suspect ? "t-row suspect" : "t-row"}
      data-testid="segment-row"
      data-segment-id={seg.id}
      data-suspect={suspect ? "1" : undefined}
    >
      <span className="t-time mono">{formatClock(seg.startMs)}</span>
      <span className="t-text">{seg.text}</span>
      {suspect ? (
        <span
          className="t-suspect"
          data-testid="segment-suspect"
          title={`${suspect}；仅标注，不会用于生成纪要`}
        >
          {suspect}
        </span>
      ) : null}
      <button
        className="row-copy"
        aria-label={`复制这条转写：${seg.text.slice(0, 12)}`}
        title="复制这一条"
        onClick={() => {
          void copyText(`[${formatClock(seg.startMs)}] ${seg.text}`).then((ok) => {
            if (ok) onCopied();
          });
        }}
      >
        <IconCopy />
      </button>
    </div>
  );
}

export interface TranscriptListProps {
  segments: TranscriptSegment[];
  empty: ReactNode;
  /** 设置里的「自动滚动」总开关 */
  autoScrollEnabled: boolean;
  showFilter?: boolean;
  onNotify?: (msg: string) => void;
  testId?: string;
}

export function TranscriptList({
  segments,
  empty,
  autoScrollEnabled,
  showFilter = true,
  onNotify,
  testId = "transcript-list",
}: TranscriptListProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickRef = useRef(true);
  const [scrolledUp, setScrolledUp] = useState(false);
  const [filter, setFilter] = useState("");

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return segments;
    return segments.filter(
      (s) =>
        s.text.toLowerCase().includes(q) || formatClock(s.startMs).includes(q),
    );
  }, [segments, filter]);

  const filtering = filter.trim().length > 0;
  // 可疑段计数：让「识别质量有问题」这件事在界面上被看见，而不是让人以为程序没工作
  const suspectCount = useMemo(() => segments.filter((s) => s.suspect).length, [segments]);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    stickRef.current = true;
    setScrolledUp(false);
  }, []);

  const maybeScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (!autoScrollEnabled || filtering || !stickRef.current) return;
    // 再按「当前位置」确认一次：用户刚上滚时 scroll 事件可能还没处理完，
    // 此刻 stickRef 仍是 true，直接吸底会把用户的滚动立刻顶回底部
    //（表现为「回到最新」按钮时有时无）。
    if (el.scrollHeight - el.scrollTop - el.clientHeight > NEAR_BOTTOM_PX) {
      stickRef.current = false;
      setScrolledUp(true);
      return;
    }
    el.scrollTop = el.scrollHeight;
  }, [autoScrollEnabled, filtering]);

  // 新定稿的句子到达时吸底
  useEffect(() => {
    maybeScroll();
  }, [segments.length, maybeScroll]);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_PX;
    stickRef.current = near;
    setScrolledUp((prev) => (prev === !near ? prev : !near));
  }, []);

  const copyAll = useCallback(() => {
    const body = filtered
      .map((s) => `[${formatClock(s.startMs)}] ${s.text}`)
      .join("\n");
    void copyText(body).then((ok) => onNotify?.(ok ? `已复制 ${filtered.length} 条转写` : "复制失败，请检查剪贴板权限"));
  }, [filtered, onNotify]);

  return (
    <div className="transcript">
      {showFilter ? (
        <div className="transcript-toolbar">
          <div className="search">
            <span className="search-icon" aria-hidden="true">
              <IconSearch />
            </span>
            <input
              className="input"
              placeholder="搜索转写内容…"
              aria-label="搜索转写内容"
              data-testid="transcript-filter"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            {filtering ? (
              <button className="icon-btn" aria-label="清空搜索" onClick={() => setFilter("")}>
                ✕
              </button>
            ) : null}
          </div>
          <span className="dim small" data-testid="transcript-count">
            {filtering ? `${filtered.length} / ${segments.length} 条` : `${segments.length} 条`}
          </span>
          {suspectCount > 0 ? (
            <span
              className="t-suspect"
              data-testid="transcript-suspect-count"
              title="这些段落的识别结果可疑（幻听/重复/服务没返回内容）。它们照常显示，但不会用于生成纪要。"
            >
              {suspectCount} 条可疑
            </span>
          ) : null}
          <button
            className="link-btn"
            onClick={copyAll}
            disabled={filtered.length === 0}
            data-testid="copy-all"
            aria-label="复制全部转写"
          >
            复制全部
          </button>
        </div>
      ) : null}

      <div className="transcript-scroll" ref={scrollRef} onScroll={handleScroll} data-testid={testId}>
        {segments.length === 0 && !filtering ? (
          empty
        ) : filtered.length === 0 ? (
          <div className="empty-inline">没有匹配「{filter}」的转写</div>
        ) : (
          <div className="t-body">
            {filtered.map((s) => (
              <SegmentRow key={s.id} seg={s} onCopied={() => onNotify?.("已复制这条转写")} />
            ))}
            <ActiveRow onGrow={maybeScroll} />
          </div>
        )}
      </div>

      {filtering ? (
        <div className="scroll-hint" data-testid="filter-hint">
          过滤中已暂停自动滚动
        </div>
      ) : null}

      {scrolledUp ? (
        <Button
          variant="primary"
          className="jump-latest"
          onClick={scrollToBottom}
          testId="jump-to-latest"
          ariaLabel="回到最新转写"
        >
          回到最新 ↓
        </Button>
      ) : null}
    </div>
  );
}
