import { describe, expect, it } from "vitest";
import {
  deleteCharBackward,
  indentMore,
  insertNewlineAndIndent,
  toggleComment,
} from "@codemirror/commands";
import { ensureSyntaxTree, indentUnit, language } from "@codemirror/language";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import {
  type CodeLanguage,
  codeIndentUnit,
  codeLanguageFor,
  detectIndentUnit,
  loadCodeLanguage,
  loadedCodeLanguage,
} from "@/components/workspace/editor/code-languages";

describe("codeLanguageFor", () => {
  it.each([
    ["model.py", "python"],
    ["stubs.pyi", "python"],
    ["radar.m", "octave"],
    ["package.json", "json"],
    ["ci.yml", "yaml"],
    ["config.yaml", "yaml"],
    ["thelab.toml", "toml"],
    ["run.sh", "shell"],
    ["setup.bash", "shell"],
    ["env.zsh", "shell"],
    ["fft.c", "cpp"],
    ["fft.h", "cpp"],
    ["fft.cpp", "cpp"],
    ["fft.hpp", "cpp"],
    ["fft.cc", "cpp"],
    ["app.js", "javascript"],
    ["app.mjs", "javascript"],
    ["app.cjs", "javascript"],
    ["view.jsx", "jsx"],
    ["store.ts", "typescript"],
    ["view.tsx", "tsx"],
    ["index.html", "html"],
    ["site.css", "css"],
    ["main.rs", "rust"],
  ])("%s is %s", (path, expected) => {
    expect(codeLanguageFor(path)).toBe(expected);
  });

  it("reads the extension in any case and after any folder", () => {
    expect(codeLanguageFor("Sim/Model.PY")).toBe("python");
    expect(codeLanguageFor("tools/plot.Json")).toBe("json");
    expect(codeLanguageFor("C:\\project\\src\\lib.rs")).toBe("rust");
  });

  it("takes the last extension of a dotted name", () => {
    expect(codeLanguageFor(".eslintrc.json")).toBe("json");
    expect(codeLanguageFor("store.test.ts")).toBe("typescript");
    expect(codeLanguageFor("types.d.ts")).toBe("typescript");
  });

  it.each([
    "main.tex",
    "refs.bib",
    "notes.md",
    "custom.sty",
    "notes.txt",
    "data.csv",
    "report.docx",
    "archive.tar.gz",
    "Makefile",
    ".zshrc",
    ".bashrc",
    "sim/.json",
    "folder.py/README",
    "trailing.",
    "x.constructor",
    "x.__proto__",
    "",
  ])("%j stays plain", (path) => {
    expect(codeLanguageFor(path)).toBeNull();
  });
});

async function stateFor(lang: CodeLanguage, doc: string): Promise<EditorState> {
  const extension = await loadCodeLanguage(lang);
  expect(extension).not.toBeNull();
  return EditorState.create({ doc, extensions: extension as Extension });
}

