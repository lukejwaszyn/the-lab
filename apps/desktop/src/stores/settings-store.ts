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
    }),
    {
      name: "claude-prism-settings",
      version: 1,
      migrate: (persisted, version) =>
        migrateSettings(persisted, version) as SettingsState,
    },
  ),
);
