/** 顶栏：应用名、可编辑会议标题、状态胶囊、计时器、设置/历史入口 */
import { useEffect, useRef, useState } from "react";
import { formatClock } from "../lib/contract";
import { api } from "../lib/api";
import { guard, useLevelStore, useStore } from "../store";

function statusOf(state: string, hasSession: boolean): { label: string; tone: string } {
  if (!hasSession && (state === "idle" || state === "")) return { label: "空闲", tone: "idle" };
  switch (state) {
    case "starting":
      return { label: "启动中", tone: "busy" };
    case "listening":
    case "speech":
      return { label: "录音中", tone: "rec" };
    case "paused":
      return { label: "已暂停", tone: "paused" };
    case "stopping":
      return { label: "正在停止", tone: "busy" };
    case "error":
      return { label: "出错", tone: "error" };
    default:
      return { label: "空闲", tone: "idle" };
  }
}

/** 计时器单独订阅 10Hz 的电平 store，避免顶栏整体重渲染 */
function SessionTimer() {
  const durationMs = useLevelStore((s) => s.durationMs);
  return (
    <span className="timer mono" data-testid="timer" aria-label="录音时长">
      {formatClock(durationMs)}
    </span>
  );
}

export function TopBar() {
  const session = useStore((s) => s.session);
  const asrState = useStore((s) => s.asrState);
  const pendingTitle = useStore((s) => s.pendingTitle);
  const setPendingTitle = useStore((s) => s.setPendingTitle);
  const patchSession = useStore((s) => s.patchSession);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const title = session?.title ?? pendingTitle;

  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const status = statusOf(asrState, Boolean(session));

  const commit = async () => {
    const next = draft.trim();
    setEditing(false);
    if (!next || next === title) return;
    if (session) {
      const info = await guard("重命名会议", () => api.renameSession(session.id, next));
      if (info) patchSession({ title: info.title ?? next });
    } else {
      setPendingTitle(next);
    }
  };

  return (
    <header className="topbar">
      {/* 品牌名不再常驻：窗口标题栏已经写明，界面里重复一遍只是噪音 */}
      <span className="logo" aria-hidden="true" title="MeetingHear">
        ◉
      </span>

      <div className="title-zone">
        {editing ? (
          <input
            ref={inputRef}
            className="title-input"
            value={draft}
            aria-label="会议标题"
            data-testid="title-input"
            onChange={(e) => setDraft(e.target.value)}
            onBlur={() => void commit()}
            onKeyDown={(e) => {
              if (e.key === "Enter") void commit();
              if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <button
            className="title-btn"
            data-testid="title-button"
            aria-label="编辑会议标题"
            title="点击修改会议标题"
            onClick={() => {
              setDraft(title || "");
              setEditing(true);
            }}
          >
            {title || "未命名会议"}
            <span className="edit-pen" aria-hidden="true">
              ✎
            </span>
          </button>
        )}
        <span className={`status-pill ${status.tone}`} data-testid="status-pill">
          {status.tone === "rec" ? <i className="rec-dot" aria-hidden="true" /> : null}
          {status.label}
        </span>
        <SessionTimer />
      </div>

      {/* 右上角不再放折叠按钮：左侧历史栏顶部已经有一个，
          两处并存的后果是用户根本分不清哪个管哪块。 */}
      <div className="topbar-right" />
    </header>
  );
}
