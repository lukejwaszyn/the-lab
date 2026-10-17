import { describe, expect, it } from "vitest";
import { toggleComment } from "@codemirror/commands";
import { ensureSyntaxTree, language } from "@codemirror/language";
import { EditorState, type Extension } from "@codemirror/state";
import {
  type CodeLanguage,
  codeLanguageFor,
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
