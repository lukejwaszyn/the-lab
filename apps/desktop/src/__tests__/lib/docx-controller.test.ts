import { invoke } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

// A stand-in for SuperDoc: it records how it was made and lets a test
// fire its callbacks.
const sd = vi.hoisted(() => {
  const made: any[] = [];
  class FakeSuperDoc {
    config: any;
    listeners: Record<string, (p: unknown) => void> = {};
    exports: Uint8Array[] = [];
    plugins: any[] = [];
    doc = { id: "doc-0" };
    activeEditor = {
      registerPlugin: (p: any) => this.plugins.push(p),
      state: { doc: this.doc },
    };
    setDocumentMode = vi.fn();
    destroy = vi.fn();
    constructor(config: any) {
      this.config = config;
      made.push(this);
      queueMicrotask(() => config.onReady?.({ superdoc: this }));
    }
    on(event: string, f: (p: unknown) => void) {
      this.listeners[event] = f;
    }
    async export() {
      const bytes = this.exports.shift() ?? new Uint8Array([9, 9]);
      // jsdom's Blob has no arrayBuffer(); WebKit's does.
      return { arrayBuffer: async () => bytes.slice().buffer };
    }
    /** A transaction that changed the document (or was refused). */
    transact(changed = true) {
      const before = this.doc;
      if (changed) this.doc = { id: `doc-${made.length}-${Math.random()}` };
      this.activeEditor.state.doc = this.doc;
      this.config.onTransaction?.({
        transaction: { docChanged: true, before },
        editor: this.activeEditor,
      });
    }
  }
  class Plugin {
    spec: any;
    constructor(spec: any) {
      this.spec = spec;
    }
  }
  class PluginKey {
    constructor(public name: string) {}
  }
  return { made, FakeSuperDoc, Plugin, PluginKey };
});

vi.mock("superdoc", () => ({
  SuperDoc: sd.FakeSuperDoc,
  Extensions: { Plugin: sd.Plugin, PluginKey: sd.PluginKey },
}));
vi.mock("superdoc/style.css", () => ({}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: vi.fn(async () => () => {}) }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), warning: vi.fn() } }));
vi.mock("@/lib/docx/preserve", () => ({
  preserveUnchanged: vi.fn((_o: Uint8Array, _b: Uint8Array, e: Uint8Array) => ({
    bytes: e,
    preserved: true,
    changedParts: ["word/document.xml"],
  })),
}));

import {
  DocxController,
  EQUATION_NOTE,
  decideOnDiskChange,
  isEquationEditError,
  removesEquation,
  type DocxFileState,
  type DocxTabState,
} from "@/lib/docx/docx-controller";
import {
  hasStashedDocxEdits,
  resetDocxSessionsForTest,
} from "@/lib/docx/docx-session";
import { preserveUnchanged } from "@/lib/docx/preserve";

const PATH = "/project/Report draft.docx";
const BYTES = new Uint8Array([80, 75, 3, 4]);

let disk: DocxFileState;
let saves: { bytes: unknown; headers: unknown }[];

function fileOnDisk(patch: Partial<DocxFileState> = {}) {
  disk = { ...disk, ...patch };
}

function makeController(
  options: Partial<ConstructorParameters<typeof DocxController>[0]> = {},
) {
  const states: DocxTabState[] = [];
  const editor = document.createElement("div");
  const controller = new DocxController({
    absolutePath: PATH,
    name: "Report draft.docx",
    editor,
    toolbarId: "toolbar-1",
    mode: "editing",
    author: "Ada Example",
    onChange: (s) => states.push(s),
    ...options,
  });
  return { controller, states, editor };
}

function last() {
  return sd.made[sd.made.length - 1];
}

/** Lets every pending promise settle (a real macrotask). */
async function settle(rounds = 5) {
  for (let i = 0; i < rounds; i++) {
    await new Promise((r) => setImmediate(r));
  }
}

async function until(done: () => boolean) {
  for (let i = 0; i < 100 && !done(); i++) await settle(1);
  expect(done()).toBe(true);
}

