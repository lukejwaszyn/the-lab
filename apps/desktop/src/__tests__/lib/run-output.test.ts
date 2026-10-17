import { describe, it, expect } from "vitest";
import {
  appendNote,
  appendOutput,
  emptyOutput,
  MAX_OUTPUT_LINES,
  type OutputBuffer,
} from "@/lib/run-output";

const texts = (buffer: OutputBuffer) =>
  buffer.lines.map((line) => `${line.stream}:${line.text}`);

describe("the Output panel's lines", () => {
  it("adds lines in order and gives each an id", () => {
    const buffer = appendOutput(emptyOutput(), [
      { stream: "meta", text: "$ python run.py" },
      { stream: "out", text: "one" },
      { stream: "err", text: "two" },
    ]);
    expect(texts(buffer)).toEqual([
      "meta:$ python run.py",
      "out:one",
      "err:two",
    ]);
    expect(buffer.lines.map((l) => l.id)).toEqual([1, 2, 3]);
  });

  it("redraws a progress bar in place, across deliveries and the other stream", () => {
    let buffer = appendOutput(emptyOutput(), [
      { stream: "err", text: " 10%", open: true },
    ]);
    const barId = buffer.lines[0].id;
    buffer = appendOutput(buffer, [
      { stream: "out", text: "epoch 1" },
      { stream: "err", text: " 60%", open: true },
    ]);
    buffer = appendOutput(buffer, [{ stream: "err", text: "100%" }]);
    buffer = appendOutput(buffer, [{ stream: "err", text: "done" }]);

    expect(texts(buffer)).toEqual(["err:100%", "out:epoch 1", "err:done"]);
    expect(buffer.lines[0].id).toBe(barId);
    expect(buffer.lines.every((line) => !line.open)).toBe(true);
  });

  it("keeps only the last lines and counts the rest", () => {
    let buffer = emptyOutput();
    for (let i = 0; i < 25; i++) {
      buffer = appendOutput(buffer, [{ stream: "out", text: `${i}` }], 0, 10);
    }
    expect(buffer.lines).toHaveLength(10);
    expect(buffer.lines[0].text).toBe("15");
    expect(buffer.dropped).toBe(15);
  });

  it("is bounded at 10,000 lines by default", () => {
    const flood = Array.from({ length: MAX_OUTPUT_LINES + 500 }, (_, i) => ({
      stream: "out" as const,
      text: `${i}`,
    }));
    const buffer = appendOutput(emptyOutput(), flood, 1000);
    expect(MAX_OUTPUT_LINES).toBe(10_000);
    expect(buffer.lines).toHaveLength(MAX_OUTPUT_LINES);
    expect(buffer.dropped).toBe(1500);
    expect(buffer.lines[buffer.lines.length - 1].text).toBe(
      `${MAX_OUTPUT_LINES + 499}`,
    );
  });

  it("does not redraw a line when lines were lost in between", () => {
    let buffer = appendOutput(emptyOutput(), [
      { stream: "err", text: " 10%", open: true },
    ]);
    buffer = appendOutput(buffer, [{ stream: "err", text: "later" }], 3);
    expect(texts(buffer)).toEqual(["err: 10%", "err:later"]);
    expect(buffer.dropped).toBe(3);
  });

  it("gives back the same buffer when nothing came", () => {
    const buffer = emptyOutput();
    expect(appendOutput(buffer, [])).toBe(buffer);
  });

  it("adds The Lab's own lines with a tone, ending open lines", () => {
    let buffer = appendOutput(emptyOutput(), [
      { stream: "out", text: "Loading", open: true },
    ]);
    buffer = appendNote(buffer, "[stopped after 4.0 s]", "note");
    buffer = appendOutput(buffer, [{ stream: "out", text: "next" }]);

    expect(texts(buffer)).toEqual([
      "out:Loading",
      "meta:[stopped after 4.0 s]",
      "out:next",
    ]);
    expect(buffer.lines[1].tone).toBe("note");
    expect(buffer.lines[0].open).toBe(false);
  });
});
