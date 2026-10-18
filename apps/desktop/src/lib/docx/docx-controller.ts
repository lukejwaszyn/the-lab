/**
 * The Lab: one Word tab's document. It reads the file, hands its bytes to
 * SuperDoc as a File, tracks unsaved edits, saves (keeping what was not
 * edited byte for byte, `preserve.ts`), and watches the file on disk: a
 * change made elsewhere reloads a tab with no unsaved edits and asks about
 * one with them, and Word's owner file beside it means Word has it open.
 *
 * It saves on ⌘S, after a pause in editing, and when the tab closes. Only
 * ⌘S may rewrite the whole file, when the unedited parts cannot be kept,
 * and only after asking; no save writes over a change made on disk that
 * the tab has not shown.
 */
import { readFile } from "@tauri-apps/plugin-fs";
import type { SuperDoc } from "superdoc";
import { createLogger } from "@/lib/debug/logger";
import type { DocxMode } from "@/stores/docx-store";
import {
  fileState,
  saveDocxBytes,
  stashDocxEdits,
  takeStashedDocxEdits,
  trackDocxClose,
  waitForDocxClose,
  type DocxFileState,
  type DocxSession,
  type SaveOptions,
  type StashedDocxEdits,
} from "./docx-session";
import { loadSuperDoc } from "./superdoc-loader";

export { fileState, saveDocxBytes, type DocxFileState } from "./docx-session";

const log = createLogger("docx");

export const DOCX_MIME =
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

export const EQUATION_NOTE =
  "Equations can't be edited here. Edit them in Word, or write the math in LaTeX.";
export const OPEN_IN_WORD_NOTE =
  "This file is open in Word. Editing it in both places will conflict.";
export const READ_ONLY_NOTE =
  "This file is read-only, so it opens for reading only.";
export const WHOLE_FILE_QUESTION =
  "This save has to rewrite the whole file, which can change its formatting. Save anyway, or Cancel and keep editing.";
export const WHOLE_FILE_HELD_NOTE =
  "Not saved: The Lab can save this file only by rewriting all of it, which can change its formatting. Your edits are kept here. Press ⌘S to choose.";
export const KEPT_EDITS_NOTE =
  "These are your unsaved edits from before. They have not been saved yet.";

export interface DocxTabState {
  phase: "loading" | "ready" | "error";
  /** Why the file could not be opened. */
  error: string | null;
  /** Why the last save failed. */
  saveError: string | null;
  dirty: boolean;
  saving: boolean;
  /** The file changed on disk while the tab had unsaved edits. */
  changedOnDisk: boolean;
  openInWord: boolean;
  /** The file cannot be written, so it opens in Viewing. */
  readOnly: boolean;
  /** An edit inside an equation was refused. */
  equationNote: boolean;
  /**
   * Only rewriting the whole file can save the edits (`preserve.ts` could
   * not keep the unedited parts): ⌘S asks first ("ask"); a save the user
   * did not ask for keeps them unsaved and says so ("held").
   */
  wholeFile: "ask" | "held" | null;
  /** The tab shows edits kept from an earlier failed save. */
  restored: boolean;
}

export const INITIAL_TAB_STATE: DocxTabState = {
  phase: "loading",
  error: null,
  saveError: null,
  dirty: false,
  saving: false,
  changedOnDisk: false,
  openInWord: false,
  readOnly: false,
  equationNote: false,
  wholeFile: null,
  restored: false,
};

export type DiskDecision = "none" | "reload" | "ask";

/**
 * What a tab does when it looks at its file again (the window regained
 * focus, or the files were refreshed, as after each Claude run): nothing
 * while the file has the time it had when the tab read or wrote it,
 * otherwise reload a tab with no unsaved edits and ask about one with them.
 */
export function decideOnDiskChange(
  knownModifiedMs: number | null,
  disk: DocxFileState,
  dirty: boolean,
): DiskDecision {
  if (!disk.exists || disk.modifiedMs == null || knownModifiedMs == null) {
    return "none";
  }
  if (disk.modifiedMs === knownModifiedMs) return "none";
  return dirty ? "ask" : "reload";
}

/** Whether SuperDoc's exception is an edit refused inside an equation. */
export function isEquationEditError(payload: unknown): boolean {
  const error = (payload as { error?: unknown } | null)?.error ?? payload;
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  return /math|equation|omml/i.test(message);
}

