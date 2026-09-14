/**
 * 左侧历史栏：新建会议 / 搜索 / 历史会话列表 / 设置。
 *
 * 取代了原来只能靠 Ctrl+K 打开的历史视图。Ctrl+K 现在只是把焦点移到这里的搜索框。
 * 折叠状态记在 localStorage；窗口窄于 1100px 时自动折叠成窄条。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import { formatDuration, type SessionListItem } from "../lib/contract";
import { asrServiceLabel } from "../lib/settings";
import { sidebarSearchRef } from "../lib/sidebarSearch";
import { formatDateTime } from "../lib/util";
import { SIDEBAR_NARROW_PX, guard, useStore } from "../store";
import { startRecording } from "../actions";
import { Button } from "./ui";
import { IconPanel, IconPlus, IconSearch } from "./icons";

function statusText(item: SessionListItem): { label: string; tone: string } | null {
  switch (item.status) {
    case "recording":
      return { label: "录音中", tone: "rec" };
    case "paused":
      return { label: "已暂停", tone: "paused" };
    case "error":
      return { label: "出错", tone: "warn" };
    default:
      return null;
  }
}

export function Sidebar() {
  const collapsedPref = useStore((s) => s.sidebarCollapsed);
  const narrow = useStore((s) => s.sidebarNarrow);
  const setSidebarNarrow = useStore((s) => s.setSidebarNarrow);
  const toggleSidebar = useStore((s) => s.toggleSidebar);
  const settings = useStore((s) => s.settings);
  const onboardingOpen = useStore((s) => s.onboardingOpen);
  const live = useStore((s) => s.session);
  const lastSessionId = useStore((s) => s.lastSessionId);
  const view = useStore((s) => s.view);
  const detailId = useStore((s) => s.detailId);
  const setView = useStore((s) => s.setView);
  const openSettings = useStore((s) => s.openSettings);
  const toast = useStore((s) => s.toast);

  const collapsed = collapsedPref || narrow;

  const [items, setItems] = useState<SessionListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);

  /* 窗口宽度：< 1100px 自动折叠 */
  useEffect(() => {
    const apply = () => setSidebarNarrow(window.innerWidth < SIDEBAR_NARROW_PX);
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [setSidebarNarrow]);

  const load = useCallback(async () => {
    const list = await guard("读取历史会话", () => api.listSessions());
    if (list) setItems(list);
    setLoading(false);
  }, []);

  /* 会话开始/结束/换标题时刷新列表（低频事件，不会跟着 10Hz 电平刷新） */
  useEffect(() => {
    void load();
  }, [load, live?.id, live?.status, live?.title, lastSessionId, view]);

  /* Ctrl+K 会把焦点交给这里 */
  useEffect(() => {
    sidebarSearchRef.current = searchRef.current;
    return () => {
      if (sidebarSearchRef.current === searchRef.current) sidebarSearchRef.current = null;
    };
  }, [collapsed]);

  /** 正在录音的会话置顶并高亮：用 store 里的实时会话覆盖列表项 */
  const merged = useMemo(() => {
    const rest = items.filter((x) => !live || x.id !== live.id);
    const list = [...rest].sort((a, b) => b.createdAt - a.createdAt);
    if (!live) return list;
    const item: SessionListItem =
      items.find((x) => x.id === live.id) ??
      {
        id: live.id,
        title: live.title,
        status: live.status,
        createdAt: live.createdAt,
        durationMs: live.durationMs,
        segments: 0,
        chars: 0,
        hasSummary: false,
        audioPath: null,
      };
    return [{ ...item, title: live.title, status: live.status, durationMs: live.durationMs }, ...list];
  }, [items, live]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return merged;
    return merged.filter((i) => i.title.toLowerCase().includes(q) || i.id.toLowerCase().includes(q));
  }, [merged, query]);

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
    // 注意：delete_session 返回 void，guard 成功时同样返回 undefined，
    // 所以这里包一层返回 true 的哨兵，避免「删成功了但界面没更新」。
    const done = await guard("删除会话", async () => {
      await api.deleteSession(item.id);
      return true;
    });
    if (!done) return;
    setItems((prev) => prev.filter((x) => x.id !== item.id));
    setConfirmId(null);
    toast("success", "删除会话", `已删除「${item.title}」`);
  };

  const newMeeting = () => {
    setView("recording");
    void startRecording();
  };

  return (
    <aside className={collapsed ? "sidebar collapsed" : "sidebar"} data-testid="sidebar" data-collapsed={collapsed}>
      <div className="sb-top">
        <Button
          variant="primary"
          onClick={newMeeting}
          testId="sidebar-new-meeting"
          ariaLabel="新建会议"
          title="新建会议（开始录音）"
          className="sb-new"
          disabled={onboardingOpen}
        >
          <IconPlus />
          {collapsed ? null : <span>新建会议</span>}
        </Button>
        <button
          className="icon-btn"
          aria-label={collapsed ? "展开历史栏" : "折叠历史栏"}
          title={narrow ? "窗口较窄自动折叠了，点这里展开" : collapsed ? "展开历史栏（Ctrl/Cmd+K 搜索）" : "折叠历史栏"}
          data-testid="sidebar-toggle"
          onClick={() => {
            // 窄窗口下 collapsed = 用户偏好 || narrow，只翻转偏好是没用的
            //（表现为「点了展开没反应」）。展开时把 narrow 一起清掉。
            if (collapsed) {
              useStore.getState().setSidebarCollapsed(false);
              useStore.getState().setSidebarNarrow(false);
            } else {
              toggleSidebar();
            }
          }}
        >
          <IconPanel />
        </button>
      </div>

      {collapsed ? (
        <div className="sb-collapsed-body">
          <button
            className="sb-icon-row"
            aria-label="搜索历史会话"
            title="搜索历史会话（Ctrl/Cmd+K）"
            data-testid="sidebar-search-icon"
            onClick={() => {
              useStore.getState().setSidebarCollapsed(false);
              useStore.getState().setSidebarNarrow(false);
              window.setTimeout(() => searchRef.current?.focus(), 0);
            }}
          >
            <IconSearch size={16} />
          </button>
          <div className="sb-mini-list">
            {merged.slice(0, 8).map((item) => (
              <button
                key={item.id}
                className={detailId === item.id ? "sb-mini active" : "sb-mini"}
                title={item.title}
                aria-label={item.title}
                data-testid={`sidebar-mini-${item.id}`}
                onClick={() => setView("detail", item.id)}
              >
                {item.status === "recording" ? <i className="rec-dot" aria-hidden="true" /> : null}
                {item.title.slice(0, 1)}
              </button>
            ))}
          </div>
        </div>
      ) : (
        <>
          <div className="sb-search search">
            <span className="search-icon" aria-hidden="true">
              <IconSearch />
            </span>
            <input
              ref={searchRef}
              className="input"
              placeholder="搜索历史会议…"
              aria-label="搜索历史会话"
              data-testid="sidebar-search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>

          <div className="sb-list" data-testid="sidebar-list">
            {loading ? (
              <p className="dim small sb-hint">正在读取历史会话…</p>
            ) : filtered.length === 0 ? (
              <div className="sb-empty" data-testid="sidebar-empty">
                {merged.length === 0 ? (
                  <>
                    <p>还没有会议记录。</p>
                    <p className="dim small">点上面的「＋ 新建会议」开始第一场会议。</p>
                  </>
                ) : (
                  <p className="dim small">没有匹配「{query}」的会议。</p>
                )}
              </div>
            ) : (
              filtered.map((item) => {
                const st = statusText(item);
                const active = view === "detail" && detailId === item.id;
                return (
                  <div
                    key={item.id}
                    className={`sb-row${st?.tone === "rec" ? " recording" : ""}${active ? " active" : ""}`}
                    data-testid={`sidebar-item-${item.id}`}
                  >
                    {renamingId === item.id ? (
                      <input
                        className="input"
                        autoFocus
                        value={draft}
                        aria-label="新的标题"
                        data-testid={`sidebar-rename-input-${item.id}`}
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={() => void commitRename(item)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void commitRename(item);
                          if (e.key === "Escape") setRenamingId(null);
                        }}
                      />
                    ) : (
                      <button
                        className="sb-main"
                        data-testid={`sidebar-open-${item.id}`}
                        title="打开这场会议的详情"
                        onClick={() => setView("detail", item.id)}
                      >
                        <span className="sb-title">
                          {st?.tone === "rec" ? <i className="rec-dot" aria-hidden="true" /> : null}
                          {item.title}
                        </span>
                        <span className="sb-meta">
                          <span>{formatDateTime(item.createdAt)}</span>
                          <span className="mono">{formatDuration(item.durationMs)}</span>
                        </span>
                        <span className="sb-stats">
                          <span>
                            段数 <b className="mono">{item.segments}</b>
                          </span>
                          <span>
                            字数 <b className="mono">{item.chars}</b>
                          </span>
                          <span className={item.hasSummary ? "ok" : "dim"}>
                            {item.hasSummary ? "有纪要" : "无纪要"}
                          </span>
                          {st ? <span className={st.tone === "rec" ? "rec-text" : "dim tiny"}>{st.label}</span> : null}
                        </span>
                      </button>
                    )}

                    <div className="sb-actions">
                      <button
                        className="icon-btn tiny-btn"
                        aria-label={`重命名 ${item.title}`}
                        title="重命名"
                        data-testid={`sidebar-rename-${item.id}`}
                        onClick={() => {
                          setDraft(item.title);
                          setRenamingId(item.id);
                        }}
                      >
                        ✎
                      </button>
                      <button
                        className="icon-btn tiny-btn"
                        aria-label={`删除 ${item.title}`}
                        title="删除"
                        data-testid={`sidebar-delete-${item.id}`}
                        onClick={() => setConfirmId(item.id)}
                      >
                        ✕
                      </button>
                    </div>

                    {confirmId === item.id ? (
                      <div className="sb-confirm" data-testid={`sidebar-confirm-${item.id}`}>
                        <span className="small">删除「{item.title}」？不可恢复。</span>
                        <Button
                          variant="danger"
                          onClick={() => void remove(item)}
                          testId={`sidebar-delete-confirm-${item.id}`}
                        >
                          确认删除
                        </Button>
                        <Button variant="ghost" onClick={() => setConfirmId(null)} testId={`sidebar-cancel-${item.id}`}>
                          取消
                        </Button>
                      </div>
                    ) : null}
                  </div>
                );
              })
            )}
          </div>
        </>
      )}

      <div className="sb-foot">
        <button
          className="sb-foot-btn"
          data-testid="sidebar-settings"
          aria-label="设置"
          title="设置"
          onClick={() => openSettings()}
        >
          <span aria-hidden="true">⚙</span>
          {collapsed ? null : <span>设置</span>}
        </button>
        {collapsed ? null : (
          <span className="sb-service" data-testid="sidebar-asr-service" title={asrServiceLabel(settings)}>
            识别：{asrServiceLabel(settings)}
          </span>
        )}
      </div>
    </aside>
  );
}
