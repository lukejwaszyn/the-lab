import {
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  PlayIcon,
  SquareIcon,
  Trash2Icon,
} from "lucide-react";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  describeInterpreter,
  type InterpreterLabel,
  interpreterFor,
  isPythonFile,
} from "@/lib/python-env";
import { MAX_OUTPUT_LINES, type OutputLine } from "@/lib/run-output";
import { cn } from "@/lib/utils";
import { useDocumentStore } from "@/stores/document-store";
import { usePythonEnvStore } from "@/stores/python-env-store";
import { type RunEnd, usePythonRunStore } from "@/stores/python-run-store";

/** The interpreter label of a file, from the map fetched when the project
 *  opened or its thelab.toml changed; null for a file that is not Python. */
function useInterpreterLabel(file: string | null): InterpreterLabel | null {
  const map = usePythonEnvStore((s) => s.map);
  const error = usePythonEnvStore((s) => s.error);
  const loading = usePythonEnvStore((s) => s.loading);
  return useMemo(() => {
    if (!file || !isPythonFile(file)) return null;
    if (error)
      return { label: "Python: problem", detail: error, problem: true };
    if (map) return describeInterpreter(interpreterFor(map, file));
    if (loading) {
      return {
        label: "Python …",
        detail: "Asking the project's interpreters for their versions",
        problem: false,
      };
    }
    return null;
  }, [file, map, error, loading]);
}

function useActivePath(): string | null {
  return useDocumentStore(
    (s) => s.files.find((f) => f.id === s.activeFileId)?.relativePath ?? null,
  );
}

/** Seconds since `since`, ticking while `running`. */
function useElapsed(since: number | null, running: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);
  return since ? Math.max(0, now - since) / 1000 : 0;
}

