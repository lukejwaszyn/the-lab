import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { PythonMap } from "@/lib/python-env";
import {
  type DocumentState,
  type ProjectFile,
  useDocumentStore,
} from "@/stores/document-store";

/**
 * The open project's interpreter map, for the interpreter label. Fetched
 * when a project opens and when its thelab.toml is saved or changes on
 * disk, not on every tab switch: each fetch asks every interpreter for its
 * version. Run asks for the map afresh on its own.
 */
interface PythonEnvState {
  projectRoot: string | null;
  map: PythonMap | null;
  /** Why the map could not be read, e.g. a broken thelab.toml. */
  error: string | null;
  loading: boolean;
  load: (projectRoot: string | null) => Promise<void>;
  reload: () => Promise<void>;
}

let generation = 0;

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const usePythonEnvStore = create<PythonEnvState>()((set, get) => ({
  projectRoot: null,
  map: null,
  error: null,
  loading: false,

  load: async (projectRoot) => {
    const mine = ++generation;
    if (!projectRoot) {
      set({ projectRoot: null, map: null, error: null, loading: false });
      return;
    }
    set((state) =>
      state.projectRoot === projectRoot
        ? { loading: true }
        : { projectRoot, map: null, error: null, loading: true },
    );
    try {
      const map = await invoke<PythonMap>("python_interpreters", {
        projectRoot,
      });
      if (mine === generation) set({ map, error: null, loading: false });
    } catch (err) {
      if (mine === generation) {
        set({ map: null, error: errorText(err), loading: false });
      }
    }
  },

  reload: () => get().load(get().projectRoot),
}));

const CONFIG_FILE = "thelab.toml";

/** What of thelab.toml counts: its saved content, or that it is absent.
 *  Edits count once they are saved. */
export function configKey(files: ProjectFile[], previous: string): string {
  const config = files.find((f) => f.relativePath === CONFIG_FILE);
  if (!config) return "absent";
  if (config.isDirty) return previous;
  return `saved:${config.content ?? ""}`;
}

let watchedRoot: string | null = null;
let watchedKey = "absent";

function follow(state: DocumentState) {
  const key = configKey(state.files, watchedKey);
  if (state.projectRoot !== watchedRoot) {
    watchedRoot = state.projectRoot;
    watchedKey = key;
    void usePythonEnvStore.getState().load(state.projectRoot);
  } else if (key !== watchedKey) {
    watchedKey = key;
    if (state.projectRoot) void usePythonEnvStore.getState().reload();
  }
}

/**
 * Keeps the map in step with the open project and its thelab.toml; App
 * calls it once. (Not done when this module loads: the document store
 * imports the editor, which imports this, so it may not exist yet.)
 * Returns the function that stops it.
 */
export function watchInterpreters(): () => void {
  follow(useDocumentStore.getState());
  return useDocumentStore.subscribe(follow);
}
