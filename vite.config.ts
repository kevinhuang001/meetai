import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望前端固定端口，且不要清屏（保留 Rust 侧日志）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // 显式绑定 IPv4。
    // 用默认的 host:false 时，vite 会去解析 localhost，绑到 127.0.0.1 还是 ::1
    // 取决于操作系统的解析顺序 —— 在 GitHub Actions 的 runner 上就绑到了 ::1，
    // 导致按 127.0.0.1 探测的冒烟测试永远连不上（vite 却报告已就绪）。
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: false,
    chunkSizeWarningLimit: 1500,
  },
});
