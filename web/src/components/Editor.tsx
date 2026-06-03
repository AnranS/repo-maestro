import CodeMirror from "@uiw/react-codemirror"
import { markdown, markdownLanguage } from "@codemirror/lang-markdown"
import { yaml } from "@codemirror/lang-yaml"
import { languages } from "@codemirror/language-data"
import { vscodeDark } from "@uiw/codemirror-theme-vscode"
import { EditorView } from "@codemirror/view"
import { useMemo } from "react"

type Kind = "markdown" | "yaml" | "plain"

function inferKind(filename: string | undefined, fallback: Kind = "markdown"): Kind {
  if (!filename) return fallback
  if (/\.(md|markdown)$/i.test(filename)) return "markdown"
  if (/\.(ya?ml)$/i.test(filename)) return "yaml"
  return "plain"
}

interface Props {
  value: string
  onChange: (next: string) => void
  filename?: string
  height?: string
  readOnly?: boolean
}

export function Editor({ value, onChange, filename, height = "60vh", readOnly }: Props) {
  const extensions = useMemo(() => {
    const kind = inferKind(filename)
    const base = [
      EditorView.lineWrapping,
      EditorView.theme({
        "&": { fontSize: "13px" },
        ".cm-content": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" },
        ".cm-gutters": { backgroundColor: "transparent", borderRight: "1px solid rgb(31 31 31)" },
        ".cm-activeLineGutter": { backgroundColor: "transparent" },
        ".cm-activeLine": { backgroundColor: "rgb(255 255 255 / 0.02)" },
        ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" },
      }),
    ]
    if (kind === "markdown") {
      return [
        ...base,
        markdown({ base: markdownLanguage, codeLanguages: languages }),
      ]
    }
    if (kind === "yaml") {
      return [...base, yaml()]
    }
    return base
  }, [filename])

  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      height={height}
      theme={vscodeDark}
      extensions={extensions}
      readOnly={readOnly}
      basicSetup={{
        lineNumbers: true,
        highlightActiveLine: true,
        highlightActiveLineGutter: true,
        foldGutter: true,
        bracketMatching: true,
        autocompletion: false,
        searchKeymap: true,
        history: true,
      }}
    />
  )
}
