import { useEffect, useId, useRef, useState } from "react";
import {
  AlertTriangleIcon,
  FileTextIcon,
  InfoIcon,
  Loader2Icon,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { APP_VISIBILITY_RESTORED } from "@/lib/debug/log-store";
import {
  DocxController,
  EQUATION_NOTE,
  INITIAL_TAB_STATE,
  KEPT_EDITS_NOTE,
  OPEN_IN_WORD_NOTE,
  READ_ONLY_NOTE,
  WHOLE_FILE_HELD_NOTE,
  WHOLE_FILE_QUESTION,
  type DocxTabState,
} from "@/lib/docx/docx-controller";
import { registerDocxSession, updateCloseGuard } from "@/lib/docx/docx-session";
import {
  FILES_REFRESHED_EVENT,
  useDocumentStore,
  type ProjectFile,
} from "@/stores/document-store";
import {
  DOCX_MODES,
  modeFor,
  useDocxPrefsStore,
  type DocxMode,
} from "@/stores/docx-store";
import {
  fillTrackChangesAuthorOnce,
  useSettingsStore,
} from "@/stores/settings-store";

let toolbarCount = 0;

/**
 * The Lab's Word tab: SuperDoc in Editing, Suggesting or Viewing mode. It
 * saves on ⌘S, after a pause in editing, and when it closes (another tab,
 * the project closing or switching, the window closing).
 */
export function DocxEditor({ file }: { file: ProjectFile }) {
  const editorRef = useRef<HTMLDivElement>(null);
  const controllerRef = useRef<DocxController | null>(null);
  const [toolbarId] = useState(() => `thelab-docx-toolbar-${++toolbarCount}`);
  const [state, setState] = useState<DocxTabState>(INITIAL_TAB_STATE);
  const mode = useDocxPrefsStore((s) => modeFor(s.modes, file.absolutePath));
  const setMode = useDocxPrefsStore((s) => s.setMode);
  const author = useSettingsStore((s) => s.trackChangesAuthor);
  const fileId = file.id;
  const modeRef = useRef(mode);
  modeRef.current = mode;

  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    let unregister: (() => void) | null = null;
    let cancelled = false;
    let controller: DocxController | null = null;
    // The file tree's dot, while this file's project is still the one open
    // (a closing tab may still be saving when the next project opens).
    const projectRoot = useDocumentStore.getState().projectRoot;
    const markDirty = (dirty: boolean) => {
      const documents = useDocumentStore.getState();
      if (documents.projectRoot === projectRoot) {
        documents.markFileDirty(fileId, dirty);
      }
    };
    (async () => {
      await fillTrackChangesAuthorOnce();
      if (cancelled) return;
      controller = new DocxController({
        absolutePath: file.absolutePath,
        name: file.name,
        editor,
        toolbarId,
        mode: modeRef.current,
        author: useSettingsStore.getState().trackChangesAuthor,
        onModeChange: (next) =>
          useDocxPrefsStore.getState().setMode(file.absolutePath, next),
        onChange: (next) => {
          if (!cancelled) setState(next);
          markDirty(next.dirty);
          updateCloseGuard();
        },
      });
      controllerRef.current = controller;
      unregister = registerDocxSession(controller);
      await controller.open();
    })();
    return () => {
      cancelled = true;
      controllerRef.current = null;
      const closing = controller;
      if (!closing) return;
      // Showing another tab, or the project closing, saves unsaved edits.
      // Edits that cannot be saved are kept, unsaved, for when the file
      // opens again, and the file keeps its dot.
      void closing.close().then((result) => {
        unregister?.();
        markDirty(result === "kept");
        if (result === "kept") {
          toast.warning(
            closing.state.changedOnDisk
              ? `${file.name} changed on disk while you had unsaved edits, so they were not saved over it. Open it again to choose Reload or Keep mine.`
              : closing.state.wholeFile
                ? `${file.name} was not saved, because saving it would rewrite the whole file. Your edits are kept: open it again and press ⌘S to choose.`
                : `${file.name} could not be saved. Your edits are kept: open it again to save them. ${closing.state.saveError ?? ""}`,
          );
        } else if (result === "lost") {
          toast.error(
            `${file.name} could not be saved, so its latest edits were lost. ${closing.state.saveError ?? ""}`,
          );
        }
      });
    };
  }, [file.absolutePath, file.name, fileId, toolbarId]);

  // The mode is remembered per file.
  useEffect(() => {
    controllerRef.current?.setMode(mode);
  }, [mode]);

  // Coming back to the window, and each refresh of the files (as after a
  // Claude run), looks at the file on disk again.
  useEffect(() => {
    const look = () => void controllerRef.current?.checkDisk();
    const onFocus = () => {
      if (document.visibilityState === "visible") look();
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener(APP_VISIBILITY_RESTORED, look);
    window.addEventListener(FILES_REFRESHED_EVENT, look);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener(APP_VISIBILITY_RESTORED, look);
      window.removeEventListener(FILES_REFRESHED_EVENT, look);
    };
  }, []);

  const status = state.saving
    ? "Saving…"
    : state.dirty
      ? "Unsaved"
      : state.phase === "ready"
        ? "Saved"
        : "";

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div className="flex h-[calc(var(--workspace-topbar-height)+var(--titlebar-height))] min-w-0 shrink-0 items-center gap-2 border-border border-b bg-muted/30 px-2">
        <div className="flex min-w-0 max-w-[min(18rem,35vw)] items-center gap-1.5">
          <FileTextIcon className="size-4 shrink-0 text-muted-foreground" />
          <span
            className="min-w-0 truncate font-medium text-muted-foreground text-sm"
            title={file.relativePath}
          >
            {file.name}
          </span>
        </div>
        <div className="flex-1" />
        <span className="shrink-0 text-muted-foreground text-xs">{status}</span>
        <ModeSwitch
          mode={state.readOnly ? "viewing" : mode}
          author={author}
          disabled={state.readOnly}
          onChange={(next) => setMode(file.absolutePath, next)}
        />
      </div>
      <div id={toolbarId} className="thelab-docx-toolbar shrink-0" />
      {state.readOnly && <Banner tone="warning">{READ_ONLY_NOTE}</Banner>}
      {state.openInWord && <Banner tone="warning">{OPEN_IN_WORD_NOTE}</Banner>}
      {state.changedOnDisk && (
        <Banner
          tone="warning"
          actions={
            <>
              <Button
                variant="ghost"
                size="sm"
                className="h-6 px-2 text-xs"
                onClick={() => void controllerRef.current?.reload()}
              >
                Reload
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="h-6 px-2 text-xs"
                onClick={() => void controllerRef.current?.keepMine()}
              >
                Keep mine
              </Button>
            </>
          }
        >
          This file changed on disk while you had unsaved edits.
        </Banner>
      )}
      {state.restored && <Banner tone="warning">{KEPT_EDITS_NOTE}</Banner>}
      {state.saveError && <Banner tone="error">{state.saveError}</Banner>}
      {state.wholeFile === "held" && state.dirty && (
        <Banner tone="warning">{WHOLE_FILE_HELD_NOTE}</Banner>
      )}
      <WholeFileQuestion
        open={state.wholeFile === "ask"}
        onSave={() => void controllerRef.current?.saveWholeFile()}
        onCancel={() => controllerRef.current?.keepEditing()}
      />
      <div className="relative min-h-0 flex-1">
        <div
          ref={editorRef}
          className={cn(
            "thelab-docx-editor absolute inset-0 overflow-auto",
            state.phase !== "ready" && "invisible",
          )}
        />
        {state.phase === "loading" && (
          <div className="absolute inset-0 flex items-center justify-center gap-2 text-muted-foreground text-sm">
            <Loader2Icon className="size-4 animate-spin" />
            Opening {file.name}…
          </div>
        )}
        {state.phase === "error" && (
          <div className="absolute inset-0 flex items-center justify-center p-8 text-center text-muted-foreground text-sm">
            {state.error}
          </div>
        )}
        {state.equationNote && (
          <div
            role="status"
            className="pointer-events-none absolute bottom-4 left-1/2 z-20 flex -translate-x-1/2 items-center gap-2 rounded-lg border border-border bg-background/95 px-3 py-2 text-xs shadow-lg"
          >
            <InfoIcon className="size-3.5 shrink-0 text-muted-foreground" />
            {EQUATION_NOTE}
          </div>
        )}
      </div>
    </div>
  );
}

