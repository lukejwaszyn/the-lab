/**
 * The Lab: the open Word tabs, so that the rest of the app can save them
 * when it must: before a file or project is renamed or moved, and before
 * the window closes or the app quits. A Word tab saves on ⌘S and when it
 * stops being shown (another tab, the project closing or switching); it
 * has no timer, because every save rewrites the whole package.
 *
 * Nothing here loads SuperDoc.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { toast } from "sonner";
import { createLogger } from "@/lib/debug/logger";

const log = createLogger("docx");

export interface DocxSession {
  /** The file the tab has open. */
  readonly absolutePath: string;
  isDirty(): boolean;
  /** Writes the tab's edits; true when nothing is left unsaved. */
  save(): Promise<boolean>;
  /** Drops the tab's unsaved edits (its file is being deleted). */
  discard(): void;
}

const sessions = new Set<DocxSession>();

export function registerDocxSession(session: DocxSession): () => void {
  sessions.add(session);
  updateCloseGuard();
  return () => {
    sessions.delete(session);
    updateCloseGuard();
  };
}

export function hasUnsavedDocx(): boolean {
  for (const s of sessions) if (s.isDirty()) return true;
  return false;
}

/** Whether `path` is `prefix` itself or a file inside the folder `prefix`. */
export function isAtOrUnder(path: string, prefix: string): boolean {
  if (path === prefix) return true;
  const folder = prefix.replace(/[\\/]+$/, "");
  return path.startsWith(`${folder}/`) || path.startsWith(`${folder}\\`);
}

/**
 * Saves every Word tab with unsaved edits (those for files at or under
 * `path`, when it is given). True when none is left unsaved.
 */
export async function flushDocxEdits(path?: string): Promise<boolean> {
  const dirty = [...sessions].filter(
    (s) => s.isDirty() && (path == null || isAtOrUnder(s.absolutePath, path)),
  );
  const results = await Promise.all(
    dirty.map((s) =>
      s.save().catch((err) => {
        log.error("A Word tab could not save", { error: String(err) });
        return false;
      }),
    ),
  );
  return results.every(Boolean);
}

/** Drops the unsaved edits of the Word tabs for files at or under `path`. */
export function discardDocxEdits(path: string): void {
  for (const s of sessions) {
    if (isAtOrUnder(s.absolutePath, path)) s.discard();
  }
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
 * saves the unsaved Word tabs first. If a save fails, the window stays
 * open once, saying so; a second close discards.
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
      "A Word file could not be saved, so the window stayed open. Close it again to discard the changes.",
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

/** For tests: forget every tab and listener. */
export function resetDocxSessionsForTest(): void {
  sessions.clear();
  unlisten?.();
  unlisten = null;
  listening = null;
  closeAnyway = false;
}