async function exportDocx(sd: SuperDoc): Promise<Uint8Array> {
  const blob = await sd.export({
    exportType: ["docx"],
    commentsType: "external",
    isFinalDoc: false,
    triggerDownload: false,
  });
  return new Uint8Array(await blob.arrayBuffer());
}

/** The node types SuperDoc gives Word's equations. */
const MATH_NODES = new Set(["mathInline", "mathBlock"]);

interface PmNode {
  type: { name: string };
  nodeSize: number;
  isText?: boolean;
  descendants(f: (node: PmNode, pos: number) => boolean | undefined): void;
  nodesBetween(
    from: number,
    to: number,
    f: (node: PmNode, pos: number) => boolean | undefined,
  ): void;
}
interface PmStep {
  from?: number;
  to?: number;
}
interface PmTransaction {
  docChanged: boolean;
  /** The document before the transaction. */
  before: PmNode;
  steps: PmStep[];
  docs: PmNode[];
  doc: PmNode;
  getMeta(key: string): unknown;
}

function countMath(doc: PmNode): number {
  let n = 0;
  doc.descendants((node) => {
    if (MATH_NODES.has(node.type.name)) n++;
    return true;
  });
  return n;
}

/**
 * Whether `tr` edits an equation: it removes one, and what it removes is
 * the equation alone (selected and typed over, or deleted with Backspace).
 * SuperDoc shows Word's equations but cannot edit them, and typing would
 * replace one. Deleting a passage that holds an equation is allowed, as
 * are undo and redo, and accepting or rejecting tracked changes (which
 * SuperDoc marks to skip change tracking).
 */
export function removesEquation(tr: PmTransaction, before: PmNode): boolean {
  if (!tr.docChanged || tr.getMeta("history$") !== undefined) return false;
  if (tr.getMeta("skipTrackChanges") === true) return false;
  let equationAlone = false;
  tr.steps.forEach((step, i) => {
    if (equationAlone || step.from == null || step.to == null) return;
    const { from, to } = step;
    if (to <= from) return;
    let math = false;
    let text = 0;
    tr.docs[i]?.nodesBetween(from, to, (node, pos) => {
      if (MATH_NODES.has(node.type.name)) {
        math = true;
        return false;
      }
      if (node.isText) {
        text += Math.min(to, pos + node.nodeSize) - Math.max(from, pos);
      }
      return true;
    });
    if (math && text === 0) equationAlone = true;
  });
  return equationAlone && countMath(tr.doc) < countMath(before);
}

export interface DocxControllerOptions {
  absolutePath: string;
  name: string;
  editor: HTMLElement;
  /** The id of the element SuperDoc puts its toolbar in. */
  toolbarId: string;
  mode: DocxMode;
  author: string;
  onChange: (state: DocxTabState) => void;
  /** SuperDoc changed the mode itself (from its own menu). */
  onModeChange?: (mode: DocxMode) => void;
  /** The pause in editing after which the tab saves itself; 0 for never. */
  autosaveMs?: number;
}

/** How long a document may take to open before the tab gives up. */
export const OPEN_TIMEOUT_MS = 60_000;

/**
 * A Word tab saves itself after this pause in editing, the same as a text
 * tab (`scheduleAutoSave` in `document-store.ts`). It bounds what a quit
 * that closes no window (from the Dock, logging out, shutting down) can
 * lose: tao tells the app only as it ends.
 */
export const AUTOSAVE_MS = 2000;

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** SuperDoc's comment events that change what a save writes. */
const COMMENT_CHANGES = new Set([
  "add",
  "update",
  "deleted",
  "resolved",
  "change-accepted",
  "change-rejected",
]);

/**
 * What became of a tab's edits when it closed: saved, kept unsaved for
 * when the file is opened again (`docx-session.ts`), or lost.
 */
export type CloseResult = "saved" | "kept" | "lost";

/** The bytes a save would write, and what they are. */
interface SaveBytes {
  bytes: Uint8Array;
  /** SuperDoc's export they were made from (the next save's baseline). */
  edited: Uint8Array;
  /** They are SuperDoc's whole export, not the file with the edits spliced in. */
  whole: boolean;
  /** `changes` when they were exported. */
  changes: number;
}

export class DocxController implements DocxSession {
  absolutePath: string;
  state: DocxTabState = { ...INITIAL_TAB_STATE };

