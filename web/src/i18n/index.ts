/**
 * Minimal i18n layer. We deliberately avoid pulling in i18next / react-intl
 * for ~3KB of dependency just to flip ~150 short strings. The shape:
 *
 *   - `Lang` is the set of supported codes (`en`, `zh`).
 *   - `Catalog` is a flat dictionary of `key -> string` per language.
 *   - `t(key)` is a *snapshot* lookup using whatever language is currently
 *     active. It's not reactive on its own — components should subscribe via
 *     `useLang()` so they re-render when the user flips the switcher.
 *
 * The active language is persisted in `localStorage` under
 * `maestro.lang`, so it survives reloads. First-run defaults to the browser
 * navigator language if we have a catalog for it, else English.
 */
import { useSyncExternalStore } from "react"
import { en } from "./en"
import { zh } from "./zh"

export type Lang = "en" | "zh"

export const SUPPORTED_LANGS: Lang[] = ["en", "zh"]

const CATALOGS: Record<Lang, Record<string, string>> = { en, zh }

const STORAGE_KEY = "maestro.lang"

function detect(): Lang {
  if (typeof window === "undefined") return "en"
  const stored = window.localStorage.getItem(STORAGE_KEY) as Lang | null
  if (stored && SUPPORTED_LANGS.includes(stored)) return stored
  const nav = window.navigator.language?.toLowerCase() ?? ""
  if (nav.startsWith("zh")) return "zh"
  return "en"
}

let current: Lang = detect()
const listeners = new Set<() => void>()

export function getLang(): Lang {
  return current
}

export function setLang(lang: Lang) {
  if (!SUPPORTED_LANGS.includes(lang)) return
  if (lang === current) return
  current = lang
  try {
    window.localStorage.setItem(STORAGE_KEY, lang)
  } catch {
    // private mode / quota — silently keep the in-memory value
  }
  listeners.forEach((fn) => fn())
}

/**
 * React hook returning the current language code. Re-renders when the
 * language is changed anywhere in the app via `setLang`.
 */
export function useLang(): Lang {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      return () => {
        listeners.delete(cb)
      }
    },
    () => current,
    () => "en" as const,
  )
}

/**
 * Translate a key. Substitutes `{name}` placeholders with values from
 * `params`. Falls back to the English value, then to the key itself.
 */
export function t(key: string, params?: Record<string, string | number>): string {
  const lang = current
  const fromLang = CATALOGS[lang]?.[key]
  const value = fromLang ?? CATALOGS.en[key] ?? key
  if (!params) return value
  return value.replace(/\{(\w+)\}/g, (_, name) => String(params[name] ?? `{${name}}`))
}

export const LANG_LABELS: Record<Lang, string> = {
  en: "English",
  zh: "中文",
}
