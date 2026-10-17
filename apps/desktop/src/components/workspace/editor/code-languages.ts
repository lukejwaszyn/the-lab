/**
 * Highlighting for code files. A file whose extension names a language below
 * gets that language's CodeMirror pack, loaded with import() the first time
 * such a file opens, so a session that opens only LaTeX loads none of them.
 * Every other file keeps the language latex-editor.tsx gave it before: BibTeX
 * for a .bib, the LaTeX mode for the rest.
 */
import { closeBrackets } from "@codemirror/autocomplete";
import { bracketMatching, StreamLanguage } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { createLogger } from "@/lib/debug/logger";

const log = createLogger("code-language");

export type CodeLanguage =
  | "python"
  | "octave"
  | "json"
  | "yaml"
  | "toml"
  | "shell"
  | "cpp"
  | "javascript"
  | "jsx"
  | "typescript"
  | "tsx"
  | "html"
  | "css"
  | "rust";

const BY_EXTENSION = new Map<string, CodeLanguage>([
  ["py", "python"],
  ["pyi", "python"],
  ["m", "octave"],
  ["json", "json"],
  ["yml", "yaml"],
  ["yaml", "yaml"],
  ["toml", "toml"],
  ["sh", "shell"],
  ["bash", "shell"],
  ["zsh", "shell"],
  ["c", "cpp"],
  ["h", "cpp"],
  ["cpp", "cpp"],
  ["hpp", "cpp"],
  ["cc", "cpp"],
  ["js", "javascript"],
  ["mjs", "javascript"],
  ["cjs", "javascript"],
  ["jsx", "jsx"],
  ["ts", "typescript"],
  ["tsx", "tsx"],
  ["html", "html"],
  ["css", "css"],
  ["rs", "rust"],
]);

/**
 * The code language of a file, from its extension (any case), or null when
 * the extension names none. A name without an extension, such as `Makefile`
 * or `.zshrc`, has none.
 */
export function codeLanguageFor(path: string): CodeLanguage | null {
  const name = path.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  return BY_EXTENSION.get(name.slice(dot + 1).toLowerCase()) ?? null;
}

/** Each pack, imported on first use. The octave, shell and TOML modes are
 * CodeMirror 5 modes from @codemirror/legacy-modes; the rest are lang-* packs. */
const LOADERS: Record<CodeLanguage, () => Promise<Extension>> = {
  python: () => import("@codemirror/lang-python").then((m) => m.python()),
  octave: () =>
    import("@codemirror/legacy-modes/mode/octave").then((m) =>
      StreamLanguage.define(m.octave),
    ),
  json: () => import("@codemirror/lang-json").then((m) => m.json()),
  yaml: () => import("@codemirror/lang-yaml").then((m) => m.yaml()),
  toml: () =>
    import("@codemirror/legacy-modes/mode/toml").then((m) =>
      StreamLanguage.define(m.toml),
    ),
  shell: () =>
    import("@codemirror/legacy-modes/mode/shell").then((m) =>
      StreamLanguage.define(m.shell),
    ),
  cpp: () => import("@codemirror/lang-cpp").then((m) => m.cpp()),
  javascript: () =>
    import("@codemirror/lang-javascript").then((m) => m.javascript()),
  jsx: () =>
    import("@codemirror/lang-javascript").then((m) =>
      m.javascript({ jsx: true }),
    ),
  typescript: () =>
    import("@codemirror/lang-javascript").then((m) =>
      m.javascript({ typescript: true }),
    ),
  tsx: () =>
    import("@codemirror/lang-javascript").then((m) =>
      m.javascript({ jsx: true, typescript: true }),
    ),
  html: () => import("@codemirror/lang-html").then((m) => m.html()),
  css: () => import("@codemirror/lang-css").then((m) => m.css()),
  rust: () => import("@codemirror/lang-rust").then((m) => m.rust()),
};

const loaded = new Map<CodeLanguage, Extension>();
const loading = new Map<CodeLanguage, Promise<Extension | null>>();

/** A language already loaded, for a view to start with; undefined until then. */
export function loadedCodeLanguage(
  language: CodeLanguage,
): Extension | undefined {
  return loaded.get(language);
}

/**
 * Loads a language once and resolves to its extension. Bracket matching and
 * closing come with it, as the LaTeX mode gave them to code files before.
 * When the import fails, it resolves to null, the file stays plain, and the
 * next file of that language tries again.
 */
export function loadCodeLanguage(
  language: CodeLanguage,
): Promise<Extension | null> {
  const pending = loading.get(language);
  if (pending) return pending;
  const next = LOADERS[language]().then(
    (support) => {
      const extension = [support, bracketMatching(), closeBrackets()];
      loaded.set(language, extension);
      return extension;
    },
    (error: unknown) => {
      log.warn(`could not load the ${language} highlighting`, {
        error: String(error),
      });
      loading.delete(language);
      return null;
    },
  );
  loading.set(language, next);
  return next;
}
