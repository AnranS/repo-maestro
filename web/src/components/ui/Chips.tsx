// F-UI-003 Slice 1: small shared chips so provenance/size affordances look
// identical everywhere, and risk renders through the single status-token source
// (no hand-mixed colors in business components).

import type { ReactNode } from "react"
import { statusToken } from "./StatusChip"

function MetaChip({
  children,
  tone = "neutral",
}: {
  children: ReactNode
  tone?: "neutral" | "warn"
}) {
  const cls =
    tone === "warn"
      ? "bg-amber-500/10 text-status-warning/80 border-amber-500/20"
      : "bg-bg-hover text-ink-faint border-line"
  return (
    <span className={`inline-flex items-center rounded border px-1 text-[10px] ${cls}`}>
      {children}
    </span>
  )
}

export const RedactedChip = () => <MetaChip tone="warn">redacted</MetaChip>
export const TruncatedChip = () => <MetaChip tone="warn">truncated</MetaChip>
export const OmittedChip = ({ reason }: { reason?: string }) => (
  <MetaChip>omitted{reason ? `: ${reason}` : ""}</MetaChip>
)

/** Risk level → status-token key (F-UI-003 §8.4). high → risk_high (amber),
 *  medium → blocked (amber), low → severity_low (muted). Low AND unknown/blank
 *  are muted — NEVER `done`/green, so an unknown risk is never misread as
 *  "safe". Pure + exported so the mapping is reviewable in isolation. */
export function riskTokenKey(level: string): string {
  switch (level) {
    case "high":
      return "risk_high"
    case "medium":
      return "blocked"
    case "low":
      return "severity_low"
    default:
      return "idle"
  }
}

export function RiskChip({ level }: { level: string }) {
  const tok = statusToken(riskTokenKey(level))
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ${tok.chip}`}
    >
      {level ? `${level} risk` : "unknown risk"}
    </span>
  )
}
