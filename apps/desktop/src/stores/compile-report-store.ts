import { create } from "zustand";
import type { CompileReport } from "@/lib/latex-compiler";

/**
 * The last compile's report per project folder, as `latex_compile_report`
 * returned it: errors and warnings by file and line, for the problems panel.
 */
interface CompileReportState {
  reports: Record<string, CompileReport>;
  setReport: (projectDir: string, report: CompileReport | null) => void;
}

export const useCompileReportStore = create<CompileReportState>()((set) => ({
  reports: {},
  setReport: (projectDir, report) =>
    set((state) => {
      const reports = { ...state.reports };
      if (report) reports[projectDir] = report;
      else delete reports[projectDir];
      return { reports };
    }),
}));
