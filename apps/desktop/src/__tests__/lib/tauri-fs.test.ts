import { describe, it, expect, beforeEach, vi } from "vitest";
import { readDir, stat } from "@tauri-apps/plugin-fs";
import {
  getProjectFileType,
  isTransientOfficeFile,
  scanProjectFolder,
  shouldSkipProjectDirectory,
} from "@/lib/tauri/fs";

describe("tauri fs helpers", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe("getProjectFileType", () => {
    it("classifies editable project files", () => {
      expect(getProjectFileType("main.tex")).toBe("tex");
      expect(getProjectFileType("chapter.TEX")).toBe("tex");
      expect(getProjectFileType("refs.bib")).toBe("bib");
      expect(getProjectFileType("output.pdf")).toBe("pdf");
      expect(getProjectFileType("figure.png")).toBe("image");
      expect(getProjectFileType("custom.sty")).toBe("style");
      expect(getProjectFileType("notes.md")).toBe("other");
      expect(getProjectFileType("script.py")).toBe("other");
    });

    it("ignores LaTeX and compiled build artifacts", () => {
      expect(getProjectFileType("main.aux")).toBeNull();
      expect(getProjectFileType("main.synctex.gz")).toBeNull();
      expect(getProjectFileType("module.pyc")).toBeNull();
      expect(getProjectFileType("native.pyd")).toBeNull();
      expect(getProjectFileType("libnative.so")).toBeNull();
    });

    it("keeps imported files with arbitrary extensions visible", () => {
      expect(getProjectFileType("archive.zip")).toBe("other");
      expect(getProjectFileType("data.xlsx")).toBe("other");
      expect(getProjectFileType("movie.mp4")).toBe("other");
    });

    it("gives Word files their own types", () => {
      expect(getProjectFileType("paper.docx")).toBe("docx");
      expect(getProjectFileType("Thesis Draft.DOCX")).toBe("docx");
      // Word's old binary format shows a note, never its bytes as text.
      expect(getProjectFileType("old report.doc")).toBe("doc");
      expect(getProjectFileType("OLD.DOC")).toBe("doc");
    });

    it("hides Office owner files and the Word tab's save file", () => {
      expect(getProjectFileType("~$per.docx")).toBeNull();
      expect(getProjectFileType("~$paper.docx")).toBeNull();
      expect(getProjectFileType("~$ta.xlsx")).toBeNull();
      expect(getProjectFileType(".paper.docx.0f3a.thelab-save")).toBeNull();
      expect(isTransientOfficeFile("~$x.docx")).toBe(true);
      expect(isTransientOfficeFile("paper~$.docx")).toBe(false);
      expect(isTransientOfficeFile("$paper.docx")).toBe(false);
    });
  });

  describe("shouldSkipProjectDirectory", () => {
    it("skips hidden and generated dependency directories", () => {
      expect(shouldSkipProjectDirectory(".git")).toBe(true);
      expect(shouldSkipProjectDirectory(".venv")).toBe(true);
      expect(shouldSkipProjectDirectory("node_modules")).toBe(true);
      expect(shouldSkipProjectDirectory("__pycache__")).toBe(true);
      expect(shouldSkipProjectDirectory("venv")).toBe(true);
      expect(shouldSkipProjectDirectory("ENV")).toBe(true);
    });

    it("keeps normal project folders visible", () => {
      expect(shouldSkipProjectDirectory("chapters")).toBe(false);
      expect(shouldSkipProjectDirectory("figures")).toBe(false);
      expect(shouldSkipProjectDirectory("attachments")).toBe(false);
    });
  });

  describe("scanProjectFolder", () => {
    it("does not recurse into generated cache directories", async () => {
      vi.mocked(readDir).mockImplementation(async (dir: string | URL) => {
        const dirPath = String(dir);
        if (dirPath === "/project") {
          return [
            { name: "__pycache__", isDirectory: true },
            { name: "node_modules", isDirectory: true },
            { name: "main.tex", isDirectory: false },
            { name: "chapters", isDirectory: true },
          ] as any;
        }

        if (dirPath === "/project/chapters") {
          return [{ name: "intro.tex", isDirectory: false }] as any;
        }

        throw new Error(`Unexpected readDir path: ${dirPath}`);
      });

      const result = await scanProjectFolder("/project");

      expect(readDir).toHaveBeenCalledWith("/project");
      expect(readDir).toHaveBeenCalledWith("/project/chapters");
      expect(readDir).not.toHaveBeenCalledWith("/project/__pycache__");
      expect(readDir).not.toHaveBeenCalledWith("/project/node_modules");
      expect(result.folders).toEqual(["chapters"]);
      expect(result.files.map((file) => file.relativePath)).toEqual([
        "main.tex",
        "chapters/intro.tex",
      ]);
    });

    it("keeps arbitrary file formats visible as other files", async () => {
      vi.mocked(readDir).mockResolvedValue([
        { name: "worker.py", isDirectory: false },
        { name: "notes.txt", isDirectory: false },
      ] as any);
      vi.mocked(stat).mockResolvedValue({ size: 128 } as any);

      const result = await scanProjectFolder("/project");

      expect(result.files.map((file) => file.relativePath)).toEqual([
        "worker.py",
        "notes.txt",
      ]);
      expect(stat).toHaveBeenCalledTimes(2);
      expect(result.files.every((file) => file.type === "other")).toBe(true);
    });

    it("lists a Word file but not Word's owner file beside it", async () => {
      vi.mocked(readDir).mockResolvedValue([
        { name: "report.docx", isDirectory: false },
        { name: "~$report.docx", isDirectory: false },
        { name: "legacy.doc", isDirectory: false },
      ] as any);

      const result = await scanProjectFolder("/project");

      expect(
        result.files.map((file) => [file.relativePath, file.type]),
      ).toEqual([
        ["report.docx", "docx"],
        ["legacy.doc", "doc"],
      ]);
    });
  });
});