describe("loadCodeLanguage", () => {
  it.each<[CodeLanguage, string, string]>([
    ["python", "python", "def f(x):\n    return x  # done\n"],
    ["octave", "octave", "function y = f(x)\n  y = x'; % done\nend\n"],
    ["json", "json", '{"a": [1, true, null]}\n'],
    ["yaml", "yaml", "a:\n  - 1 # done\n"],
    ["toml", "toml", '[python]\ndefault = "python3" # done\n'],
    ["shell", "shell", 'for f in *.py; do echo "$f"; done\n'],
    ["cpp", "cpp", "#include <cstdio>\nint main() { return 0; }\n"],
    ["javascript", "javascript", "const a = () => 1;\n"],
    ["jsx", "javascript", "const a = <div>{1}</div>;\n"],
    ["typescript", "typescript", "const a: number = 1;\n"],
    ["tsx", "typescript", "const a: JSX.Element = <div />;\n"],
    ["html", "html", "<p>Hi <b>there</b></p>\n"],
    ["css", "css", "p { color: red; }\n"],
    ["rust", "rust", "fn main() { let x: u8 = 1; }\n"],
  ])("%s loads and parses as %s", async (lang, name, doc) => {
    const state = await stateFor(lang, doc);
    expect(state.facet(language)?.name).toBe(name);
    const tree = ensureSyntaxTree(state, state.doc.length, 5000);
    expect(tree?.length).toBe(state.doc.length);
    expect(loadedCodeLanguage(lang)).toBeDefined();
  });

  it("parses each dialect of JavaScript as its own", async () => {
    // "⚠" is the node a Lezer parser leaves where the text does not parse.
    const parses = async (lang: CodeLanguage, doc: string) => {
      const state = await stateFor(lang, doc);
      let clean = true;
      ensureSyntaxTree(state, doc.length, 5000)?.iterate({
        enter: (n) => {
          if (n.type.isError) clean = false;
        },
      });
      return clean;
    };
    const jsx = "const a = <div>{1}</div>;";
    const ts = "interface A { b: number }";
    expect(await parses("javascript", "const a = () => 1;")).toBe(true);
    expect(await parses("javascript", jsx)).toBe(false);
    expect(await parses("javascript", ts)).toBe(false);
    expect(await parses("jsx", jsx)).toBe(true);
    expect(await parses("jsx", ts)).toBe(false);
    expect(await parses("typescript", ts)).toBe(true);
    expect(await parses("tsx", `${ts}\n${jsx}`)).toBe(true);
  });

  it("loads a language once", async () => {
    const first = loadCodeLanguage("rust");
    expect(loadCodeLanguage("rust")).toBe(first);
    expect(await first).toBe(loadedCodeLanguage("rust"));
  });
});

describe("the comment toggle", () => {
  async function toggled(lang: CodeLanguage, line: string): Promise<string> {
    let state = await stateFor(lang, line);
    toggleComment({
      state,
      dispatch: (tr) => {
        state = tr.state;
      },
    });
    return state.doc.toString();
  }

  it.each<[CodeLanguage, string, string]>([
    ["python", "x = 1", "# x = 1"],
    ["octave", "x = 1;", "% x = 1;"],
    ["shell", "echo hi", "# echo hi"],
    ["toml", "a = 1", "# a = 1"],
    ["yaml", "a: 1", "# a: 1"],
    ["cpp", "int a;", "// int a;"],
    ["javascript", "let a;", "// let a;"],
    ["typescript", "let a;", "// let a;"],
    ["rust", "let a = 1;", "// let a = 1;"],
    ["css", "p { color: red; }", "/* p { color: red; } */"],
    ["html", "<p>hi</p>", "<!-- <p>hi</p> -->"],
  ])("comments a %s line with its own syntax", async (lang, line, expected) => {
    expect(await toggled(lang, line)).toBe(expected);
  });

  it("leaves JSON alone, which has no comments", async () => {
    expect(await toggled("json", '{"a": 1}')).toBe('{"a": 1}');
  });
});

describe("detectIndentUnit", () => {
  it.each<[string, string, string]>([
    [
      "four spaces",
      "def f(x):\n    if x:\n        return 1\n    return 2\n",
      "    ",
    ],
    ["two spaces", "def f(x):\n  if x:\n    return 1\n  return 2\n", "  "],
    ["tabs", "def f(x):\n\tif x:\n\t\treturn 1\n\treturn 2\n", "\t"],
    ["CRLF line ends", "fn main() {\r\n    let a = 1;\r\n}\r\n", "    "],
  ])("reads %s", (_, text, expected) => {
    expect(detectIndentUnit(text)).toBe(expected);
  });

  it("is not misled by arguments aligned under a bracket", () => {
    const text = [
      "def plot(ax, data):",
      "    line = ax.plot(data.x,",
      "                   data.y)",
      "    if line:",
      "        ax.legend()",
      "    for k in data:",
      "        print(k)",
      "",
    ].join("\n");
    expect(detectIndentUnit(text)).toBe("    ");
  });

  it.each([
    ["an empty file", ""],
    ["a file with no indented line", "import os\nx = 1\n"],
    ["one step of 2 against one of 4", "a:\n  b: 1\nc:\n    d: 1\n"],
    ["steps of 3 spaces", "a:\n   b:\n      c: 1\n"],
    ["a call aligned under its bracket", "x = f(a,\n      b)\n"],
  ])("says nothing for %s", (_, text) => {
    expect(detectIndentUnit(text)).toBeNull();
  });
});

