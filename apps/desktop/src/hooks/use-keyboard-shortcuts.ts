import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getAppZoomAction, shouldHandleAppZoomShortcut } from "@/lib/app-zoom";
import { useDocumentStore } from "@/stores/document-store";
import { usePythonRunStore } from "@/stores/python-run-store";

const IS_MAC =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

export function useKeyboardShortcuts() {
  useEffect(() => {
    const handleZoomKeyDown = (e: KeyboardEvent) => {
      const zoomAction = getAppZoomAction(e);
      if (!zoomAction || !shouldHandleAppZoomShortcut(e.target)) {
        return;
      }

      e.preventDefault();
      e.stopPropagation();
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        const state = useDocumentStore.getState();
        state.setIsSaving(true);
        state.saveCurrentFile().finally(() => {
          setTimeout(() => state.setIsSaving(false), 500);
        });
      }

      if (
        (e.metaKey || e.ctrlKey) &&
        e.shiftKey &&
        e.key.toLowerCase() === "n"
      ) {
        e.preventDefault();
        invoke("create_new_window").catch(console.error);
      }

      // Cmd+R (macOS) / Ctrl+R (others): run the open Python file. While a
      // script runs it starts nothing and says so; it never stops one.
      // Only Cmd on macOS, so Vim's Ctrl+R (redo) keeps working there.
      const runModifier = IS_MAC ? e.metaKey : e.ctrlKey;
      if (
        runModifier &&
        !e.shiftKey &&
        !e.altKey &&
        e.key.toLowerCase() === "r"
      ) {
        e.preventDefault();
        // A held key repeats; one press is one Run.
        if (!e.repeat && useDocumentStore.getState().projectRoot) {
          void usePythonRunStore.getState().run();
        }
      }

      // Cmd+. (macOS) / Ctrl+. (others): stop the running script.
      if (
        runModifier &&
        !e.shiftKey &&
        !e.altKey &&
        (e.key === "." || e.code === "Period")
      ) {
        e.preventDefault();
        void usePythonRunStore.getState().stop();
      }

      // Cmd+X (macOS) / Ctrl+X (others): Capture & Ask
      if (
        (e.metaKey || e.ctrlKey) &&
        e.key.toLowerCase() === "x" &&
        !e.shiftKey &&
        !e.altKey
      ) {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("toggle-capture-mode"));
      }

      // Cmd+Shift+D (macOS) / Ctrl+Shift+D (others): Toggle debug panel
      if (
        (e.metaKey || e.ctrlKey) &&
        e.shiftKey &&
        e.key.toLowerCase() === "d"
      ) {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("toggle-debug-panel"));
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    document.addEventListener("keydown", handleZoomKeyDown, true);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("keydown", handleZoomKeyDown, true);
    };
  }, []);
}
