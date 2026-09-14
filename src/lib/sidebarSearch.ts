/**
 * 左侧历史栏搜索框的 DOM 引用。
 *
 * Ctrl/Cmd+K 需要把焦点移到搜索框，但按键处理写在 App 里、输入框在 Sidebar 里，
 * 用一个小模块级引用比把 DOM 穿透整棵树更省事。
 */
export const sidebarSearchRef: { current: HTMLInputElement | null } = { current: null };

export function focusSidebarSearch(): boolean {
  const el = sidebarSearchRef.current;
  if (!el) return false;
  el.focus();
  el.select();
  return true;
}
