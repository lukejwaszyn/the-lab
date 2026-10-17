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
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

import {
  type DocxSession,
  discardDocxEdits,
  flushDocxEdits,
  hasUnsavedDocx,
  isAtOrUnder,
  registerDocxSession,
  resetDocxSessionsForTest,
  updateCloseGuard,
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
    setDirty(next: boolean) {
      isDirty = next;
    },
  };
  return s satisfies DocxSession;
}

const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  resetDocxSessionsForTest();
  closeListeners.handlers = [];
  closeListeners.unlisten.mockClear();
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
