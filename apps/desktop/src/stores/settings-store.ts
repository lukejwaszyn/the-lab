import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * The PDF preview's engine menu. The Lab compiles through thelab-core:
 * "auto" lets it choose per document (latexmk for biblatex with biber and
 * for LuaLaTeX, Tectonic otherwise, whichever is installed), "tectonic" is
 * the installed `tectonic` program, and "texlive" is TeX Live's `latexmk`.
 */
export type CompilerBackend = "auto" | "tectonic" | "texlive";

interface SettingsState {
  compilerBackend: CompilerBackend;
  setCompilerBackend: (backend: CompilerBackend) => void;
  vimMode: boolean;
  setVimMode: (enabled: boolean) => void;
  /** The Lab: the author of the tracked changes made in Word tabs. */
  trackChangesAuthor: string;
  /** Whether the account's full name has been offered for it (once). */
  trackChangesAuthorOffered: boolean;
  setTrackChangesAuthor: (name: string) => void;
  offerTrackChangesAuthor: (accountName: string | null) => void;
}

/**
 * Settings saved before version 1 had "tectonic" as the default, and it
 * meant the Tectonic library built into the app. That library is gone, so
 * a saved "tectonic" (almost always the old default) becomes "auto" once.
 */
export function migrateSettings(
  persisted: unknown,
  version: number,
): Partial<SettingsState> {
  const state = (persisted ?? {}) as Partial<SettingsState>;
  if (version < 1 && state.compilerBackend === "tectonic") {
    return { ...state, compilerBackend: "auto" };
  }
  return state;
}

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      compilerBackend: "auto",
      setCompilerBackend: (backend) => set({ compilerBackend: backend }),
      vimMode: false,
      setVimMode: (enabled) => set({ vimMode: enabled }),
      trackChangesAuthor: "",
      trackChangesAuthorOffered: false,
      setTrackChangesAuthor: (name) =>
        set({ trackChangesAuthor: name, trackChangesAuthorOffered: true }),
      offerTrackChangesAuthor: (accountName) =>
        set((s) =>
          s.trackChangesAuthorOffered
            ? {}
            : {
                trackChangesAuthorOffered: true,
                trackChangesAuthor:
                  s.trackChangesAuthor || (accountName ?? "").trim(),
              },
        ),
    }),
    {
      name: "claude-prism-settings",
      version: 1,
      migrate: (persisted, version) =>
        migrateSettings(persisted, version) as SettingsState,
    },
  ),
);

/**
 * Fills "Your name for tracked changes" once, from the account's full name
 * (empty when it cannot be read). Later the name is only what the user sets.
 */
export async function fillTrackChangesAuthorOnce(): Promise<void> {
  if (useSettingsStore.getState().trackChangesAuthorOffered) return;
  let name: string | null = null;
  try {
    name = await invoke<string | null>("account_full_name");
  } catch {
    name = null;
  }
  useSettingsStore.getState().offerTrackChangesAuthor(name);
}
