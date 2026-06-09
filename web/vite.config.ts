import { defineConfig, loadEnv } from "vite"
import react from "@vitejs/plugin-react"

// API target + dev port are env-configurable so the same frontend can validate
// against different backends: the demo run server (default 7777) or a real
// source-repo codegraph API (e.g. MAESTRO_API_TARGET=http://127.0.0.1:7780).
// loadEnv (not process.env) keeps this typecheck-clean without @types/node — it
// merges in any MAESTRO_-prefixed shell vars.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "MAESTRO_")
  const apiTarget = env.MAESTRO_API_TARGET || "http://127.0.0.1:7777"
  const devPort = Number(env.MAESTRO_WEB_PORT) || 5173

  return {
    plugins: [react()],
    server: {
      port: devPort,
      proxy: {
        "/api": {
          target: apiTarget,
          changeOrigin: true,
          ws: false,
        },
      },
    },
    build: {
      outDir: "dist",
      emptyOutDir: true,
      target: "es2020",
      rollupOptions: {
        output: {
          manualChunks: {
            vendor: ["react", "react-dom"],
            graph: ["@xyflow/react", "@dagrejs/dagre"],
            markdown: ["react-markdown", "remark-gfm", "rehype-highlight"],
            codemirror: [
              "@codemirror/lang-markdown",
              "@codemirror/lang-yaml",
              "@codemirror/language-data",
              "@codemirror/view",
              "@uiw/codemirror-theme-vscode",
              "@uiw/react-codemirror",
            ],
          },
        },
      },
    },
  }
})