/** ⌘S could not keep the unedited parts: rewrite the whole file? */
function WholeFileQuestion({
  open,
  onSave,
  onCancel,
}: {
  open: boolean;
  onSave: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent
        showCloseButton={false}
        onOpenAutoFocus={(event) => {
          // Return and Space keep editing; only a click rewrites the file.
          event.preventDefault();
          cancelRef.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>Save the whole file?</DialogTitle>
          <DialogDescription>{WHOLE_FILE_QUESTION}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button ref={cancelRef} variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button onClick={onSave}>Save anyway</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ModeSwitch({
  mode,
  author,
  disabled,
  onChange,
}: {
  mode: DocxMode;
  author: string;
  disabled?: boolean;
  onChange: (mode: DocxMode) => void;
}) {
  const id = useId();
  return (
    <div
      role="radiogroup"
      aria-label="Mode"
      className="flex shrink-0 items-center rounded-md border border-border bg-background p-0.5"
    >
      {DOCX_MODES.map((m) => (
        <button
          key={m.mode}
          id={`${id}-${m.mode}`}
          type="button"
          role="radio"
          aria-checked={mode === m.mode}
          disabled={disabled}
          title={
            m.mode === "suggesting"
              ? author.trim()
                ? `${m.hint} (${author.trim()})`
                : `${m.hint}. Set your name in Settings, Word files.`
              : m.hint
          }
          onClick={() => onChange(m.mode)}
          className={cn(
            "rounded px-2 py-0.5 text-xs transition-colors",
            mode === m.mode
              ? "bg-muted font-medium text-foreground"
              : "text-muted-foreground hover:text-foreground",
            disabled && "cursor-default opacity-60",
          )}
        >
          {m.label}
        </button>
      ))}
    </div>
  );
}

function Banner({
  tone,
  actions,
  children,
}: {
  tone: "warning" | "error";
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div
      role="alert"
      className={cn(
        "flex shrink-0 items-center gap-2 border-border border-b px-3 py-1.5 text-xs",
        tone === "warning"
          ? "bg-amber-500/10 text-amber-800 dark:text-amber-200"
          : "bg-red-500/10 text-red-700 dark:text-red-300",
      )}
    >
      <AlertTriangleIcon className="size-3.5 shrink-0" />
      <span className="min-w-0 flex-1">{children}</span>
      {actions}
    </div>
  );
}

/** Word's old binary format (.doc): The Lab does not open it. */
export function LegacyDocNotice({ file }: { file: ProjectFile }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 p-8 text-center">
      <div className="max-w-md rounded-lg border border-border bg-card/50 p-6 shadow-sm">
        <p className="mb-1 font-medium text-foreground text-sm">{file.name}</p>
        <p className="text-muted-foreground text-xs">
          This is Word's old .doc format, which The Lab cannot open. Open it in
          Word and save as .docx.
        </p>
      </div>
    </div>
  );
}
