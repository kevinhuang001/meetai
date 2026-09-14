/** 内联 SVG 图标：避免依赖系统 emoji / 符号字体（不同平台字形差异很大） */
import type { ReactNode } from "react";

function Svg({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      style={{ display: "block", flex: "none" }}
    >
      {children}
    </svg>
  );
}

export function IconMic({ size = 40 }: { size?: number }) {
  return (
    <Svg size={size}>
      <rect x="9" y="2" width="6" height="12" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0" />
      <path d="M12 18v4" />
      <path d="M8 22h8" />
    </Svg>
  );
}

export function IconFolder({ size = 16 }: { size?: number }) {
  return (
    <Svg size={size}>
      <path d="M3 7a2 2 0 0 1 2-2h3.5l2 2.5H19a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </Svg>
  );
}

export function IconSearch({ size = 14 }: { size?: number }) {
  return (
    <Svg size={size}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M16 16l4.5 4.5" />
    </Svg>
  );
}

export function IconCopy({ size = 14 }: { size?: number }) {
  return (
    <Svg size={size}>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M6 15H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v1" />
    </Svg>
  );
}

export function IconSparkle({ size = 18 }: { size?: number }) {
  return (
    <Svg size={size}>
      <path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z" />
      <path d="M18.5 16.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z" />
    </Svg>
  );
}

export function IconChevron({ dir, size = 16 }: { dir: "left" | "right"; size?: number }) {
  return (
    <Svg size={size}>
      {dir === "left" ? <path d="M14.5 6l-6 6 6 6" /> : <path d="M9.5 6l6 6-6 6" />}
    </Svg>
  );
}

/** 左侧历史栏的折叠 / 展开 */
export function IconPanel({ size = 16 }: { size?: number }) {
  return (
    <Svg size={size}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9.5 4v16" />
      <path d="M5.5 8h1.5M5.5 11.5h1.5" />
    </Svg>
  );
}

export function IconPlus({ size = 15 }: { size?: number }) {
  return (
    <Svg size={size}>
      <path d="M12 5v14M5 12h14" />
    </Svg>
  );
}
