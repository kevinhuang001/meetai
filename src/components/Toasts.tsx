/** 右上角 toast 堆叠 */
import { useStore } from "../store";

const ICON: Record<string, string> = { error: "⚠", info: "ℹ", success: "✓" };

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="toasts" data-testid="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`} role={t.kind === "error" ? "alert" : "status"}>
          <span className="toast-icon" aria-hidden="true">
            {ICON[t.kind] ?? "•"}
          </span>
          <div className="toast-body">
            <div className="toast-scope">{t.scope}</div>
            <div className="toast-msg">{t.message}</div>
          </div>
          <button className="icon-btn" aria-label="关闭提示" onClick={() => dismiss(t.id)}>
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
