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
          DEFAULT: "#0a0a0a",
          soft: "#111111",
          panel: "#161616",
          inset: "#0d0d0d",
          hover: "#1c1c1c",
        },
        line: {
          DEFAULT: "#1f1f1f",
          soft: "#262626",
        },
        accent: {
          DEFAULT: "#60a5fa",
          soft: "#93c5fd",
          muted: "#1d4ed8",
        },
        ink: {
          DEFAULT: "#ededed",
          dim: "#a3a3a3",
          mute: "#737373",
          faint: "#525252",
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
    },
  },
  plugins: [],
}
