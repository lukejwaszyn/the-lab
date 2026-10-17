import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isPythonFile } from "@/lib/python-env";
import {
  appendNote,
  appendOutput,
  emptyOutput,
  type IncomingLine,
  type OutputBuffer,
} from "@/lib/run-output";
import { useDocumentStore } from "@/stores/document-store";

/**
 * Run (⌘R) and Stop (⌘.) for the open Python file, and the Output panel's
 * state. The backend (thelab_core::run) runs one script per window from
 * the project root with the interpreter thelab.toml names for its folder.
 * While a script runs, Run starts nothing: a stray ⌘R never ends a long
 * simulation.
 */

export type RunStatus = "idle" | "starting" | "running" | "stopping";

export interface RunEnd {
  code: number | null;
  signal: number | null;
  stopped: boolean;
  seconds: number;
  /** "[exited 0 after 1.2 s]", "[stopped after 4.0 s]". */
  summary: string;
}

interface RunStarted {
  runId: string;
  file: string;
  interpreter: string;
  folder: string | null;
  command: string;
}

interface OutputEvent {
  runId: string;
  lines: IncomingLine[];
  dropped: number;
}

interface FinishedEvent {
  runId: string;
  end: RunEnd;
}

interface PythonRunState {
  status: RunStatus;
  /** The run the panel shows; events of any other run are ignored. */
  runId: string | null;
  /** The script running or run last, relative to the project root. */
  file: string | null;
  startedAt: number | null;
  lastEnd: RunEnd | null;
  output: OutputBuffer;
  collapsed: boolean;
  /** Stop was pressed before the backend had started the script. */
  stopRequested: boolean;

  run: () => Promise<void>;
  stop: () => Promise<void>;
  clear: () => void;
  setCollapsed: (collapsed: boolean) => void;
  receiveOutput: (
    runId: string,
    lines: IncomingLine[],
    dropped: number,
  ) => void;
  receiveEnd: (runId: string, end: RunEnd) => void;
  /** The window's project closed or switched: its script ends. */
  projectChanged: (previous: string | null) => void;
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

let runCounter = 0;

function newRunId(): string {
  runCounter += 1;
  return `run-${Date.now().toString(36)}-${runCounter}-${Math.random()
    .toString(36)
    .slice(2, 10)}`;
}

let listening: Promise<void> | null = null;

/** Listens for the backend's run events, once. */
function ensureListening(): Promise<void> {
  listening ??= Promise.all([
    listen<OutputEvent>("python-run-output", (event) => {
      const { runId, lines, dropped } = event.payload;
      usePythonRunStore.getState().receiveOutput(runId, lines, dropped);
    }),
    listen<FinishedEvent>("python-run-finished", (event) => {
      const { runId, end } = event.payload;
      usePythonRunStore.getState().receiveEnd(runId, end);
    }),
  ]).then(
    () => undefined,
    (err) => {
      listening = null;
      throw err;
    },
  );
  return listening;
}

const idle = {
  status: "idle" as const,
  runId: null,
  file: null,
  startedAt: null,
  lastEnd: null,
  stopRequested: false,
};

export const usePythonRunStore = create<PythonRunState>()((set, get) => ({
  ...idle,
  output: emptyOutput(),
  collapsed: false,

  run: async () => {
    const documents = useDocumentStore.getState();
    const projectRoot = documents.projectRoot;
    if (!projectRoot) return;

    const current = get();
    if (current.status !== "idle") {
      const note = `${current.file ?? "A script"} is still running, so Run started nothing. Stop it first with ⌘. or the Stop button.`;
      const lines = current.output.lines;
      const saidSo = lines[lines.length - 1]?.text === note;
      set((s) => ({
        collapsed: false,
        output: saidSo ? s.output : appendNote(s.output, note, "note"),
      }));
      return;
    }

    const file = documents.files.find((f) => f.id === documents.activeFileId);
    if (!file) return;
    if (!isPythonFile(file.relativePath)) {
      set({
        ...idle,
        collapsed: false,
        output: appendNote(
          emptyOutput(),
          `Run works on Python files (.py); ${file.name} is not one.`,
          "error",
        ),
      });
      return;
    }

    const runId = newRunId();
    set({
      ...idle,
      status: "starting",
      runId,
      file: file.relativePath,
      startedAt: Date.now(),
      collapsed: false,
      output: emptyOutput(),
    });

    try {
      await ensureListening();
      await documents.saveAllFiles();
      const saved = useDocumentStore
        .getState()
        .files.find((f) => f.id === file.id);
      if (saved?.isDirty) {
        throw new Error(`${file.name} could not be saved, so it was not run.`);
      }
      await invoke<RunStarted>("python_run_start", {
        runId,
        projectRoot,
        file: file.relativePath,
      });
    } catch (err) {
      if (get().runId !== runId) return;
      set((s) => ({
        status: "idle",
        startedAt: null,
        stopRequested: false,
        output: appendNote(s.output, errorText(err), "error"),
      }));
      return;
    }

    const after = get();
    if (after.runId !== runId) {
      // The project closed or switched while the script was starting, so
      // closing it may have found nothing to end: end this run now.
      invoke("python_run_close_project", { runId }).catch(() => {});
      return;
    }
    // It may have ended already.
    if (after.status !== "starting") return;
    if (after.stopRequested) {
      set({ status: "stopping" });
      await invoke("python_run_stop").catch(() => {});
      return;
    }
    set({ status: "running" });
  },

  stop: async () => {
    const { status } = get();
    if (status === "starting") {
      set({ stopRequested: true });
      return;
    }
    if (status === "running") set({ status: "stopping" });
    // Asked even when nothing seems to run: a script whose panel was lost
    // (a reloaded window) can still be stopped.
    await invoke("python_run_stop").catch(() => {});
  },

  clear: () => {
    if (get().status !== "idle") return;
    set({ ...idle, output: emptyOutput() });
  },

  setCollapsed: (collapsed) => set({ collapsed }),

  receiveOutput: (runId, lines, dropped) => {
    if (runId !== get().runId) return;
    set((s) => ({ output: appendOutput(s.output, lines, dropped) }));
  },

  receiveEnd: (runId, end) => {
    if (runId !== get().runId) return;
    const tone = end.stopped ? "note" : end.code === 0 ? undefined : "error";
    set((s) => ({
      status: "idle",
      lastEnd: end,
      stopRequested: false,
      output: appendNote(s.output, end.summary, tone),
    }));
  },

  projectChanged: (previous) => {
    if (previous) {
      invoke("python_run_close_project").catch(() => {});
    }
    set({ ...idle, output: emptyOutput(), collapsed: false });
  },
}));

/**
 * Closing or switching the project ends its script (F2's lesson: state
 * dropped without stopping its process left the process waiting forever).
 * App calls it once; it is not done when this module loads, because the
 * document store imports the editor, which imports this. Returns the
 * function that stops it.
 */
export function watchProjectForRuns(): () => void {
  return useDocumentStore.subscribe((state, previous) => {
    if (state.projectRoot !== previous.projectRoot) {
      usePythonRunStore.getState().projectChanged(previous.projectRoot);
    }
  });
}
