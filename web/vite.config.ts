import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:7777",
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
})