  private readonly options: DocxControllerOptions;
  private sd: SuperDoc | null = null;
  /** The bytes on disk, as last read or written. */
  private original: Uint8Array | null = null;
  /** SuperDoc's export of `original`, taken when it loaded. */
  private baseline: Uint8Array | null = null;
  /** Doc-changing transactions since the document loaded. */
  private changes = 0;
  /** `changes` when the file was last read or written. */
  private savedChanges = 0;
  /** The tab shows edits kept from a failed save, not yet written. */
  private restored = false;
  /**
   * The bytes the tab shows are SuperDoc's whole export (edits kept when
   * only rewriting the whole file could save them), so any save of them
   * rewrites the whole file.
   */
  private wholeBase = false;
  /** What the last failed save meant to write. */
  private unsaved: SaveBytes | null = null;
  private knownModifiedMs: number | null = null;
  private mode: DocxMode;
  private readOnly = false;
  private generation = 0;
  private discarded = false;
  private closed = false;
  private saving: Promise<boolean> | null = null;
  private noteTimer: ReturnType<typeof setTimeout> | null = null;
  private autosaveTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly autosaveMs: number;

  constructor(options: DocxControllerOptions) {
    this.options = options;
    this.absolutePath = options.absolutePath;
    this.mode = options.mode;
    this.autosaveMs = options.autosaveMs ?? AUTOSAVE_MS;
  }

  private update(patch: Partial<DocxTabState>) {
    this.state = { ...this.state, ...patch };
    this.options.onChange(this.state);
  }

  isDirty(): boolean {
    return (
      !this.discarded && (this.restored || this.changes !== this.savedChanges)
    );
  }

  /** Its file was renamed or moved; later saves write it there. */
  moveTo(absolutePath: string) {
    this.absolutePath = absolutePath;
  }

  private noteChange() {
    this.changes++;
    if (!this.state.dirty) this.update({ dirty: true });
    this.scheduleAutosave();
  }

  /** (Re)starts the pause after which a dirty tab saves itself. */
  private scheduleAutosave() {
    this.cancelAutosave();
    if (this.closed || this.autosaveMs <= 0) return;
    this.autosaveTimer = setTimeout(() => {
      this.autosaveTimer = null;
      if (!this.closed && this.isDirty()) void this.save({ implicit: true });
    }, this.autosaveMs);
  }

  private cancelAutosave() {
    if (this.autosaveTimer) clearTimeout(this.autosaveTimer);
    this.autosaveTimer = null;
  }