/** The next save that reaches the disk waits until `release()`. */
function slowNextSave() {
  let release: () => void = () => {};
  const gate = new Promise<void>((r) => {
    release = r;
  });
  const normal = vi.mocked(invoke).getMockImplementation();
  vi.mocked(invoke).mockImplementation(
    async (cmd: string, args?: any, opts?: any) => {
      if (cmd === "docx_save") {
        vi.mocked(invoke).mockImplementation(normal as never);
        saves.push({ bytes: args, headers: opts?.headers });
        await gate;
        disk = { ...disk, modifiedMs: (disk.modifiedMs ?? 0) + 500 };
        return { ...disk };
      }
      return normal?.(cmd, args, opts);
    },
  );
  return () => release();
}

beforeEach(() => {
  resetDocxSessionsForTest();
  sd.made.length = 0;
  saves = [];
  disk = {
    exists: true,
    modifiedMs: 1000,
    size: 4,
    openInWord: false,
    readOnly: false,
  };
  vi.mocked(readFile).mockReset();
  vi.mocked(readFile).mockResolvedValue(BYTES);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(
    async (cmd: string, args?: any, opts?: any) => {
      if (cmd === "docx_file_state") return { ...disk };
      if (cmd === "docx_save") {
        saves.push({ bytes: args, headers: opts?.headers });
        disk = { ...disk, modifiedMs: (disk.modifiedMs ?? 0) + 500 };
        return { ...disk };
      }
      throw new Error(`unexpected ${cmd}`);
    },
  );
});

describe("opening", () => {
  it("hands SuperDoc the file's bytes as a File, without telemetry", async () => {
    const { controller, editor } = makeController();
    await controller.open();
    expect(readFile).toHaveBeenCalledWith(PATH);
    const { config } = last();
    expect(config.document).toBeInstanceOf(File);
    expect(config.document.name).toBe("Report draft.docx");
    expect(config.document.type).toBe(
      "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    );
    const read = await new Promise<ArrayBuffer>((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as ArrayBuffer);
      reader.readAsArrayBuffer(config.document);
    });
    expect(new Uint8Array(read)).toEqual(BYTES);
    expect(config.selector).toBe(editor);
    expect(config.toolbar).toBe("#toolbar-1");
    expect(config.documentMode).toBe("editing");
    expect(config.user).toEqual({ name: "Ada Example", email: "" });
    expect(config.telemetry).toEqual({ enabled: false });
    expect(config.modules.toolbar.excludeItems).toEqual(["documentMode", "ai"]);
    expect(config.modules.ai).toBeUndefined();
    expect(controller.state.phase).toBe("ready");
  });

  it("opens in the remembered mode, and a read-only file in Viewing", async () => {
    const { controller } = makeController({ mode: "suggesting" });
    await controller.open();
    expect(last().config.documentMode).toBe("suggesting");

    fileOnDisk({ readOnly: true });
    const ro = makeController({ mode: "editing" }).controller;
    await ro.open();
    expect(last().config.documentMode).toBe("viewing");
    expect(ro.state.readOnly).toBe(true);
    ro.setMode("editing");
    expect(last().setDocumentMode).toHaveBeenLastCalledWith("viewing");
  });

  it("names tracked changes after Settings, or a plain Author", async () => {
    await makeController({ author: "  " }).controller.open();
    expect(last().config.user.name).toBe("Author");
  });

  it("shows a short message, not SuperDoc's error, when a file will not open", async () => {
    vi.mocked(readFile).mockRejectedValueOnce(new Error("EACCES: raw detail"));
    const { controller } = makeController();
    await controller.open();
    expect(controller.state.phase).toBe("error");
    expect(controller.state.error).toBe(
      "This file could not be opened as a Word document.",
    );
  });
});

