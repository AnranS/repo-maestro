/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      fontFamily: {
        sans: [
          "-apple-system",
          "BlinkMacSystemFont",
          "Inter",
          "Segoe UI",
          "Roboto",
          "sans-serif",
        ],
        mono: [
          "ui-monospace",
          "SFMono-Regular",
          "Menlo",
          "JetBrains Mono",
          "monospace",
        ],
      },
      colors: {
        bg: {
          DEFAULT: "rgb(var(--color-bg) / <alpha-value>)",
          soft: "rgb(var(--color-bg-soft) / <alpha-value>)",
          panel: "rgb(var(--color-bg-panel) / <alpha-value>)",
          inset: "rgb(var(--color-bg-inset) / <alpha-value>)",
          hover: "rgb(var(--color-bg-hover) / <alpha-value>)",
        },
        line: {
          DEFAULT: "rgb(var(--color-line) / <alpha-value>)",
          soft: "rgb(var(--color-line-soft) / <alpha-value>)",
        },
        accent: {
          DEFAULT: "rgb(var(--color-accent) / <alpha-value>)",
          soft: "rgb(var(--color-accent-soft) / <alpha-value>)",
          muted: "rgb(var(--color-accent-muted) / <alpha-value>)",
        },
        ink: {
          DEFAULT: "rgb(var(--color-ink) / <alpha-value>)",
          dim: "rgb(var(--color-ink-dim) / <alpha-value>)",
          mute: "rgb(var(--color-ink-mute) / <alpha-value>)",
          faint: "rgb(var(--color-ink-faint) / <alpha-value>)",
        },
        // F-135: theme-aware status foreground tokens (dark == the old -300
        // shade, light == -700). Use text-status-* for status TEXT; the
        // translucent chip bg-*-500/15 backgrounds stay theme-neutral.
        status: {
          success: "rgb(var(--color-status-success) / <alpha-value>)",
          warning: "rgb(var(--color-status-warning) / <alpha-value>)",
          danger: "rgb(var(--color-status-danger) / <alpha-value>)",
          info: "rgb(var(--color-status-info) / <alpha-value>)",
        },
      },
      keyframes: {
        "spin-slow": {
          to: { transform: "rotate(360deg)" },
        },
      },
      animation: {
        "pulse-fast": "pulse 1s cubic-bezier(0.4, 0, 0.6, 1) infinite",
        "spin-slow": "spin-slow 1.4s linear infinite",
      },
      // F-UI-003: named z-index tiers. Numeric values mirror the pre-existing
      // stacking order so migration can be semantic without changing layering.
      zIndex: {
        sticky: "10",
        dropdown: "20",
        scrim: "30",
        popover: "30",
        drawer: "40",
        modal: "50",
        "nested-modal": "60",
        tooltip: "70",
      },
      boxShadow: {
        overlay: "0 18px 42px -28px rgb(var(--shadow-overlay) / 0.75)",
      },
    },
  },
  plugins: [],
}