  /**
   * Reads the file and shows it, or the edits kept for it from a failed
   * save (`docx-session.ts`).
   */
  async open(): Promise<void> {
    const generation = ++this.generation;
    this.cancelAutosave();
    this.changes = 0;
    this.savedChanges = 0;
    this.restored = false;
    this.wholeBase = false;
    this.unsaved = null;
    this.update({ ...INITIAL_TAB_STATE });
    let kept: StashedDocxEdits | null = null;
    try {
      // A tab on this file that is closing may still be saving it.
      await waitForDocxClose(this.absolutePath);
      if (generation !== this.generation || this.closed) return;
      kept = takeStashedDocxEdits(this.absolutePath);
      const [bytes, disk] = await Promise.all([
        kept ? kept.bytes : readFile(this.absolutePath),
        fileState(this.absolutePath),
      ]);
      const { SuperDoc, Extensions } = await loadSuperDoc();
      if (generation !== this.generation || this.closed) {
        if (kept) stashDocxEdits(kept);
        return;
      }
      this.destroyEditor();
      this.original = new Uint8Array(bytes);
      this.baseline = null;
      this.changes = 0;
      this.savedChanges = 0;
      this.knownModifiedMs = kept ? kept.knownModifiedMs : disk.modifiedMs;
      this.readOnly = disk.readOnly;
      this.update({ openInWord: disk.openInWord, readOnly: disk.readOnly });

      let ready = false;
      const sd = await new Promise<SuperDoc>((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("SuperDoc did not finish opening the file")),
          OPEN_TIMEOUT_MS,
        );
        const file = new File([this.original as BlobPart], this.options.name, {
          type: DOCX_MIME,
        });
        const current = () => ready && generation === this.generation;
        this.sd = new SuperDoc({
          selector: this.options.editor,
          toolbar: `#${this.options.toolbarId}`,
          document: file,
          documentMode: this.readOnly ? "viewing" : this.mode,
          role: "editor",
          user: { name: this.options.author.trim() || "Author", email: "" },
          // SuperDoc reports each opened document to its makers unless told not to.
          telemetry: { enabled: false },
          contained: true,
          modules: {
            toolbar: {
              // The tab's own toolbar picks the mode; Claude is the app's AI.
              excludeItems: ["documentMode", "ai"],
              responsiveToContainer: true,
            },
          },
          onReady: ({ superdoc }) => {
            ready = true;
            clearTimeout(timer);
            resolve(superdoc);
          },
          onTransaction: (event) => {
            // SuperDoc reports every transaction, a refused one too (as by
            // the equation guard); only one that changed the document counts.
            const tr = event?.transaction as unknown as
              | PmTransaction
              | undefined;
            const editor = (event?.sourceEditor ?? event?.editor) as
              | { state?: { doc?: unknown } }
              | undefined;
            if (
              current() &&
              tr?.docChanged &&
              editor?.state?.doc !== tr.before
            ) {
              this.noteChange();
            }
          },
          onCommentsUpdate: (event) => {
            // A comment added, answered, resolved or deleted changes the
            // file even when the text does not. Tracked changes show as
            // comments too; their transactions already count.
            const comment = event?.comment as
              | { trackedChange?: boolean }
              | undefined;
            if (
              current() &&
              COMMENT_CHANGES.has(event?.type ?? "") &&
              !comment?.trackedChange
            ) {
              this.noteChange();
            }
          },
          onException: (payload) => {
            if (!ready && "stage" in (payload ?? {})) {
              clearTimeout(timer);
              reject(payload.error);
            } else {
              this.onException(payload);
            }
          },
          onContentError: ({ error }) => {
            if (!ready) {
              clearTimeout(timer);
              reject(error);
            } else {
              log.warn("SuperDoc content error", { error: String(error) });
            }
          },
        });
      });
      if (generation !== this.generation || this.closed) {
        sd.destroy();
        if (kept) stashDocxEdits(kept);
        return;
      }
      sd.on("document-mode-change", (payload) => {
        const mode = (payload as { documentMode?: DocxMode })?.documentMode;
        if (mode && mode !== this.mode && !this.readOnly) {
          this.mode = mode;
          this.options.onModeChange?.(mode);
        }
      });
      this.guardEquations(sd, Extensions);
      // The document shows once the baseline is taken, so that no edit
      // comes before it.
      await this.takeBaseline(sd, generation);
      if (generation !== this.generation || this.closed) {
        if (kept) stashDocxEdits(kept);
        return;
      }
      if (kept) {
        this.restored = true;
        this.wholeBase = kept.wholeFile === true;
        const reason = kept.conflict || kept.wholeFile ? null : kept.reason;
        const changedSince =
          disk.exists &&
          disk.modifiedMs != null &&
          kept.knownModifiedMs != null &&
          disk.modifiedMs !== kept.knownModifiedMs;
        const wholeFile = this.wholeBase ? "held" : null;
        kept = null;
        this.update({
          phase: "ready",
          dirty: true,
          restored: true,
          changedOnDisk: changedSince,
          wholeFile,
          saveError: reason ? `Not saved: ${reason}` : null,
        });
      } else {
        this.update({ phase: "ready" });
      }
    } catch (err) {
      if (kept) stashDocxEdits(kept);
      if (generation !== this.generation) return;
      log.error("A Word file could not be opened", {
        path: this.absolutePath,
        error: String(err),
      });
      this.update({
        phase: "error",
        error: "This file could not be opened as a Word document.",
      });
    }
  }

  /**
   * SuperDoc's export of the file as it loaded, which a save compares with
   * the edited export to find what changed. An edit while it is taken would
   * blur that line, so then there is none, and a save writes SuperDoc's
   * whole export.
   */
  private async takeBaseline(sd: SuperDoc, generation: number) {
    const before = this.changes;
    try {
      const baseline = await exportDocx(sd);
      if (generation === this.generation && this.changes === before) {
        this.baseline = baseline;
      }
    } catch (err) {
      log.warn("No baseline export", { error: String(err) });
    }
  }

  private guardEquations(
    sd: SuperDoc,
    Extensions: Awaited<ReturnType<typeof loadSuperDoc>>["Extensions"],
  ) {
    const editor = sd.activeEditor;
    if (!editor) return;
    try {
      const plugin = new Extensions.Plugin({
        key: new Extensions.PluginKey("thelabEquationGuard"),
        filterTransaction: (tr, state) => {
          if (
            removesEquation(
              tr as unknown as PmTransaction,
              state.doc as unknown as PmNode,
            )
          ) {
            this.showEquationNote();
            return false;
          }
          return true;
        },
      });
      editor.registerPlugin(plugin);
    } catch (err) {
      log.warn("Could not guard equations", { error: String(err) });
    }
  }

  private onException(payload: unknown) {
    if (isEquationEditError(payload)) {
      this.showEquationNote();
      return;
    }
    // Never a raw error in the tab; the log keeps it.
    log.warn("SuperDoc exception", {
      error: String((payload as { error?: unknown })?.error ?? payload),
    });
  }

  showEquationNote() {
    if (this.noteTimer) clearTimeout(this.noteTimer);
    this.update({ equationNote: true });
    this.noteTimer = setTimeout(() => {
      this.noteTimer = null;
      this.update({ equationNote: false });
    }, 6000);
  }

  setMode(mode: DocxMode) {
    this.mode = mode;
    this.sd?.setDocumentMode(this.readOnly ? "viewing" : mode);
  }

  /**
   * Saves unsaved edits; true when nothing is left unsaved. One save runs
   * at a time: a save asked for while one runs waits for it, then saves
   * what is still unsaved (edits made meanwhile).
   *
   * No save writes over a change made on disk that the tab has not shown:
   * it shows it instead (Reload or Keep mine). An implicit save (a pause
   * in editing, leaving the tab, renaming, closing the window) also does
   * not write over a change the tab is still asking about, which ⌘S does,
   * as Keep mine would, and it never rewrites the whole file.
   */
  async save(options: SaveOptions = {}): Promise<boolean> {
    while (this.saving) await this.saving;
    if (!this.isDirty()) return true;
    if (options.implicit && this.state.changedOnDisk) return false;
    const saving = this.saveNow(options).finally(() => {
      if (this.saving === saving) this.saving = null;
    });
    this.saving = saving;
    return saving;
  }

  /** ⌘S's question answered: rewrite the whole file. */
  saveWholeFile(): Promise<boolean> {
    this.update({ wholeFile: null });
    return this.save({ wholeFile: true });
  }

  /** ⌘S's question answered: keep editing, unsaved. */
  keepEditing(): void {
    if (this.state.wholeFile === "ask") this.update({ wholeFile: "held" });
  }

  /** The bytes a save writes now: SuperDoc's export, unchanged parts kept. */
  private async bytesToSave(): Promise<SaveBytes> {
    const sd = this.sd;
    const original = this.original;
    if (!sd || !original) throw new Error("The document is not open.");
    const changes = this.changes;
    const edited = await exportDocx(sd);
    // Loaded with the first save, like SuperDoc on the first open.
    const { preserveUnchanged } = await import("./preserve");
    const result = this.baseline
      ? preserveUnchanged(original, this.baseline, edited)
      : {
          bytes: edited,
          preserved: false,
          reason: "no baseline",
          changedParts: [],
        };
    if (!result.preserved) {
      log.warn("Only SuperDoc's whole export can save this file", {
        reason: result.reason,
      });
    }
    return {
      bytes: result.bytes,
      edited,
      whole: this.wholeBase || !result.preserved,
      changes,
    };
  }

  private async saveNow(options: SaveOptions): Promise<boolean> {
    if (!this.isDirty()) return true;
    if (!this.sd || !this.original) return false;
    this.update({ saving: true, saveError: null });
    try {
      if (!this.state.changedOnDisk) {
        // Changed elsewhere since the tab last looked: show it, never
        // write over it unseen.
        const disk = await fileState(this.absolutePath);
        if (decideOnDiskChange(this.knownModifiedMs, disk, true) !== "none") {
          this.update({ saving: false, changedOnDisk: true });
          return false;
        }
      }
      const save = await this.bytesToSave();
      if (save.whole && !options.wholeFile) {
        // Ask before rewriting the whole file; a save the user did not
        // ask for keeps the edits unsaved and says so.
        const wholeFile =
          options.implicit && this.state.wholeFile !== "ask" ? "held" : "ask";
        this.update({ saving: false, wholeFile });
        return false;
      }
      if (!this.restored && sameBytes(save.bytes, this.original)) {
        // Edited and edited back: the file is already what it should be.
        this.savedChanges = save.changes;
        this.update({ saving: false, dirty: this.isDirty(), wholeFile: null });
        return !this.isDirty();
      }
      this.unsaved = save;
      const disk = await saveDocxBytes(this.absolutePath, save.bytes);
      this.original = save.bytes;
      this.baseline = save.edited;
      this.unsaved = null;
      this.restored = false;
      this.wholeBase = false;
      this.knownModifiedMs = disk.modifiedMs;
      this.savedChanges = save.changes;
      this.update({
        saving: false,
        dirty: this.isDirty(),
        changedOnDisk: false,
        restored: false,
        wholeFile: null,
        openInWord: disk.openInWord,
      });
      return !this.isDirty();
    } catch (err) {
      log.error("A Word file could not be saved", {
        path: this.absolutePath,
        error: String(err),
      });
      this.update({
        saving: false,
        saveError: `Not saved: ${err instanceof Error ? err.message : String(err)}`,
      });
      return false;
    }
  }

  /** Looks at the file on disk again. */
  async checkDisk(): Promise<void> {
    if (this.closed || this.state.phase !== "ready" || this.saving) return;
    const known = this.knownModifiedMs;
    let disk: DocxFileState;
    try {
      disk = await fileState(this.absolutePath);
    } catch {
      return;
    }
    // A save (or reload) meanwhile makes this look stale.
    if (this.closed || this.saving || this.knownModifiedMs !== known) return;
    if (disk.openInWord !== this.state.openInWord) {
      this.update({ openInWord: disk.openInWord });
    }
    const decision = decideOnDiskChange(
      this.knownModifiedMs,
      disk,
      this.isDirty(),
    );
    if (decision === "reload") await this.reload();
    else if (decision === "ask" && !this.state.changedOnDisk) {
      this.update({ changedOnDisk: true });
    }
  }

  /** Drops this tab's edits and reads the file again. */
  async reload(): Promise<void> {
    this.discarded = false;
    await this.open();
  }

  /**
   * Keeps the tab's edits; the next save, ⌘S or after the pause in
   * editing, replaces the file on disk.
   */
  async keepMine(): Promise<void> {
    try {
      this.knownModifiedMs = (await fileState(this.absolutePath)).modifiedMs;
    } catch {
      /* the next look asks again */
    }
    this.update({ changedOnDisk: false });
    if (this.isDirty()) this.scheduleAutosave();
  }

  discard(): void {
    this.discarded = true;
    this.cancelAutosave();
    if (this.state.dirty) this.update({ dirty: false });
  }

  private destroyEditor() {
    const sd = this.sd;
    this.sd = null;
    if (!sd) return;
    try {
      sd.destroy();
    } catch (err) {
      log.warn("SuperDoc did not close cleanly", { error: String(err) });
    }
    this.options.editor.replaceChildren();
    document.getElementById(this.options.toolbarId)?.replaceChildren();
  }

  /**
   * Saves unsaved edits (unless discarded) and closes the document. Edits
   * that cannot be saved, or that would overwrite a change on disk the tab
   * was still asking about, are kept for when the file opens again.
   */
  close(): Promise<CloseResult> {
    const path = this.absolutePath;
    const done = this.closeNow();
    trackDocxClose(path, done);
    return done;
  }

  private async closeNow(): Promise<CloseResult> {
    if (this.closed) return "saved";
    this.cancelAutosave();
    let result: CloseResult = "saved";
    // A save running (⌘S, or after a pause) finishes first, and what was
    // edited after it began is saved after it.
    if (this.isDirty() && !(await this.save({ implicit: true }))) {
      if (this.isDirty()) result = await this.keep();
    }
    this.closed = true;
    this.generation++;
    this.cancelAutosave();
    if (this.noteTimer) clearTimeout(this.noteTimer);
    this.destroyEditor();
    return result;
  }

  /** Keeps the edits a save could not write (`docx-session.ts`). */
  private async keep(): Promise<CloseResult> {
    let save: SaveBytes | null = null;
    try {
      save = await this.bytesToSave();
    } catch (err) {
      // What the failed save meant to write, if nothing was edited since.
      if (this.unsaved?.changes === this.changes) save = this.unsaved;
      if (!save) {
        log.error("Unsaved Word edits could not be exported", {
          error: String(err),
        });
        return "lost";
      }
    }
    const conflict = this.state.changedOnDisk;
    stashDocxEdits({
      absolutePath: this.absolutePath,
      bytes: save.bytes,
      knownModifiedMs: this.knownModifiedMs,
      reason: conflict
        ? "the file changed on disk while it had unsaved edits"
        : save.whole
          ? "saving it would rewrite the whole file"
          : (this.state.saveError?.replace(/^Not saved: /, "") ??
            "the save failed"),
      conflict,
      wholeFile: save.whole,
    });
    return "kept";
  }
}
