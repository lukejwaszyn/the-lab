import { create } from "zustand";
import { persist } from "zustand/middleware";
import { movedPath } from "@/lib/docx/docx-session";

/** SuperDoc's three modes, as the Word tab's toolbar names them. */
export type DocxMode = "editing" | "suggesting" | "viewing";

export const DOCX_MODES: { mode: DocxMode; label: string; hint: string }[] = [
  { mode: "editing", label: "Editing", hint: "Edit the document directly" },
  {
    mode: "suggesting",
    label: "Suggesting",
    hint: "Edits become tracked changes under your name",
  },
  { mode: "viewing", label: "Viewing", hint: "Read only" },
];

interface DocxPrefsState {
  /** The mode chosen for each file, by absolute path. */
  modes: Record<string, DocxMode>;
  setMode: (absolutePath: string, mode: DocxMode) => void;
  /** A file or folder was renamed or moved from `from` to `to`. */
  movePaths: (from: string, to: string) => void;
}

/** A file opens in the mode last chosen for it, and in Editing at first. */
export function modeFor(
  modes: Record<string, DocxMode>,
  absolutePath: string,
): DocxMode {
  const mode = modes[absolutePath];
  return mode === "suggesting" || mode === "viewing" ? mode : "editing";
}

export const useDocxPrefsStore = create<DocxPrefsState>()(
  persist(
    (set) => ({
      modes: {},
      setMode: (absolutePath, mode) =>
        set((s) => {
          const modes = { ...s.modes };
          // Editing is the default, so it needs no entry.
          if (mode === "editing") delete modes[absolutePath];
          else modes[absolutePath] = mode;
          return { modes };
        }),
      movePaths: (from, to) =>
        set((s) => {
          const modes: Record<string, DocxMode> = {};
          let moved = false;
          for (const [path, mode] of Object.entries(s.modes)) {
            const next = movedPath(path, from, to);
            if (next !== path) moved = true;
            modes[next] = mode;
          }
          return moved ? { modes } : s;
        }),
    }),
    { name: "thelab-docx-tabs", version: 1 },
  ),
);
