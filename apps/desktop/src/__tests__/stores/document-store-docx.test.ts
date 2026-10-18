import { invoke } from "@tauri-apps/api/core";
import {
  readDir,
  readTextFile,
  remove,
  rename,
  writeTextFile,
} from "@tauri-apps/plugin-fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/stores/history-store", () => ({
  useHistoryStore: {
    getState: vi.fn(() => ({
      init: vi.fn(() => Promise.resolve()),
      loadSnapshots: vi.fn(() => Promise.resolve()),
      createSnapshot: vi.fn(() => Promise.resolve()),
      reset: vi.fn(),
      stopReview: vi.fn(),
    })),
  },
}));
vi.mock("@/stores/claude-chat-store", () => ({
  useClaudeChatStore: { getState: vi.fn(() => ({ newSession: vi.fn() })) },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: vi.fn(async () => () => {}) }),
}));

import {
  type DocxSession,
  registerDocxSession,
  resetDocxSessionsForTest,
} from "@/lib/docx/docx-session";
import {
  FILES_REFRESHED_EVENT,
  type ProjectFile,
  useDocumentStore,
} from "@/stores/document-store";

function file(overrides: Partial<ProjectFile>): ProjectFile {
  return {
    id: "report.docx",
    name: "report.docx",
    relativePath: "report.docx",
    absolutePath: "/project/report.docx",
    type: "docx",
    isDirty: false,
    ...overrides,
  };
}

function wordTab(absolutePath: string, saves = true) {
  let dirty = true;
  const tab = {
    absolutePath,
    isDirty: () => dirty,
    save: vi.fn(async () => {
      if (saves) dirty = false;
      return saves;
    }),
    discard: vi.fn(() => {
      dirty = false;
    }),
    moveTo: vi.fn((path: string) => {
      tab.absolutePath = path;
    }),
  } satisfies DocxSession;
  registerDocxSession(tab);
  return tab;
}

beforeEach(() => {
  vi.clearAllMocks();
  resetDocxSessionsForTest();
  useDocumentStore.setState({
    projectRoot: "/project",
    files: [
      file({}),
      {
        id: "main.tex",
        name: "main.tex",
        relativePath: "main.tex",
        absolutePath: "/project/main.tex",
        type: "tex",
        content: "\\documentclass{article}",
        isDirty: false,
      },
    ],
    folders: [],
    activeFileId: "report.docx",
    cursorPosition: 0,
  });
});

describe("Word files in the document store", () => {
  it("opens a project without reading a .docx as text or listing Word's owner file", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined as never);
    vi.mocked(readDir).mockResolvedValue([
      { name: "main.tex", isDirectory: false },
      { name: "report.docx", isDirectory: false },
      { name: "~$port.docx", isDirectory: false },
      { name: "old.doc", isDirectory: false },
    ] as any);
    vi.mocked(readTextFile).mockResolvedValue("\\documentclass{article}");
    await useDocumentStore.getState().openProject("/project");
    const files = useDocumentStore.getState().files;
    expect(files.map((f) => [f.relativePath, f.type])).toEqual([
      ["main.tex", "tex"],
      ["report.docx", "docx"],
      ["old.doc", "doc"],
    ]);
    expect(readTextFile).toHaveBeenCalledTimes(1);
    expect(files.find((f) => f.type === "docx")?.content).toBeUndefined();
  });

  it("never writes text into a Word file", async () => {
    const store = useDocumentStore.getState();
    store.insertAtCursor("\\section{Oops}");
    store.replaceSelection(0, 0, "x");
    expect(store.findAndReplace("a", "b")).toBe(false);
    const docx = useDocumentStore.getState().files[0];
    expect(docx.content).toBeUndefined();
    expect(docx.isDirty).toBe(false);
    useDocumentStore.getState().markFileDirty("report.docx", true);
    await useDocumentStore.getState().saveAllFiles();
    expect(writeTextFile).not.toHaveBeenCalled();
  });

  it("shows a Word tab's unsaved edits with the file tree's dot", () => {
    useDocumentStore.getState().markFileDirty("report.docx", true);
    expect(useDocumentStore.getState().files[0].isDirty).toBe(true);
    useDocumentStore.getState().markFileDirty("report.docx", false);
    expect(useDocumentStore.getState().files[0].isDirty).toBe(false);
  });

  it("saves the open Word tab on ⌘S", async () => {
    const tab = wordTab("/project/report.docx");
    await useDocumentStore.getState().saveCurrentFile();
    expect(tab.save).toHaveBeenCalled();
    expect(writeTextFile).not.toHaveBeenCalled();
  });

  it("saves a Word tab before its file is renamed, and refuses if it cannot", async () => {
    const tab = wordTab("/project/report.docx");
    await useDocumentStore.getState().renameFile("report.docx", "final.docx");
    expect(tab.save).toHaveBeenCalled();
    expect(rename).toHaveBeenCalledWith(
      "/project/report.docx",
      "/project/final.docx",
    );

    resetDocxSessionsForTest();
    vi.mocked(rename).mockClear();
    wordTab("/project/final.docx", false);
    await useDocumentStore.getState().renameFile("final.docx", "other.docx");
    expect(rename).not.toHaveBeenCalled();
  });

  it("drops a Word tab's edits when its file is deleted", async () => {
    const tab = wordTab("/project/report.docx");
    await useDocumentStore.getState().deleteFile("report.docx");
    expect(tab.discard).toHaveBeenCalled();
    expect(tab.save).not.toHaveBeenCalled();
    expect(remove).toHaveBeenCalledWith("/project/report.docx");
  });

  it("tells Word tabs when the files were refreshed", async () => {
    vi.mocked(readDir).mockResolvedValue([] as any);
    const heard = vi.fn();
    window.addEventListener(FILES_REFRESHED_EVENT, heard);
    await useDocumentStore.getState().refreshFiles();
    window.removeEventListener(FILES_REFRESHED_EVENT, heard);
    expect(heard).toHaveBeenCalledTimes(1);
  });
});
