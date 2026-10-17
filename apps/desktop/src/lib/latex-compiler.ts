import { invoke } from "@tauri-apps/api/core";
import { resolveTexRoot, type ProjectFile } from "@/stores/document-store";
import { useCompileReportStore } from "@/stores/compile-report-store";
import type { CompilerBackend } from "@/stores/settings-store";
import { createLogger } from "@/lib/debug/logger";

const log = createLogger("latex");

/** Resolve which file to compile and the root ID for caching.
 *  resolveTexRoot now handles \documentclass detection and main.tex fallback,
 *  so the only remaining fallback here is for projects with no .tex files.
 *  Returns `null` when the project has no compilable .tex file. */
export function resolveCompileTarget(
  activeFileId: string,
  files: ProjectFile[],
): { rootId: string; targetPath: string } | null {
  const rootId = resolveTexRoot(activeFileId, files);
  const rootEntry = files.find((f) => f.id === rootId);
  if (rootEntry?.type === "tex") {
    return { rootId, targetPath: rootEntry.relativePath };
  }
  // No .tex file exists — cannot compile
  const anyTex = files.find((f) => f.type === "tex");
  if (anyTex) {
    return { rootId: anyTex.id, targetPath: anyTex.relativePath };
  }
  return null;
}

/** Extract a human-readable error message from an unknown catch value. */
export function formatCompileError(error: unknown): string {
  return error instanceof Error
    ? error.message
    : typeof error === "string"
      ? error
      : "Compilation failed";
}

/** The lines of a failed compile's message, one error each, without the
 *  "Compilation failed (engine)" heading that `compile_latex` starts with. */
export function compileErrorLines(message: string): string[] {
  const lines = message
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !/^Compilation failed\b/.test(line));
  return [...new Set(lines)];
}

/** One error or warning of a compile, as `latex_compile_report` gives it. */
export interface CompileProblem {
  severity: "error" | "warning";
  /** Relative to the project for its own files (a .bib file included),
   *  absolute for others; null when the engine did not say. */
  file: string | null;
  line: number | null;
  message: string;
}

/** What the last compile of a project found (thelab-core's result). */
export interface CompileReport {
  projectDir: string;
  mainFile: string;
  /** null when no engine could be started. */
  engine: "tectonic" | "latexmk" | null;
  ok: boolean;
  hasPdf: boolean;
  durationMs: number;
  errors: CompileProblem[];
  warnings: CompileProblem[];
}

/** Where a compile problem is, as the problems panel shows it. */
export function problemLocation(problem: {
  file: string | null;
  line: number | null;
}): string {
  if (!problem.file) return "";
  return problem.line ? `${problem.file}:${problem.line}` : problem.file;
}

/** The offset of the start of 1-based `line` in `content`, clamped to it. */
export function lineOffset(content: string, line: number): number {
  const lines = content.split("\n");
  const target = Math.max(1, Math.min(line, lines.length));
  let offset = 0;
  for (let i = 0; i < target - 1; i++) offset += lines[i].length + 1;
  return offset;
}

/** Fetches the report of the project's last compile into the store. */
export async function refreshCompileReport(projectDir: string): Promise<void> {
  try {
    const report = await invoke<CompileReport | null>("latex_compile_report", {
      projectDir,
    });
    useCompileReportStore.getState().setReport(projectDir, report ?? null);
  } catch (err) {
    log.debug("No compile report", { error: String(err) });
  }
}

export async function compileLatex(
  projectDir: string,
  mainFile: string = "main.tex",
  engine: CompilerBackend = "auto",
): Promise<Uint8Array> {
  log.info(`Compiling ${mainFile} (engine: ${engine})`);
  const start = performance.now();
  try {
    // compile_latex returns raw PDF bytes via Tauri IPC Response
    const buffer = await invoke<ArrayBuffer>("compile_latex", {
      projectDir,
      mainFile,
      engine,
    });

    const result = new Uint8Array(buffer);
    log.info(
      `Compiled ${mainFile} in ${(performance.now() - start).toFixed(0)}ms (${(result.byteLength / 1024).toFixed(0)} KB)`,
    );
    return result;
  } finally {
    // Errors and warnings by file and line, for the problems panel, whether
    // or not the compile left a PDF.
    await refreshCompileReport(projectDir);
  }
}

export interface TexliveStatus {
  available: boolean;
  engines: string[];
  version: string | null;
}

export async function detectTexlive(): Promise<TexliveStatus> {
  return invoke<TexliveStatus>("detect_texlive");
}

export interface SynctexResult {
  file: string;
  line: number;
  column: number;
}

export async function synctexEdit(
  projectDir: string,
  page: number,
  x: number,
  y: number,
): Promise<SynctexResult | null> {
  try {
    const result = await invoke<SynctexResult>("synctex_edit", {
      projectDir,
      page,
      x,
      y,
    });
    if (result)
      log.debug(`SyncTeX: page ${page} → ${result.file}:${result.line}`);
    return result;
  } catch (err) {
    log.debug("SyncTeX lookup failed", { page, error: String(err) });
    return null;
  }
}