describe("codeIndentUnit", () => {
  it.each<[CodeLanguage, string]>([
    ["python", "    "],
    ["rust", "    "],
    ["cpp", "    "],
    ["octave", "    "],
    ["javascript", "  "],
    ["typescript", "  "],
    ["json", "  "],
    ["yaml", "  "],
    ["html", "  "],
    ["css", "  "],
    ["shell", "  "],
    ["toml", "  "],
  ])("starts a new %s file with %j", (lang, expected) => {
    expect(codeIndentUnit(lang, "")).toBe(expected);
  });

  it("follows the file over the language's usual unit", () => {
    expect(codeIndentUnit("python", "if x:\n  y = 1\n")).toBe("  ");
    expect(codeIndentUnit("python", "if x:\n\ty = 1\n")).toBe("\t");
    expect(codeIndentUnit("json", '{\n    "a": 1\n}\n')).toBe("    ");
  });
});

describe("Enter and Tab in code files", () => {
  /** The state of a code file as the editor sets it up, cursor at the end. */
  async function codeState(lang: CodeLanguage, doc: string) {
    const extension = await loadCodeLanguage(lang);
    return EditorState.create({
      doc,
      selection: { anchor: doc.length },
      extensions: [
        extension as Extension,
        indentUnit.of(codeIndentUnit(lang, doc)),
      ],
    });
  }

  /** The indentation Enter gives the new line after `doc`. */
  async function enterAfter(lang: CodeLanguage, doc: string): Promise<string> {
    let state = await codeState(lang, doc);
    insertNewlineAndIndent({
      state,
      dispatch: (tr) => {
        state = tr.state;
      },
    });
    return state.doc.lineAt(state.doc.length).text;
  }

  // A 4-space Python file, as PEP 8 writes it.
  const PY = "def f(x):\n    a = 1\n    if x:\n        b = 2";

  it.each<[string, string, string]>([
    ["a def line", "def f(x):", "    "],
    ["a line in a function", "def f(x):\n    a = 1", "    "],
    ["an if line in a function", "def f(x):\n    a = 1\n    if x:", "        "],
    ["a line in an if block", PY, "        "],
  ])("in 4-space Python, Enter after %s indents to the block", async (_, doc, expected) => {
    expect(await enterAfter("python", doc)).toBe(expected);
  });

  it("follows a 2-space Python file", async () => {
    expect(await enterAfter("python", "def f(x):\n  if x:")).toBe("    ");
    expect(await enterAfter("python", "def f(x):\n  a = 1")).toBe("  ");
  });

  it("indents a tab-indented Python file with tabs", async () => {
    expect(await enterAfter("python", "def f(x):\n\tif x:")).toBe("\t\t");
  });

  it("indents Rust and C blocks by four spaces", async () => {
    expect(await enterAfter("rust", "fn main() {")).toBe("    ");
    expect(await enterAfter("cpp", "int main(void) {")).toBe("    ");
    expect(await enterAfter("cpp", "int main(void) {\n    if (a) {")).toBe(
      "        ",
    );
  });

  it("keeps JavaScript at two spaces", async () => {
    expect(await enterAfter("javascript", "function f() {")).toBe("  ");
  });

  it("steps back one unit with Backspace in the indentation", async () => {
    // Backspace is a view command, so it runs on a view.
    const view = new EditorView({
      state: await codeState("python", `${PY}\n        `),
    });
    deleteCharBackward(view);
    expect(view.state.doc.lineAt(view.state.doc.length).text).toBe("    ");
    view.destroy();
  });

  it("indents by the same unit with Tab", async () => {
    let state = await codeState("python", "x = 1");
    indentMore({
      state,
      dispatch: (tr) => {
        state = tr.state;
      },
    });
    expect(state.doc.toString()).toBe("    x = 1");
  });
});
