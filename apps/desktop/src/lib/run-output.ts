/**
 * The Output panel's lines: a script's stdout and stderr as thelab-core's
 * Run engine delivers them, with The Lab's own lines (the command, the
 * end, a refusal). Bounded, so a chatty simulation cannot fill memory.
 */

export type OutputStream = "out" | "err" | "meta";

/** How a line of The Lab's own reads: an error, or a note. */
export type OutputTone = "error" | "note";

export interface OutputLine {
  /** Stable while the line is redrawn, so the panel can key rows by it. */
  id: number;
  stream: OutputStream;
  text: string;
  /** Still being written: its stream's next line replaces it. */
  open: boolean;
  tone?: OutputTone;
}

/** A line as the `python-run-output` event carries it. */
export interface IncomingLine {
  stream: OutputStream;
  text: string;
  open?: boolean;
}

/** The panel keeps the last this many lines. */
export const MAX_OUTPUT_LINES = 10_000;

export interface OutputBuffer {
  lines: OutputLine[];
  /** Lines no longer kept, here or before they were delivered. */
  dropped: number;
  /** The id of stdout's and of stderr's open line. */
  open: { out: number | null; err: number | null };
  nextId: number;
}

export function emptyOutput(): OutputBuffer {
  return { lines: [], dropped: 0, open: { out: null, err: null }, nextId: 1 };
}

function indexOfId(lines: OutputLine[], id: number): number {
  // Open lines sit near the end.
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].id === id) return i;
    if (lines[i].id < id) return -1;
  }
  return -1;
}

/**
 * Adds delivered lines. A stream's open line is replaced in place by that
 * stream's next line, even when the other stream wrote in between, which
 * is how a progress bar redraws. `droppedBefore` lines were lost before
 * these; the open lines are left as they were, since a lost line may have
 * ended them.
 */
export function appendOutput(
  buffer: OutputBuffer,
  incoming: IncomingLine[],
  droppedBefore = 0,
  max = MAX_OUTPUT_LINES,
): OutputBuffer {
  if (incoming.length === 0 && droppedBefore === 0) return buffer;
  const lines = buffer.lines.slice();
  const open =
    droppedBefore > 0 ? { out: null, err: null } : { ...buffer.open };
  let nextId = buffer.nextId;
  let dropped = buffer.dropped + droppedBefore;

  for (const line of incoming) {
    const isOpen = !!line.open;
    if (line.stream !== "meta") {
      const openId = open[line.stream];
      const at = openId === null ? -1 : indexOfId(lines, openId);
      if (at >= 0) {
        lines[at] = { ...lines[at], text: line.text, open: isOpen };
        if (!isOpen) open[line.stream] = null;
        continue;
      }
    }
    const id = nextId++;
    lines.push({ id, stream: line.stream, text: line.text, open: isOpen });
    if (line.stream !== "meta") open[line.stream] = isOpen ? id : null;
  }

  if (lines.length > max) {
    const extra = lines.length - max;
    lines.splice(0, extra);
    dropped += extra;
  }
  return { lines, dropped, open, nextId };
}

/** Adds a line of The Lab's own, and ends any open lines. */
export function appendNote(
  buffer: OutputBuffer,
  text: string,
  tone?: OutputTone,
  max = MAX_OUTPUT_LINES,
): OutputBuffer {
  const closed = buffer.lines.map((line) =>
    line.open ? { ...line, open: false } : line,
  );
  const next = appendOutput(
    { ...buffer, lines: closed, open: { out: null, err: null } },
    [{ stream: "meta", text }],
    0,
    max,
  );
  const last = next.lines[next.lines.length - 1];
  if (tone && last) next.lines[next.lines.length - 1] = { ...last, tone };
  return next;
}