describe("unsaved edits", () => {
  it("counts only transactions that changed the document after it loaded", async () => {
    const { controller, states } = makeController();
    await controller.open();
    expect(controller.isDirty()).toBe(false);
    last().transact(false); // refused, as by the equation guard
    expect(controller.isDirty()).toBe(false);
    last().transact(true);
    expect(controller.isDirty()).toBe(true);
    expect(states[states.length - 1].dirty).toBe(true);
  });

  it("counts a comment added, answered, resolved or deleted", async () => {
    const { controller } = makeController();
    await controller.open();
    const comments = (type: string, comment: object = {}) =>
      last().config.onCommentsUpdate({ type, comment });
    comments("selected");
    comments("add", { trackedChange: true }); // its transaction counts
    expect(controller.isDirty()).toBe(false);
    for (const type of ["add", "update", "deleted", "resolved"]) {
      const c = makeController().controller;
      await c.open();
      last().config.onCommentsUpdate({ type, comment: {} });
      expect(c.isDirty()).toBe(true);
    }
  });

  it("saves through the preserving save, and is clean after", async () => {
    const { controller } = makeController();
    await controller.open();
    const baseline = new Uint8Array([1]);
    const edited = new Uint8Array([2]);
    // The baseline was taken on open; set what the next export returns.
    (controller as any).baseline = baseline;
    last().exports.push(edited);
    last().transact();
    expect(await controller.save()).toBe(true);
    expect(preserveUnchanged).toHaveBeenCalledWith(BYTES, baseline, edited);
    expect(saves).toHaveLength(1);
    expect(saves[0].bytes).toEqual(edited);
    expect(saves[0].headers).toEqual({
      path: encodeURIComponent(PATH),
    });
    expect(controller.isDirty()).toBe(false);
    expect(controller.state.dirty).toBe(false);
  });

  it("does not write a clean document", async () => {
    const { controller } = makeController();
    await controller.open();
    expect(await controller.save()).toBe(true);
    expect(saves).toHaveLength(0);
  });

  it("keeps the edits marked unsaved when the save fails, and says why", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === "docx_save")
        throw "Report draft.docx is read-only, so it was not saved.";
      return { ...disk };
    });
    expect(await controller.save()).toBe(false);
    expect(controller.isDirty()).toBe(true);
    expect(controller.state.saveError).toMatch(/read-only/);
  });

  it("saves on close, unless the file is being deleted", async () => {
    const a = makeController().controller;
    await a.open();
    last().transact();
    expect(await a.close()).toBe("saved");
    expect(saves).toHaveLength(1);
    expect(last().destroy).toHaveBeenCalled();

    const b = makeController().controller;
    await b.open();
    last().transact();
    b.discard();
    expect(await b.close()).toBe("saved");
    expect(saves).toHaveLength(1);
  });

  it("runs one save at a time, and saves after it what was edited meanwhile", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    const release = slowNextSave();
    const first = controller.save();
    await until(() => saves.length === 1);
    last().transact(); // typed while the first save writes
    const second = controller.save();
    await settle();
    expect(saves).toHaveLength(1);
    release();
    expect(await first).toBe(false);
    expect(await second).toBe(true);
    expect(saves).toHaveLength(2);
    expect(controller.isDirty()).toBe(false);
  });
});

describe("closing with edits a save could not write", () => {
  function failSaves() {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === "docx_save") throw "The folder could not be written.";
      return { ...disk };
    });
  }

  it("keeps them, and shows them unsaved when the file opens again", async () => {
    const a = makeController().controller;
    await a.open();
    last().exports.push(new Uint8Array([4, 2]));
    last().transact();
    failSaves();
    expect(await a.close()).toBe("kept");
    expect(last().destroy).toHaveBeenCalled();
    expect(hasStashedDocxEdits(PATH)).toBe(true);

    const b = makeController().controller;
    await b.open();
    expect(readFile).toHaveBeenCalledTimes(1);
    expect(b.state.restored).toBe(true);
    expect(b.state.dirty).toBe(true);
    expect(b.state.saveError).toMatch(/could not be written/);
    expect(b.isDirty()).toBe(true);
    expect(hasStashedDocxEdits(PATH)).toBe(false);
  });

  it("saves after a running save what was typed before the tab closed", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    const release = slowNextSave();
    const cmdS = controller.save();
    await until(() => saves.length === 1);
    last().transact(); // more typing, then another tab
    const closing = controller.close();
    await settle();
    release();
    expect(await cmdS).toBe(false);
    expect(await closing).toBe("saved");
    expect(saves).toHaveLength(2);
    expect(hasStashedDocxEdits(PATH)).toBe(false);
    expect(controller.state.saveError).toBeNull();
  });
});

