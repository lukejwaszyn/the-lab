import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

const closeListeners = vi.hoisted(() => ({
  handlers: [] as ((e: { preventDefault(): void }) => Promise<void>)[],
  unlisten: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async (handler: any) => {
      closeListeners.handlers.push(handler);
      return () => {
        closeListeners.unlisten();
        closeListeners.handlers = closeListeners.handlers.filter(
          (h) => h !== handler,
        );
      };
    }),
  }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), warning: vi.fn() } }));

import {
  type DocxSession,
  discardDocxEdits,
  flushDocxEdits,
  hasStashedDocxEdits,
  hasUnsavedDocx,
  isAtOrUnder,
  moveDocxEdits,
  movedPath,
  registerDocxSession,
  resetDocxSessionsForTest,
  stashDocxEdits,
  takeStashedDocxEdits,
  trackDocxClose,
  updateCloseGuard,
  waitForDocxClose,
} from "@/lib/docx/docx-session";

function session(absolutePath: string, dirty = true, saves = true) {
  let isDirty = dirty;
  const s = {
    absolutePath,
    isDirty: () => isDirty,
    save: vi.fn(async () => {
      if (saves) isDirty = false;
      return saves;
    }),
    discard: vi.fn(() => {
      isDirty = false;
    }),
    moveTo: vi.fn((path: string) => {
      s.absolutePath = path;
    }),
    setDirty(next: boolean) {
      isDirty = next;
    },
  };
  return s satisfies DocxSession;
}

/** The disk as `docx_file_state` and `docx_save` see it. */
let disk: Record<string, number>;
let written: { path: string; bytes: unknown }[];
let saveFails = false;

function kept(
  absolutePath: string,
  extra: Partial<Parameters<typeof stashDocxEdits>[0]> = {},
) {
  stashDocxEdits({
    absolutePath,
    bytes: new Uint8Array([1, 2, 3]),
    knownModifiedMs: 1000,
    reason: "the folder could not be written",
    conflict: false,
    ...extra,
  });
}

const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetDocxSessionsForTest();
  closeListeners.handlers = [];
  closeListeners.unlisten.mockClear();
  disk = {};
  written = [];
  saveFails = false;
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(
    async (cmd: string, args?: any, opts?: any) => {
      if (cmd === "docx_file_state") {
        const at = disk[args.path];
        return {
          exists: at != null,
          modifiedMs: at ?? null,
          size: 3,
          openInWord: false,
          readOnly: false,
        };
      }
      if (cmd === "docx_save") {
        if (saveFails) throw "The folder is gone.";
        const path = decodeURIComponent(opts.headers.path);
        written.push({ path, bytes: args });
        disk[path] = (disk[path] ?? 0) + 500;
        return {
          exists: true,
          modifiedMs: disk[path],
          size: 3,
          openInWord: false,
          readOnly: false,
        };
      }
      throw new Error(`unexpected ${cmd}`);
    },
  );
});

describe("Word tabs the app can save", () => {
  it("matches a file or the files in a folder", () => {
    expect(isAtOrUnder("/p/a.docx", "/p/a.docx")).toBe(true);
    expect(isAtOrUnder("/p/sub/a.docx", "/p/sub")).toBe(true);
    expect(isAtOrUnder("/p/sub/a.docx", "/p/sub/")).toBe(true);
    expect(isAtOrUnder("/p/subway/a.docx", "/p/sub")).toBe(false);
  });

  it("saves only the tabs with unsaved edits, under a path when given", async () => {
    const a = session("/p/a.docx");
    const b = session("/p/other/b.docx");
    const clean = session("/p/c.docx", false);
    registerDocxSession(a);
    registerDocxSession(b);
    registerDocxSession(clean);
    expect(await flushDocxEdits("/p/other")).toBe(true);
    expect(b.save).toHaveBeenCalled();
    expect(a.save).not.toHaveBeenCalled();
    expect(await flushDocxEdits()).toBe(true);
    expect(a.save).toHaveBeenCalled();
    expect(clean.save).not.toHaveBeenCalled();
    expect(hasUnsavedDocx()).toBe(false);
  });

  it("reports a tab that could not save", async () => {
    registerDocxSession(session("/p/a.docx", true, false));
    expect(await flushDocxEdits()).toBe(false);
  });

  it("saves implicitly, except on Cmd+S", async () => {
    const a = session("/p/a.docx");
    registerDocxSession(a);
    await flushDocxEdits();
    expect(a.save).toHaveBeenLastCalledWith({ implicit: true });
    a.setDirty(true);
    await flushDocxEdits("/p/a.docx", { explicit: true });
    expect(a.save).toHaveBeenLastCalledWith({ implicit: false });
  });

  it("moves tabs and their remembered paths with a renamed folder", () => {
    const a = session("/p/sub/a.docx");
    const b = session("/p/b.docx");
    registerDocxSession(a);
    registerDocxSession(b);
    kept("/p/sub/c.docx");
    moveDocxEdits("/p/sub", "/p/renamed");
    expect(a.moveTo).toHaveBeenCalledWith("/p/renamed/a.docx");
    expect(b.moveTo).not.toHaveBeenCalled();
    expect(hasStashedDocxEdits("/p/sub/c.docx")).toBe(false);
    expect(takeStashedDocxEdits("/p/renamed/c.docx")?.absolutePath).toBe(
      "/p/renamed/c.docx",
    );
    expect(movedPath("/p/a.docx", "/p/a.docx", "/p/b.docx")).toBe("/p/b.docx");
    expect(movedPath("/p/subway/x.docx", "/p/sub", "/q")).toBe(
      "/p/subway/x.docx",
    );
  });

  it("drops the edits of tabs whose files are deleted", () => {
    const a = session("/p/sub/a.docx");
    const b = session("/p/b.docx");
    registerDocxSession(a);
    registerDocxSession(b);
    discardDocxEdits("/p/sub");
    expect(a.discard).toHaveBeenCalled();
    expect(b.discard).not.toHaveBeenCalled();
  });
});

