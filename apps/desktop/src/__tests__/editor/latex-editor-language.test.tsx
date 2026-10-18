import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { indentUnit, language } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { latexLanguage } from "codemirror-lang-latex";
import { loadCodeLanguage } from "@/components/workspace/editor/code-languages";
import { LatexEditor } from "@/components/workspace/editor/latex-editor";
import { type ProjectFile, useDocumentStore } from "@/stores/document-store";
import type { ProjectFileType } from "@/lib/tauri/fs";

// Every code pack records its import; Rust's waits for the test to let it
// finish, so a file switch can happen while it loads.
const packs = vi.hoisted(() => {
  let release = () => {};
  const rustGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { imported: [] as string[], rustGate, releaseRust: () => release() };
});

vi.mock("@codemirror/lang-python", (importOriginal) => {
  packs.imported.push("lang-python");
  return importOriginal();
});
vi.mock("@codemirror/lang-json", (importOriginal) => {
  packs.imported.push("lang-json");
  return importOriginal();
});
vi.mock("@codemirror/lang-yaml", (importOriginal) => {
  packs.imported.push("lang-yaml");
  return importOriginal();
});
vi.mock("@codemirror/lang-cpp", (importOriginal) => {
  packs.imported.push("lang-cpp");
  return importOriginal();
});
vi.mock("@codemirror/lang-javascript", (importOriginal) => {
  packs.imported.push("lang-javascript");
  return importOriginal();
});
vi.mock("@codemirror/lang-html", (importOriginal) => {
  packs.imported.push("lang-html");
  return importOriginal();
});
vi.mock("@codemirror/lang-css", (importOriginal) => {
  packs.imported.push("lang-css");
  return importOriginal();
});
vi.mock("@codemirror/lang-rust", async (importOriginal) => {
  packs.imported.push("lang-rust");
  await packs.rustGate;
  return importOriginal();
});
vi.mock("@codemirror/legacy-modes/mode/octave", (importOriginal) => {
  packs.imported.push("legacy octave");
  return importOriginal();
});
vi.mock("@codemirror/legacy-modes/mode/toml", (importOriginal) => {
  packs.imported.push("legacy toml");
  return importOriginal();
});
vi.mock("@codemirror/legacy-modes/mode/shell", (importOriginal) => {
  packs.imported.push("legacy shell");
  return importOriginal();
});

// The editor's neighbours that need a PDF engine, a chat or a toolbar.
vi.mock("@/components/workspace/preview/pdf-viewer", () => ({
  PdfViewer: () => null,
}));
vi.mock("@/components/claude-chat/claude-chat-drawer", () => ({
  ClaudeChatDrawer: () => null,
}));
vi.mock("@/components/claude-chat/proposed-changes-panel", () => ({
  ProposedChangesPanel: () => null,
}));
vi.mock("@/components/workspace/editor/editor-toolbar", () => ({
  EditorToolbar: () => null,
}));
vi.mock("@/components/workspace/editor/output-panel", () => ({
  OutputPanel: () => null,
}));

function projectFile(
  relativePath: string,
  type: ProjectFileType,
  content: string,
): ProjectFile {
  return {
    id: relativePath,
    name: relativePath.split("/").pop() ?? relativePath,
    relativePath,
    absolutePath: `/project/${relativePath}`,
    type,
    content,
    isDirty: false,
  };
}

const FILES = [
  projectFile("main.tex", "tex", "\\section{Intro} % a note\n"),
  projectFile("refs.bib", "bib", "@article{key, title={T}}\n"),
  projectFile("notes.md", "other", "# Notes with $x$\n"),
  projectFile("macros.sty", "style", "\\newcommand{\\R}{\\mathbb{R}}\n"),
  projectFile("sim/model.py", "other", "x = 1  # a note\n"),
  projectFile("src/lib.rs", "other", "fn main() {}\n"),
  projectFile("tools/short.py", "other", "def f():\n  return 1\n"),
];

let container: HTMLDivElement;
let root: Root;

function editorState() {
  const dom = container.querySelector<HTMLElement>(".cm-editor");
  const view = dom ? EditorView.findFromDOM(dom) : null;
  if (!view) throw new Error("no editor is open");
  return view.state;
}

function editorLanguage() {
  return editorState().facet(language);
}

async function open(id: string) {
  await act(async () => {
    useDocumentStore.getState().setActiveFile(id);
  });
}

/** Lets pending imports and effects run. */
async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
}

beforeAll(async () => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  useDocumentStore.setState({
    files: FILES,
    activeFileId: "main.tex",
    projectRoot: "/project",
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<LatexEditor />);
  });
});

afterAll(async () => {
  await act(async () => root.unmount());
  container.remove();
});

// The tests share one editor and run in order.
describe("the editor's language", () => {
  it("is LaTeX for a .tex file, and opening one imports no code pack", async () => {
    await settle();
    expect(editorLanguage()).toBe(latexLanguage);
    expect(packs.imported).toEqual([]);
  });

  it("stays as it was for .bib, .md and .sty files, with no code pack", async () => {
    await open("refs.bib");
    expect(editorLanguage()?.name).toBe("bibtex");
    await open("notes.md");
    expect(editorLanguage()).toBe(latexLanguage);
    await open("macros.sty");
    expect(editorLanguage()).toBe(latexLanguage);
    await settle();
    expect(packs.imported).toEqual([]);
  });

  it("is Python for a .py file once its pack has loaded", async () => {
    await open("sim/model.py");
    await vi.waitFor(() => expect(editorLanguage()?.name).toBe("python"));
    expect(packs.imported).toEqual(["lang-python"]);
  });

  it("goes back to LaTeX when a .tex file follows a .py file", async () => {
    await open("main.tex");
    expect(editorLanguage()).toBe(latexLanguage);
    // Python is loaded now, so the .py file has it from the start.
    await open("sim/model.py");
    expect(editorLanguage()?.name).toBe("python");
    await open("main.tex");
    expect(editorLanguage()).toBe(latexLanguage);
  });

  it("indents a .py file by its own unit and leaves .tex as it was", async () => {
    await open("sim/model.py");
    expect(editorState().facet(indentUnit)).toBe("    ");
    await open("tools/short.py");
    expect(editorState().facet(indentUnit)).toBe("  ");
    await open("main.tex");
    // CodeMirror's default, which a .tex file had before.
    expect(editorState().facet(indentUnit)).toBe(
      EditorState.create().facet(indentUnit),
    );
  });

  it("keeps LaTeX when a pack finishes loading after its file was left", async () => {
    await open("src/lib.rs");
    expect(editorLanguage()).toBeNull();
    await open("main.tex");
    packs.releaseRust();
    await act(async () => {
      await loadCodeLanguage("rust");
    });
    await settle();
    expect(editorLanguage()).toBe(latexLanguage);
    await open("src/lib.rs");
    expect(editorLanguage()?.name).toBe("rust");
    expect(packs.imported).toEqual(["lang-python", "lang-rust"]);
  });
});
