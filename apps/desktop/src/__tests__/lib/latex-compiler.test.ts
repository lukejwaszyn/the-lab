import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  compileErrorLines,
  compileLatex,
  lineOffset,
  problemLocation,
  type CompileReport,
} from "@/lib/latex-compiler";
import { useCompileReportStore } from "@/stores/compile-report-store";
import { migrateSettings } from "@/stores/settings-store";
import {
  compileDiagnostics,
  diagnosticLocation,
} from "@/components/workspace/editor/problems-panel";

const mockInvoke = vi.mocked(invoke);

/** A biber document whose .bib file has an entry left open. */
const brokenBib: CompileReport = {
  projectDir: "/p",
  mainFile: "main.tex",
  engine: "latexmk",
  ok: false,
  hasPdf: false,
  durationMs: 812,
  errors: [
    {
      severity: "error",
      file: "refs.bib",
      line: 9,
      message: "syntax error: found end of entry, expected one of ...",
    },
    {
      severity: "error",
      file: null,
      line: null,
      message: "biber failed",
    },
  ],
  warnings: [
    {
      severity: "warning",
      file: "chapters/one.tex",
      line: 4,
      message: "Citation `nobody' undefined",
    },
  ],
};

describe("compileErrorLines", () => {
  it("lists one error per line without the heading", () => {
    const message =
      "Compilation failed (latexmk)\n\nrefs.bib:9: syntax error\nbiber failed\nbiber failed";
    expect(compileErrorLines(message)).toEqual([
      "refs.bib:9: syntax error",
      "biber failed",
    ]);
  });

  it("keeps a message that is not a compile failure", () => {
    expect(
      compileErrorLines("Server busy, too many concurrent compilations"),
    ).toEqual(["Server busy, too many concurrent compilations"]);
  });
});

describe("problemLocation", () => {
  it("names the file and line, the file alone, or nothing", () => {
    expect(problemLocation({ file: "refs.bib", line: 9 })).toBe("refs.bib:9");
    expect(problemLocation({ file: "refs.bib", line: null })).toBe("refs.bib");
    expect(problemLocation({ file: null, line: null })).toBe("");
  });
});

describe("lineOffset", () => {
  it("finds the start of a line and clamps to the text", () => {
    const text = "ab\ncde\nf";
    expect(lineOffset(text, 1)).toBe(0);
    expect(lineOffset(text, 2)).toBe(3);
    expect(lineOffset(text, 3)).toBe(7);
    expect(lineOffset(text, 99)).toBe(7);
    expect(lineOffset(text, 0)).toBe(0);
  });
});

describe("compileDiagnostics", () => {
  it("turns a report into panel items, errors then warnings", () => {
    const items = compileDiagnostics(brokenBib);
    expect(items.map((d) => [d.severity, d.file, d.line])).toEqual([
      ["error", "refs.bib", 9],
      ["error", undefined, 0],
      ["warning", "chapters/one.tex", 4],
    ]);
    expect(items.every((d) => d.source === "compile")).toBe(true);
    expect(items.map((d) => diagnosticLocation(d, "main.tex"))).toEqual([
      "refs.bib:9",
      "",
      "chapters/one.tex:4",
    ]);
  });

  it("has nothing without a report", () => {
    expect(compileDiagnostics(undefined)).toEqual([]);
  });

  it("leaves lint items at the open file", () => {
    expect(
      diagnosticLocation(
        { from: 5, to: 6, severity: "error", message: "x", line: 2 },
        "main.tex",
      ),
    ).toBe("main.tex:2");
  });
});

describe("compileLatex", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useCompileReportStore.setState({ reports: {} });
  });

  it("passes the engine choice and keeps the report of a failed compile", async () => {
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "compile_latex") {
        throw "Compilation failed (latexmk)\n\nrefs.bib:9: syntax error";
      }
      if (command === "latex_compile_report") return brokenBib;
      throw new Error(`unexpected ${command}`);
    });

    await expect(compileLatex("/p", "main.tex", "auto")).rejects.toContain(
      "refs.bib:9",
    );

    expect(mockInvoke).toHaveBeenCalledWith("compile_latex", {
      projectDir: "/p",
      mainFile: "main.tex",
      engine: "auto",
    });
    expect(mockInvoke).toHaveBeenCalledWith("latex_compile_report", {
      projectDir: "/p",
    });
    expect(useCompileReportStore.getState().reports["/p"]).toEqual(brokenBib);
  });

  it("clears the report when the backend has none", async () => {
    useCompileReportStore.getState().setReport("/p", brokenBib);
    mockInvoke.mockImplementation(async (command: string) =>
      command === "compile_latex" ? new ArrayBuffer(4) : null,
    );

    const pdf = await compileLatex("/p", "main.tex", "texlive");

    expect(pdf.byteLength).toBe(4);
    expect(useCompileReportStore.getState().reports["/p"]).toBeUndefined();
  });
});

describe("migrateSettings", () => {
  it("moves the old default Tectonic to Auto once", () => {
    expect(migrateSettings({ compilerBackend: "tectonic" }, 0)).toEqual({
      compilerBackend: "auto",
    });
  });

  it("keeps a TeX Live choice, and Tectonic chosen after the change", () => {
    expect(migrateSettings({ compilerBackend: "texlive" }, 0)).toEqual({
      compilerBackend: "texlive",
    });
    expect(migrateSettings({ compilerBackend: "tectonic" }, 1)).toEqual({
      compilerBackend: "tectonic",
    });
  });
});
