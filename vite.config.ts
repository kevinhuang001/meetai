import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望前端固定端口，且不要清屏（保留 Rust 侧日志）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: false,
    chunkSizeWarningLimit: 1500,
  },
});
