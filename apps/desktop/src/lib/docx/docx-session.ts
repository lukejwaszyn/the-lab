/**
 * The Lab: the open Word tabs, so that the rest of the app can save them
 * when it must: before a file or project is renamed or moved, and before
 * the window closes or the app quits. A Word tab saves on ⌘S and when it
 * stops being shown (another tab, the project closing or switching); it
 * has no timer, because every save rewrites the whole package.
 *
 * A tab whose save fails when it stops being shown does not lose its
 * edits: the bytes it meant to write are kept here, its file keeps its
 * dot, and opening the file again shows those edits, unsaved. The same
 * happens when the file changed on disk and the tab was still asking
 * whether to reload: no save made on the way out overwrites that change.
 *
 * Nothing here loads SuperDoc.
 */
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { toast } from "sonner";
import { createLogger } from "@/lib/debug/logger";

const log = createLogger("docx");

export interface SaveOptions {
  /**
   * The save was asked for by leaving the tab, renaming or closing, not
   * by ⌘S: it does not overwrite a change made on disk that the tab has
   * not been told to keep or drop.
   */
  implicit?: boolean;
}

export interface DocxSession {
  /** The file the tab has open. */
  readonly absolutePath: string;
  isDirty(): boolean;
  /** Writes the tab's edits; true when nothing is left unsaved. */
  save(options?: SaveOptions): Promise<boolean>;
  /** Drops the tab's unsaved edits (its file is being deleted). */
  discard(): void;
}

/** A closed Word tab's edits that could not be saved. */
export interface StashedDocxEdits {
  absolutePath: string;
  /** The whole file, as the failed save meant to write it. */
  bytes: Uint8Array;
  /** The file's time on disk when the tab last read or wrote it. */
  knownModifiedMs: number | null;
  /** Why it was not saved. */
  reason: string;
  /** The file changed on disk, and the tab had not been told what to do. */
  conflict: boolean;
}

/** What `docx_file_state` says about a file on disk (`docx.rs`). */
export interface DocxFileState {
  exists: boolean;
  modifiedMs: number | null;
  size: number;
  openInWord: boolean;
  readOnly: boolean;
}

export function fileState(path: string): Promise<DocxFileState> {
  return invoke<DocxFileState>("docx_file_state", { path });
}

/** Writes a Word file in one step (`docx_save` in `docx.rs`). */
export function saveDocxBytes(
  path: string,
  bytes: Uint8Array,
): Promise<DocxFileState> {
  return invoke<DocxFileState>("docx_save", bytes, {
    headers: { path: encodeURIComponent(path) },
  });
}

const sessions = new Set<DocxSession>();
const stashed = new Map<string, StashedDocxEdits>();
const closing = new Map<string, Promise<unknown>>();

export function registerDocxSession(session: DocxSession): () => void {
  sessions.add(session);
  updateCloseGuard();
  return () => {
    sessions.delete(session);
    updateCloseGuard();
  };
}

/** Whether `path` is `prefix` itself or a file inside the folder `prefix`. */
export function isAtOrUnder(path: string, prefix: string): boolean {
  if (path === prefix) return true;
  const folder = prefix.replace(/[\\/]+$/, "");
  return path.startsWith(`${folder}/`) || path.startsWith(`${folder}\\`);
}

// ─── Edits kept after a failed save ───

export function stashDocxEdits(edits: StashedDocxEdits): void {
  stashed.set(edits.absolutePath, edits);
  updateCloseGuard();
}

/** The kept edits for `path`, handed over (to the tab opening it). */
export function takeStashedDocxEdits(path: string): StashedDocxEdits | null {
  const edits = stashed.get(path) ?? null;
  if (edits) {
    stashed.delete(path);
    updateCloseGuard();
  }
  return edits;
}

/** Whether edits to `path` are kept unsaved (for the file tree's dot). */
export function hasStashedDocxEdits(path: string): boolean {
  return stashed.has(path);
}

/** Writes kept edits; false when they are still unsaved. */
async function saveStashed(edits: StashedDocxEdits): Promise<boolean> {
  if (edits.conflict) return false;
  try {
    const disk = await fileState(edits.absolutePath);
    if (
      disk.exists &&
      disk.modifiedMs != null &&
      edits.knownModifiedMs != null &&
      disk.modifiedMs !== edits.knownModifiedMs
    ) {
      // Changed elsewhere since: never overwrite that unasked.
      stashed.set(edits.absolutePath, { ...edits, conflict: true });
      return false;
    }
    await saveDocxBytes(edits.absolutePath, edits.bytes);
    if (stashed.get(edits.absolutePath) === edits) {
      stashed.delete(edits.absolutePath);
    }
    updateCloseGuard();
    return true;
  } catch (err) {
    log.error("Kept Word edits could not be saved", { error: String(err) });
    return false;
  }
}

