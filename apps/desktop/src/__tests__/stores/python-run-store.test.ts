import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useDocumentStore, type ProjectFile } from "@/stores/document-store";
import {
  usePythonRunStore,
  watchProjectForRuns,
} from "@/stores/python-run-store";
import { emptyOutput, MAX_OUTPUT_LINES } from "@/lib/run-output";

vi.mock("@/stores/history-store", () => ({
  useHistoryStore: {
    getState: vi.fn(() => ({
      init: vi.fn(() => Promise.resolve()),
      loadSnapshots: vi.fn(() => Promise.resolve()),
      createSnapshot: vi.fn(() => Promise.resolve()),
      reset: vi.fn(),
    })),
  },
}));

vi.mock("@/stores/claude-chat-store", () => ({
  useClaudeChatStore: {
    getState: vi.fn(() => ({ newSession: vi.fn() })),
  },
}));

// The backend's events, captured so a test can send them.
const handlers: Record<string, (event: { payload: unknown }) => void> = {};
vi.mocked(listen).mockImplementation(async (event, handler) => {
  handlers[event] = handler as (event: { payload: unknown }) => void;
  return () => {};
});

function file(relativePath: string, overrides: Partial<ProjectFile> = {}) {
  return {
    id: relativePath,
    name: relativePath.split("/").pop() ?? relativePath,
    relativePath,
    absolutePath: `/thesis/${relativePath}`,
    type: "other" as const,
    content: "print('hi')\n",
    isDirty: false,
    ...overrides,
  };
}

function openProject(root: string, files: ProjectFile[], active: string) {
  useDocumentStore.setState({
    projectRoot: root,
    files,
    activeFileId: active,
  });
}

/** What `invoke` does: python_run_start answers as the backend would. */
function backend(start?: (args: Record<string, unknown>) => unknown) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "python_run_start") {
      const a = args as Record<string, unknown>;
      if (start) return start(a);
      return {
        runId: a.runId,
        file: a.file,
        interpreter: "/Users/dev/opt/openEMS/venv/bin/python",
        folder: "sim",
        command: `~/opt/openEMS/venv/bin/python ${a.file}`,
      };
    }
    return true;
  });
}

const startCalls = () =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === "python_run_start");

function deliver(runId: string | null, lines: unknown[], dropped = 0) {
  handlers["python-run-output"]({ payload: { runId, lines, dropped } });
}

function finish(runId: string | null, end: Record<string, unknown>) {
  handlers["python-run-finished"]({ payload: { runId, end } });
}

const shown = () =>
  usePythonRunStore
    .getState()
    .output.lines.map((line) => `${line.stream}:${line.text}`);

let projectCount = 0;