describe("changes on disk", () => {
  it("decides: nothing, reload a clean tab, or ask a dirty one", () => {
    const at = (modifiedMs: number | null, exists = true): DocxFileState => ({
      exists,
      modifiedMs,
      size: 1,
      openInWord: false,
      readOnly: false,
    });
    expect(decideOnDiskChange(1000, at(1000), false)).toBe("none");
    expect(decideOnDiskChange(1000, at(1000), true)).toBe("none");
    expect(decideOnDiskChange(1000, at(2000), false)).toBe("reload");
    expect(decideOnDiskChange(1000, at(2000), true)).toBe("ask");
    expect(decideOnDiskChange(1000, at(null, false), true)).toBe("none");
    expect(decideOnDiskChange(null, at(2000), false)).toBe("none");
  });

  it("reloads a clean tab when the file changed elsewhere", async () => {
    const { controller } = makeController();
    await controller.open();
    expect(sd.made).toHaveLength(1);
    fileOnDisk({ modifiedMs: 5000 });
    await controller.checkDisk();
    expect(sd.made).toHaveLength(2);
    expect(sd.made[0].destroy).toHaveBeenCalled();
    expect(readFile).toHaveBeenCalledTimes(2);
  });

  it("asks a tab with unsaved edits, and Keep mine keeps them", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    fileOnDisk({ modifiedMs: 5000 });
    await controller.checkDisk();
    expect(sd.made).toHaveLength(1);
    expect(controller.state.changedOnDisk).toBe(true);
    await controller.keepMine();
    expect(controller.state.changedOnDisk).toBe(false);
    await controller.checkDisk();
    expect(controller.state.changedOnDisk).toBe(false);
    expect(controller.isDirty()).toBe(true);
  });

  it("Reload drops the tab's edits and reads the file again", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    fileOnDisk({ modifiedMs: 5000 });
    await controller.checkDisk();
    await controller.reload();
    expect(controller.isDirty()).toBe(false);
    expect(controller.state.changedOnDisk).toBe(false);
    expect(sd.made).toHaveLength(2);
  });

  it("does not take its own save for a change on disk", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    await controller.save();
    await controller.checkDisk();
    expect(sd.made).toHaveLength(1);
    expect(controller.state.changedOnDisk).toBe(false);
  });

  it("does not take a look that overlapped its own save for a change", async () => {
    const { controller } = makeController();
    await controller.open();
    last().transact();
    const stale = { ...disk };
    let answerLook: (state: DocxFileState) => void = () => {};
    vi.mocked(invoke).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          answerLook = resolve as (state: DocxFileState) => void;
        }) as never,
    );
    // The look reads the file's time before the save lands, and answers after.
    const looking = controller.checkDisk();
    await controller.save();
    answerLook(stale);
    await looking;
    expect(controller.state.changedOnDisk).toBe(false);
    expect(sd.made).toHaveLength(1);
  });

  it("says when Word has the file open", async () => {
    const { controller } = makeController();
    await controller.open();
    expect(controller.state.openInWord).toBe(false);
    fileOnDisk({ openInWord: true });
    await controller.checkDisk();
    expect(controller.state.openInWord).toBe(true);
    fileOnDisk({ openInWord: false });
    await controller.checkDisk();
    expect(controller.state.openInWord).toBe(false);
  });
});