// ─── Tabs closing ───

/** Notes that the tab for `path` is closing (and may still be saving). */
export function trackDocxClose(path: string, done: Promise<unknown>): void {
  const tracked = done.catch(() => undefined);
  closing.set(path, tracked);
  void tracked.then(() => {
    if (closing.get(path) === tracked) closing.delete(path);
  });
}

/** Waits until a tab closing on `path` has saved (or kept) its edits. */
export async function waitForDocxClose(path: string): Promise<void> {
  for (let p = closing.get(path); p; p = closing.get(path)) {
    await p;
    if (closing.get(path) === p) break;
  }
}

// ─── What the rest of the app calls ───

export function hasUnsavedDocx(): boolean {
  if (stashed.size > 0) return true;
  for (const s of sessions) if (s.isDirty()) return true;
  return false;
}

/**
 * Saves every Word tab with unsaved edits, and edits kept from closed
 * tabs (those for files at or under `path`, when it is given). True when
 * none is left unsaved. `explicit` is ⌘S: it saves over a change made on
 * disk that the tab is still asking about.
 */
export async function flushDocxEdits(
  path?: string,
  { explicit = false }: { explicit?: boolean } = {},
): Promise<boolean> {
  const under = (p: string) => path == null || isAtOrUnder(p, path);
  const dirty = [...sessions].filter(
    (s) => s.isDirty() && under(s.absolutePath),
  );
  const open = new Set([...sessions].map((s) => s.absolutePath));
  const kept = [...stashed.values()].filter(
    (e) => under(e.absolutePath) && !open.has(e.absolutePath),
  );
  const results = await Promise.all([
    ...dirty.map((s) =>
      s.save({ implicit: !explicit }).catch((err) => {
        log.error("A Word tab could not save", { error: String(err) });
        return false;
      }),
    ),
    ...kept.map(saveStashed),
  ]);
  return results.every(Boolean);
}

/** Drops the unsaved edits of the Word tabs for files at or under `path`. */
export function discardDocxEdits(path: string): void {
  for (const s of sessions) {
    if (isAtOrUnder(s.absolutePath, path)) s.discard();
  }
  for (const key of [...stashed.keys()]) {
    if (isAtOrUnder(key, path)) stashed.delete(key);
  }
  updateCloseGuard();
}

// ─── Saving before the window closes (and so before the app quits) ───

/** How long a closing window waits for its Word tabs to save. */
export const CLOSE_SAVE_TIMEOUT_MS = 20_000;

let unlisten: (() => void) | null = null;
let listening: Promise<void> | null = null;
let closeAnyway = false;

function withTimeout<T>(work: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}

/**
 * The window's close request (its close button, ⌘W, or The Lab's Quit)
 * saves the unsaved Word tabs first. If one cannot be saved (a failed
 * save, or a change on disk the tab is still asking about), the window
 * stays open once, saying so; a second close discards.
 */
async function onCloseRequested(event: { preventDefault(): void }) {
  if (closeAnyway || !hasUnsavedDocx()) return;
  const saved = await withTimeout(
    flushDocxEdits(),
    CLOSE_SAVE_TIMEOUT_MS,
    false,
  );
  if (!saved) {
    event.preventDefault();
    closeAnyway = true;
    toast.error(
      "A Word file has edits that were not saved, so the window stayed open. Open it to see why, or close the window again to discard them.",
    );
  }
}

/**
 * Listens for the close request only while a Word tab has unsaved edits,
 * so that otherwise the window closes as it always has.
 */
export function updateCloseGuard(): void {
  const wanted = hasUnsavedDocx();
  if (wanted && !unlisten && !listening) {
    closeAnyway = false;
    listening = (async () => {
      try {
        const stop =
          await getCurrentWindow().onCloseRequested(onCloseRequested);
        if (hasUnsavedDocx()) unlisten = stop;
        else stop();
      } catch (err) {
        log.warn("Could not watch the window's close", { error: String(err) });
      } finally {
        listening = null;
      }
    })();
  } else if (!wanted && unlisten) {
    unlisten();
    unlisten = null;
  }
}

/** For tests: forget every tab, kept edit and listener. */
export function resetDocxSessionsForTest(): void {
  sessions.clear();
  stashed.clear();
  closing.clear();
  unlisten?.();
  unlisten = null;
  listening = null;
  closeAnyway = false;
}