describe("Run and the Output panel", () => {
  beforeAll(() => {
    watchProjectForRuns();
  });

  beforeEach(() => {
    vi.clearAllMocks();
    backend();
    // A fresh project each time; switching to it resets the panel.
    projectCount += 1;
    openProject(
      `/thesis-${projectCount}`,
      [file("sim/bowtie.py"), file("main.tex", { type: "tex" })],
      "sim/bowtie.py",
    );
    vi.mocked(invoke).mockClear();
  });

  it("saves, then runs the open file by its path in the project", async () => {
    useDocumentStore.setState({
      files: [file("sim/bowtie.py", { content: "print(2)\n", isDirty: true })],
    });

    await usePythonRunStore.getState().run();

    const [[, args]] = startCalls() as [[string, Record<string, unknown>]];
    expect(args).toMatchObject({
      projectRoot: `/thesis-${projectCount}`,
      file: "sim/bowtie.py",
    });
    expect(typeof args.runId).toBe("string");
    expect(useDocumentStore.getState().files[0].isDirty).toBe(false);
    const state = usePythonRunStore.getState();
    expect(state.status).toBe("running");
    expect(state.runId).toBe(args.runId);
    expect(state.file).toBe("sim/bowtie.py");
  });

  it("streams the run's lines and ends with its summary", async () => {
    await usePythonRunStore.getState().run();
    const { runId } = usePythonRunStore.getState();

    deliver(runId, [
      { stream: "meta", text: "$ ~/opt/openEMS/venv/bin/python sim/bowtie.py" },
      { stream: "out", text: "meshing" },
      { stream: "err", text: "warning: coarse mesh" },
    ]);
    deliver("some-other-run", [{ stream: "out", text: "not ours" }]);
    finish(runId, {
      code: 0,
      signal: null,
      stopped: false,
      seconds: 1.2,
      summary: "[exited 0 after 1.2 s]",
    });

    expect(shown()).toEqual([
      "meta:$ ~/opt/openEMS/venv/bin/python sim/bowtie.py",
      "out:meshing",
      "err:warning: coarse mesh",
      "meta:[exited 0 after 1.2 s]",
    ]);
    const state = usePythonRunStore.getState();
    expect(state.status).toBe("idle");
    expect(state.lastEnd?.code).toBe(0);
  });

  it("marks a failed run's summary as an error", async () => {
    await usePythonRunStore.getState().run();
    finish(usePythonRunStore.getState().runId, {
      code: 1,
      signal: null,
      stopped: false,
      seconds: 0.3,
      summary: "[exited 1 after 0.3 s]",
    });
    const lines = usePythonRunStore.getState().output.lines;
    const last = lines[lines.length - 1];
    expect(last?.text).toBe("[exited 1 after 0.3 s]");
    expect(last?.tone).toBe("error");
  });

  it("starts nothing while a script runs, and says so", async () => {
    await usePythonRunStore.getState().run();
    const { runId } = usePythonRunStore.getState();
    deliver(runId, [{ stream: "out", text: "step 1 of 4000" }]);

    await usePythonRunStore.getState().run();
    await usePythonRunStore.getState().run();

    // One start, and nothing that would stop or replace the running script.
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "python_run_start",
    ]);
    const state = usePythonRunStore.getState();
    expect(state.status).toBe("running");
    expect(state.runId).toBe(runId);
    // Said once, however often Run is pressed.
    expect(shown()).toEqual([
      "out:step 1 of 4000",
      "meta:sim/bowtie.py is still running, so Run started nothing. Stop it first with ⌘. or the Stop button.",
    ]);
  });

  it("refuses a file that is not Python, in plain words", async () => {
    useDocumentStore.setState({ activeFileId: "main.tex" });

    await usePythonRunStore.getState().run();

    expect(startCalls()).toHaveLength(0);
    expect(shown()).toEqual([
      "meta:Run works on Python files (.py); main.tex is not one.",
    ]);
    expect(usePythonRunStore.getState().output.lines[0].tone).toBe("error");
    expect(usePythonRunStore.getState().status).toBe("idle");
  });

  it("shows the backend's reason when the interpreter has a problem", async () => {
    backend(() => {
      throw 'bowtie.py was not run: /Users/dev/opt/openEMS/venv/bin/python does not exist (folders."sim" in thelab.toml)';
    });

    await usePythonRunStore.getState().run();

    expect(usePythonRunStore.getState().status).toBe("idle");
    expect(shown()).toEqual([
      'meta:bowtie.py was not run: /Users/dev/opt/openEMS/venv/bin/python does not exist (folders."sim" in thelab.toml)',
    ]);
  });

  it("stops with SIGINT through the backend", async () => {
    await usePythonRunStore.getState().run();

    await usePythonRunStore.getState().stop();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("python_run_stop");
    expect(usePythonRunStore.getState().status).toBe("stopping");
    finish(usePythonRunStore.getState().runId, {
      code: 130,
      signal: null,
      stopped: true,
      seconds: 4,
      summary: "[stopped after 4.0 s]",
    });
    expect(usePythonRunStore.getState().status).toBe("idle");
    const lines = usePythonRunStore.getState().output.lines;
    expect(lines[lines.length - 1]?.tone).toBe("note");
  });

  it("a Stop pressed while the script is starting stops it once started", async () => {
    let release: (value: unknown) => void = () => {};
    backend(
      (args) =>
        new Promise((resolve) => {
          release = () =>
            resolve({ runId: args.runId, file: args.file, command: "" });
        }),
    );

    const running = usePythonRunStore.getState().run();
    await vi.waitFor(() => expect(startCalls()).toHaveLength(1));
    await usePythonRunStore.getState().stop();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("python_run_stop");
    release(undefined);
    await running;

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("python_run_stop");
    expect(usePythonRunStore.getState().status).toBe("stopping");
  });

  it("closing or switching the project ends the script and clears the panel", async () => {
    await usePythonRunStore.getState().run();
    const { runId } = usePythonRunStore.getState();
    deliver(runId, [{ stream: "out", text: "meshing" }]);

    useDocumentStore.setState({ projectRoot: "/another-project" });

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("python_run_close_project");
    const state = usePythonRunStore.getState();
    expect(state.status).toBe("idle");
    expect(state.runId).toBe(null);
    expect(state.output.lines).toEqual([]);
    // The old run's last words do not reach the new project's panel.
    finish(runId, {
      code: null,
      signal: 9,
      stopped: true,
      seconds: 3,
      summary: "[stopped after 3.0 s]",
    });
    expect(usePythonRunStore.getState().output.lines).toEqual([]);

    vi.mocked(invoke).mockClear();
    useDocumentStore.setState({ projectRoot: null });
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("python_run_close_project");
  });

  it("a script that started while its project closed is ended by its id", async () => {
    let release: (value: unknown) => void = () => {};
    let startedId = "";
    backend(
      (args) =>
        new Promise((resolve) => {
          startedId = args.runId as string;
          release = () =>
            resolve({ runId: args.runId, file: args.file, command: "" });
        }),
    );

    const running = usePythonRunStore.getState().run();
    await vi.waitFor(() => expect(startCalls()).toHaveLength(1));
    useDocumentStore.setState({ projectRoot: "/next-project" });
    release(undefined);
    await running;

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("python_run_close_project", {
      runId: startedId,
    });
    expect(usePythonRunStore.getState().status).toBe("idle");
  });

  it("keeps at most 10,000 lines of a long run", async () => {
    await usePythonRunStore.getState().run();
    const { runId } = usePythonRunStore.getState();
    for (let batch = 0; batch < 12; batch++) {
      const lines = Array.from({ length: 1000 }, (_, i) => ({
        stream: "out",
        text: `step ${batch * 1000 + i}`,
      }));
      deliver(runId, lines);
    }
    const { output } = usePythonRunStore.getState();
    expect(output.lines).toHaveLength(MAX_OUTPUT_LINES);
    expect(output.dropped).toBe(2000);
    expect(output.lines[output.lines.length - 1]?.text).toBe("step 11999");
  });

  it("clears a finished run's output, but not a running one's", async () => {
    await usePythonRunStore.getState().run();
    const { runId } = usePythonRunStore.getState();
    deliver(runId, [{ stream: "out", text: "line" }]);

    usePythonRunStore.getState().clear();
    expect(shown()).toEqual(["out:line"]);

    finish(runId, {
      code: 0,
      signal: null,
      stopped: false,
      seconds: 1,
      summary: "[exited 0 after 1.0 s]",
    });
    usePythonRunStore.getState().clear();
    expect(usePythonRunStore.getState().output).toEqual(emptyOutput());
  });
});