describe("equations", () => {
  it("knows SuperDoc's refusal of an equation edit", () => {
    expect(
      isEquationEditError({ error: new Error("Cannot edit math node") }),
    ).toBe(true);
    expect(isEquationEditError({ error: "OMML equation is read-only" })).toBe(
      true,
    );
    expect(isEquationEditError({ error: new Error("Network failure") })).toBe(
      false,
    );
    expect(isEquationEditError(null)).toBe(false);
  });

  it("shows a short note, not the error, and hides it again", async () => {
    vi.useFakeTimers();
    try {
      const { controller } = makeController();
      const opening = controller.open();
      await vi.runAllTicks();
      await vi.advanceTimersByTimeAsync(0);
      await opening;
      last().config.onException({
        error: new Error("edit inside an equation failed"),
      });
      expect(controller.state.equationNote).toBe(true);
      expect(controller.state.saveError).toBeNull();
      vi.advanceTimersByTime(6500);
      expect(controller.state.equationNote).toBe(false);
      last().config.onException({ error: new Error("something else") });
      expect(controller.state.equationNote).toBe(false);
    } finally {
      vi.useRealTimers();
    }
    expect(EQUATION_NOTE).toBe(
      "Equations can't be edited here. Edit them in Word, or write the math in LaTeX.",
    );
  });

  it("guards equations from being replaced or deleted", async () => {
    const { controller } = makeController();
    await controller.open();
    const [plugin] = last().plugins;
    expect(plugin.spec.key.name).toBe("thelabEquationGuard");
    const math = { type: { name: "mathBlock" }, nodeSize: 1 };
    const node = (children: any[]) => ({
      type: { name: "doc" },
      nodeSize: 10,
      descendants: (f: any) => children.forEach((c) => f(c)),
      nodesBetween: (_a: number, _b: number, f: any) =>
        children.forEach((c) => f(c)),
    });
    const withMath = node([math]);
    const withoutMath = node([]);
    const tr = {
      docChanged: true,
      steps: [{ from: 3, to: 4 }],
      docs: [withMath],
      doc: withoutMath,
      getMeta: () => undefined,
    };
    expect(plugin.spec.filterTransaction(tr, { doc: withMath })).toBe(false);
    expect(controller.state.equationNote).toBe(true);
    // Typing elsewhere passes.
    const typing = { ...tr, docs: [withoutMath], doc: withMath };
    expect(plugin.spec.filterTransaction(typing, { doc: withMath })).toBe(true);
    // Undo may remove an equation a paste added.
    expect(
      removesEquation({ ...tr, getMeta: () => ({}) } as any, withMath as any),
    ).toBe(false);
  });

  it("lets a deleted passage take its equation with it, and accepting or rejecting", () => {
    // A paragraph: 10 characters of text, then an equation at 11.
    const text = { type: { name: "text" }, isText: true, nodeSize: 10 };
    const math = { type: { name: "mathInline" }, nodeSize: 1 };
    const doc = (withEquation: boolean) => ({
      type: { name: "doc" },
      nodeSize: 20,
      descendants: (f: any) => {
        f(text, 1);
        if (withEquation) f(math, 11);
      },
      nodesBetween: (from: number, to: number, f: any) => {
        if (from < 11 && to > 1) f(text, 1);
        if (withEquation && from <= 11 && to > 11) f(math, 11);
      },
    });
    const removing = (from: number, to: number, meta?: string) => ({
      docChanged: true,
      steps: [{ from, to }],
      docs: [doc(true)],
      doc: doc(false),
      getMeta: (key: string) => (key === meta ? true : undefined),
    });
    // The equation alone: refused.
    expect(removesEquation(removing(11, 12) as any, doc(true) as any)).toBe(
      true,
    );
    // Text and the equation together: a deletion, allowed.
    expect(removesEquation(removing(5, 12) as any, doc(true) as any)).toBe(
      false,
    );
    // Accepting or rejecting a tracked change skips change tracking.
    expect(
      removesEquation(
        removing(11, 12, "skipTrackChanges") as any,
        doc(true) as any,
      ),
    ).toBe(false);
  });
});