function elapsedText(seconds: number): string {
  const whole = Math.floor(seconds);
  if (whole < 60) return `${whole} s`;
  const minutes = Math.floor(whole / 60);
  if (minutes < 60) return `${minutes} min ${whole % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

function endText(end: RunEnd): string {
  if (end.stopped) return "stopped";
  if (end.code === 0) return "done";
  if (end.code !== null) return `exit ${end.code}`;
  return "ended";
}

function lineClass(line: OutputLine): string {
  if (line.stream === "meta") {
    if (line.tone === "error") return "text-red-600 dark:text-red-400";
    if (line.tone === "note") return "text-orange-600 dark:text-orange-400";
    return "text-muted-foreground";
  }
  if (line.stream === "err") return "text-red-700/90 dark:text-red-300/90";
  return "text-foreground";
}

const Line = memo(function Line({ line }: { line: OutputLine }) {
  return (
    <div className={cn("whitespace-pre-wrap break-words", lineClass(line))}>
      {line.text || " "}
    </div>
  );
});

function OutputLines({
  lines,
  dropped,
}: {
  lines: OutputLine[];
  dropped: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  // Follow new output while scrolled to the bottom; stay put otherwise.
  const following = useRef(true);
  useLayoutEffect(() => {
    const element = ref.current;
    if (element && following.current && lines.length > 0) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lines]);

  return (
    <div
      ref={ref}
      onScroll={(e) => {
        const el = e.currentTarget;
        following.current =
          el.scrollHeight - el.scrollTop - el.clientHeight < 16;
      }}
      className="max-h-56 select-text overflow-y-auto border-border border-t px-3 py-1 font-mono text-[11px] leading-[1.45]"
    >
      {dropped > 0 && (
        <div className="text-muted-foreground italic">
          … {dropped.toLocaleString()} earlier lines not kept (the panel keeps
          the last {MAX_OUTPUT_LINES.toLocaleString()})
        </div>
      )}
      {lines.map((line) => (
        <Line key={line.id} line={line} />
      ))}
    </div>
  );
}

/**
 * The Output panel, under Problems: what Run's script writes, stdout and
 * stderr interleaved, ending with "[exited N after T s]" or "[stopped
 * after T s]". Its header names the interpreter of the open .py file.
 * Shown while a .py file is open, or while there is output to read.
 */
export function OutputPanel() {
  const activePath = useActivePath();
  const status = usePythonRunStore((s) => s.status);
  const file = usePythonRunStore((s) => s.file);
  const startedAt = usePythonRunStore((s) => s.startedAt);
  const lastEnd = usePythonRunStore((s) => s.lastEnd);
  const output = usePythonRunStore((s) => s.output);
  const collapsed = usePythonRunStore((s) => s.collapsed);
  const interpreter = useInterpreterLabel(activePath);

  const busy = status !== "idle";
  const elapsed = useElapsed(startedAt, status === "running");
  const hasLines = output.lines.length > 0;
  if (!interpreter && !busy && !hasLines) return null;

  const state =
    status === "starting"
      ? "starting …"
      : status === "running"
        ? `running · ${elapsedText(elapsed)}`
        : status === "stopping"
          ? "stopping …"
          : lastEnd
            ? endText(lastEnd)
            : null;
  const run = usePythonRunStore.getState();

  return (
    <div className="shrink-0 border-border border-t bg-background">
      <div className="flex items-center gap-2 pr-2">
        <button
          type="button"
          onClick={() => run.setCollapsed(!collapsed)}
          className="flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-xs transition-colors hover:bg-muted/50"
        >
          {collapsed ? (
            <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronDownIcon className="size-3.5 shrink-0 text-muted-foreground" />
          )}
          <span className="shrink-0 font-medium text-foreground">Output</span>
          {file && (
            <span className="min-w-0 truncate text-muted-foreground">
              {file}
            </span>
          )}
          {state && (
            <span
              className={cn(
                "flex shrink-0 items-center gap-1.5",
                busy
                  ? "text-foreground"
                  : lastEnd && !lastEnd.stopped && lastEnd.code !== 0
                    ? "text-red-600 dark:text-red-400"
                    : "text-muted-foreground",
              )}
            >
              {status === "running" && (
                <span className="inline-block size-1.5 animate-pulse rounded-full bg-green-500" />
              )}
              {state}
            </span>
          )}
        </button>
        {interpreter && (
          <Tooltip>
            <TooltipTrigger asChild>
              <span
                className={cn(
                  "max-w-[40%] shrink-0 cursor-default truncate text-xs",
                  interpreter.problem
                    ? "text-orange-600 dark:text-orange-400"
                    : "text-muted-foreground",
                )}
              >
                {interpreter.label}
              </span>
            </TooltipTrigger>
            <TooltipContent
              side="top"
              className="max-w-md whitespace-pre-line break-all"
            >
              {interpreter.detail}
            </TooltipContent>
          </Tooltip>
        )}
        {busy ? (
          <TooltipIconButton
            tooltip="Stop (⌘.)"
            side="top"
            onClick={() => void run.stop()}
            disabled={status === "stopping"}
          >
            <SquareIcon className="size-3 fill-current text-red-500" />
          </TooltipIconButton>
        ) : (
          hasLines && (
            <TooltipIconButton
              tooltip="Clear output"
              side="top"
              onClick={() => run.clear()}
            >
              <Trash2Icon className="size-3.5 text-muted-foreground" />
            </TooltipIconButton>
          )
        )}
      </div>
      {!collapsed && hasLines && (
        <OutputLines lines={output.lines} dropped={output.dropped} />
      )}
    </div>
  );
}

/** Run and Stop for the editor toolbar, shown when the open file is .py. */
export function RunControls() {
  const status = usePythonRunStore((s) => s.status);
  const busy = status !== "idle";
  const run = usePythonRunStore.getState();
  return (
    <>
      <TooltipIconButton
        tooltip={
          busy ? "A script is running: Run starts nothing (⌘R)" : "Run (⌘R)"
        }
        onClick={() => void run.run()}
        className={cn(busy && "opacity-50")}
      >
        <PlayIcon className="size-4 text-green-600 dark:text-green-500" />
      </TooltipIconButton>
      {busy && (
        <TooltipIconButton
          tooltip="Stop (⌘.)"
          onClick={() => void run.stop()}
          disabled={status === "stopping"}
        >
          <SquareIcon className="size-3.5 fill-current text-red-500" />
        </TooltipIconButton>
      )}
    </>
  );
}