describe("edits kept after a failed save", () => {
  it("count as unsaved, and are handed over once", () => {
    expect(hasUnsavedDocx()).toBe(false);
    kept("/p/a.docx");
    expect(hasUnsavedDocx()).toBe(true);
    expect(hasStashedDocxEdits("/p/a.docx")).toBe(true);
    expect(takeStashedDocxEdits("/p/a.docx")?.reason).toMatch(/folder/);
    expect(takeStashedDocxEdits("/p/a.docx")).toBeNull();
    expect(hasUnsavedDocx()).toBe(false);
  });

  it("are written by a later flush when the file is as it was", async () => {
    disk["/p/a.docx"] = 1000;
    kept("/p/a.docx");
    expect(await flushDocxEdits()).toBe(true);
    expect(written).toEqual([
      { path: "/p/a.docx", bytes: new Uint8Array([1, 2, 3]) },
    ]);
    expect(hasUnsavedDocx()).toBe(false);
  });

  it("are not written over a change made on disk since", async () => {
    disk["/p/a.docx"] = 4000;
    kept("/p/a.docx");
    expect(await flushDocxEdits()).toBe(false);
    expect(written).toHaveLength(0);
    // Asked about once; still kept, never written unasked.
    expect(takeStashedDocxEdits("/p/a.docx")?.conflict).toBe(true);

    kept("/p/b.docx", { conflict: true });
    expect(await flushDocxEdits()).toBe(false);
    expect(written).toHaveLength(0);
  });

  it("stay kept when the save fails again", async () => {
    disk["/p/a.docx"] = 1000;
    saveFails = true;
    kept("/p/a.docx");
    expect(await flushDocxEdits()).toBe(false);
    expect(hasStashedDocxEdits("/p/a.docx")).toBe(true);
  });

  it("are dropped with their deleted file", () => {
    kept("/p/sub/a.docx");
    discardDocxEdits("/p/sub");
    expect(hasUnsavedDocx()).toBe(false);
  });

  it("are left to the open tab for the same file", async () => {
    const a = session("/p/a.docx");
    registerDocxSession(a);
    disk["/p/a.docx"] = 1000;
    kept("/p/a.docx");
    await flushDocxEdits();
    expect(a.save).toHaveBeenCalled();
    expect(written).toHaveLength(0);
  });
});

describe("a tab closing", () => {
  it("is waited for by the next tab on the same file", async () => {
    let finish: () => void = () => {};
    trackDocxClose(
      "/p/a.docx",
      new Promise<void>((r) => {
        finish = r;
      }),
    );
    let opened = false;
    const opening = waitForDocxClose("/p/a.docx").then(() => {
      opened = true;
    });
    await settle();
    expect(opened).toBe(false);
    await waitForDocxClose("/p/other.docx");
    finish();
    await opening;
    expect(opened).toBe(true);
  });
});

describe("saving before the window closes", () => {
  it("listens for the close only while a tab has unsaved edits", async () => {
    const a = session("/p/a.docx", false);
    const unregister = registerDocxSession(a);
    await settle();
    expect(closeListeners.handlers).toHaveLength(0);
    a.setDirty(true);
    updateCloseGuard();
    await settle();
    expect(closeListeners.handlers).toHaveLength(1);
    a.setDirty(false);
    updateCloseGuard();
    expect(closeListeners.unlisten).toHaveBeenCalled();
    unregister();
  });

  it("saves, then lets the window close", async () => {
    const a = session("/p/a.docx");
    registerDocxSession(a);
    await settle();
    const event = { preventDefault: vi.fn() };
    await closeListeners.handlers[0](event);
    expect(a.save).toHaveBeenCalled();
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("keeps the window open once for kept edits that cannot be written", async () => {
    disk["/p/a.docx"] = 9000;
    kept("/p/a.docx");
    await settle();
    expect(closeListeners.handlers).toHaveLength(1);
    const first = { preventDefault: vi.fn() };
    await closeListeners.handlers[0](first);
    expect(first.preventDefault).toHaveBeenCalled();
    expect(written).toHaveLength(0);
  });

  it("keeps the window open once when a save fails, and closes the next time", async () => {
    const a = session("/p/a.docx", true, false);
    registerDocxSession(a);
    await settle();
    const first = { preventDefault: vi.fn() };
    await closeListeners.handlers[0](first);
    expect(first.preventDefault).toHaveBeenCalled();
    const second = { preventDefault: vi.fn() };
    await closeListeners.handlers[0](second);
    expect(second.preventDefault).not.toHaveBeenCalled();
  });
});
