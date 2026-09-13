/** 主题与字号的应用（作用到 <html>） */
import type { Settings } from "./contract";

export function resolvedTheme(theme: "dark" | "light" | "system"): "dark" | "light" {
  if (theme !== "system") return theme;
  if (typeof window === "undefined" || !window.matchMedia) return "dark";
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function applyAppearance(settings: Settings | null): void {
  const root = document.documentElement;
  const theme = resolvedTheme(settings?.general.theme ?? "dark");
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
  const scale = Math.min(1.6, Math.max(0.8, settings?.general.fontScale ?? 1));
  root.style.fontSize = `${Math.round(16 * scale * 100) / 100}px`;
}

/** 跟随系统主题变化 */
export function watchSystemTheme(handler: () => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => undefined;
  const mq = window.matchMedia("(prefers-color-scheme: light)");
  mq.addEventListener("change", handler);
  return () => mq.removeEventListener("change", handler);
}
