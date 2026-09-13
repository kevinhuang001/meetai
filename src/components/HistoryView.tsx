/** 历史会话视图 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import { formatDuration, type SessionListItem } from "../lib/contract";
import { formatDateTime } from "../lib/util";
import { guard, useStore } from "../store";
import { Badge, Button } from "./ui";
import { IconFolder, IconSearch } from "./icons";

function StatusBadge({ item }: { item: SessionListItem }) {
  if (item.status === "recording") return <Badge tone="accent">进行中</Badge>;
  if (item.status === "paused") return <Badge tone="warn">已暂停</Badge>;
  if (item.status === "error") return <Badge tone="warn">出错</Badge>;
  return <Badge>已结束</Badge>;
}

export function HistoryView() {
  const setView = useStore((s) => s.setView);
  const toast = useStore((s) => s.toast);

  const [items, setItems] = useState<SessionListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmId, setConfirmId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    const list = await guard("读取历史会话", () => api.listSessions());
    setItems(list ?? []);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return items;
    return items.filter((i) => i.title.toLowerCase().includes(q) || i.id.toLowerCase().includes(q));
  }, [items, query]);

  const commitRename = async (item: SessionListItem) => {
    const next = draft.trim();
    setRenamingId(null);
    if (!next || next === item.title) return;
    const info = await guard("重命名会话", () => api.renameSession(item.id, next));
    if (info) {
      setItems((prev) => prev.map((x) => (x.id === item.id ? { ...x, title: next } : x)));
      toast("success", "重命名", `已改为「${next}」`);
    }
  };

  const remove = async (item: SessionListItem) => {
    const r = await guard("删除会话", () => api.deleteSession(item.id));
    if (r === undefined) return;
    setItems((prev) => prev.filter((x) => x.id !== item.id));
    setConfirmId(null);
    toast("success", "删除会话", `已删除「${item.title}」`);
  };

  return (
    <div className="history" data-testid="history-view">
      <header className="view-head">
        <div>
          <h1>历史会话</h1>
          <p className="dim small">共 {items.length} 条记录，点击卡片查看完整转写与纪要</p>
        </div>
        <div className="view-head-right">
          <div className="search">
            <span className="search-icon" aria-hidden="true">
              <IconSearch />
            </span>
            <input
              className="input"
              placeholder="搜索标题…"
              aria-label="搜索历史会话"
              data-testid="history-search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <Button variant="ghost" onClick={() => void load()} testId="history-refresh" ariaLabel="刷新历史会话">
            ⟳ 刷新
          </Button>
          <Button variant="primary" onClick={() => setView("recording")} testId="history-back" ariaLabel="返回录制">
            返回录制
          </Button>
        </div>
      </header>

      {loading ? (
        <div className="empty-inline">正在读取历史会话…</div>
      ) : filtered.length === 0 ? (
        <div className="empty-state" data-testid="history-empty">
          <div className="empty-icon" aria-hidden="true">
            <IconFolder size={36} />
          </div>
          <h3>{items.length === 0 ? "还没有历史会话" : "没有匹配的会话"}</h3>
          <p>
            {items.length === 0
              ? "开始一次录音，结束后会自动保存到这里。"
              : "换一个关键字试试，或者清空搜索框。"}
          </p>
          {items.length === 0 ? (
            <Button variant="primary" onClick={() => setView("recording")}>
              去开始录音
            </Button>
          ) : null}
        </div>
      ) : (
        <div className="session-grid" data-testid="session-list">
          {filtered.map((item) => (
            <article className="session-card" key={item.id} data-testid={`session-card-${item.id}`}>
              <div className="card-top">
                {renamingId === item.id ? (
                  <input
                    className="input"
                    autoFocus
                    value={draft}
                    aria-label="新的标题"
                    onChange={(e) => setDraft(e.target.value)}
                    onBlur={() => void commitRename(item)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void commitRename(item);
                      if (e.key === "Escape") setRenamingId(null);
                    }}
                  />
                ) : (
                  <button
                    className="card-title"
                    data-testid={`session-open-${item.id}`}
                    onClick={() => setView("detail", item.id)}
                    title="查看详情"
                  >
                    {item.title}
                  </button>
                )}
                <StatusBadge item={item} />
              </div>

              <div className="card-meta">
                <span>{formatDateTime(item.createdAt)}</span>
                <span className="mono">{formatDuration(item.durationMs)}</span>
              </div>

              <div className="card-stats">
                <span>
                  段数 <b className="mono">{item.segments}</b>
                </span>
                <span>
                  字数 <b className="mono">{item.chars}</b>
                </span>
                <span className={item.hasSummary ? "ok" : "dim"}>{item.hasSummary ? "有纪要" : "无纪要"}</span>
                <span className={item.audioPath ? "ok" : "dim"}>{item.audioPath ? "有录音" : "无录音"}</span>
              </div>

              <div className="card-actions">
                <Button variant="ghost" onClick={() => setView("detail", item.id)} ariaLabel={`打开 ${item.title}`}>
                  打开
                </Button>
                <Button
                  variant="ghost"
                  onClick={() => {
                    setDraft(item.title);
                    setRenamingId(item.id);
                  }}
                  testId={`session-rename-${item.id}`}
                  ariaLabel={`重命名 ${item.title}`}
                >
                  重命名
                </Button>
                {confirmId === item.id ? (
                  <>
                    <Button variant="danger" onClick={() => void remove(item)} testId={`session-delete-confirm-${item.id}`}>
                      确认删除
                    </Button>
                    <Button variant="ghost" onClick={() => setConfirmId(null)}>
                      取消
                    </Button>
                  </>
                ) : (
                  <Button
                    variant="ghost"
                    onClick={() => setConfirmId(item.id)}
                    testId={`session-delete-${item.id}`}
                    ariaLabel={`删除 ${item.title}`}
                  >
                    删除
                  </Button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
    </div>
  );
}
