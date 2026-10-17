import { describe, it, expect, beforeAll, beforeEach, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { useDocumentStore, type ProjectFile } from "@/stores/document-store";
import {
  configKey,
  usePythonEnvStore,
  watchInterpreters,
} from "@/stores/python-env-store";
import type { PythonMap } from "@/lib/python-env";

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

function file(relativePath: string, overrides: Partial<ProjectFile> = {}) {
  return {
    id: relativePath,
    name: relativePath.split("/").pop() ?? relativePath,
    relativePath,
    absolutePath: `/thesis/${relativePath}`,
    type: "other" as const,
    content: "",
    isDirty: false,
    ...overrides,
  };
}

const map = (root: string): PythonMap => ({
  root,
  default: {
    path: "/opt/homebrew/bin/python3",
    source: "config",
    configured: "/opt/homebrew/bin/python3",
    version: "3.14.7",
    virtualEnv: null,
    problem: null,
  },
  folders: [],
  notes: [],
});

const fetches = () =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === "python_interpreters");

let count = 0;

describe("the interpreter map follows the project and its thelab.toml", () => {
  let root = "";

  beforeAll(() => {
    watchInterpreters();
  });

  beforeEach(async () => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockImplementation(async (command, args) =>
      command === "python_interpreters"
        ? map((args as { projectRoot: string }).projectRoot)
        : undefined,
    );
    count += 1;
    root = `/project-${count}`;
    useDocumentStore.setState({
      projectRoot: root,
      files: [
        file("sim/run.py"),
        file("tools/build.py"),
        file("thelab.toml", { content: "[python]\n" }),
      ],
      activeFileId: "sim/run.py",
    });
    await vi.waitFor(() =>
      expect(usePythonEnvStore.getState().loading).toBe(false),
    );
  });

  it("is fetched once when the project opens", () => {
    expect(fetches()).toEqual([["python_interpreters", { projectRoot: root }]]);
    expect(usePythonEnvStore.getState().map?.root).toBe(root);
    expect(usePythonEnvStore.getState().projectRoot).toBe(root);
  });

  it("is not fetched again when the open file changes", () => {
    useDocumentStore.setState({ activeFileId: "tools/build.py" });
    useDocumentStore.setState({ activeFileId: "sim/run.py" });
    expect(fetches()).toHaveLength(1);
  });

  it("is fetched again when thelab.toml is saved, not while it is edited", () => {
    const files = useDocumentStore.getState().files;
    const edit = (content: string, isDirty: boolean) =>
      useDocumentStore.setState({
        files: files.map((f) =>
          f.relativePath === "thelab.toml" ? { ...f, content, isDirty } : f,
        ),
      });

    edit('[python]\ninterpreter = "/opt', true);
    edit('[python]\ninterpreter = "/opt/homebrew/bin/python3"\n', true);
    expect(fetches()).toHaveLength(1);

    edit('[python]\ninterpreter = "/opt/homebrew/bin/python3"\n', false);
    expect(fetches()).toHaveLength(2);
  });

  it("is fetched again when thelab.toml appears or goes", () => {
    const withoutConfig = useDocumentStore
      .getState()
      .files.filter((f) => f.relativePath !== "thelab.toml");
    useDocumentStore.setState({ files: withoutConfig });
    expect(fetches()).toHaveLength(2);
    useDocumentStore.setState({
      files: [...withoutConfig, file("thelab.toml", { content: "" })],
    });
    expect(fetches()).toHaveLength(3);
  });

  it("says why when thelab.toml cannot be read", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(
      "Invalid /project/thelab.toml: unknown field `interpreters`",
    );
    await usePythonEnvStore.getState().reload();
    const state = usePythonEnvStore.getState();
    expect(state.map).toBe(null);
    expect(state.error).toContain("unknown field `interpreters`");
  });

  it("is dropped when the project closes", () => {
    useDocumentStore.setState({ projectRoot: null, files: [] });
    const state = usePythonEnvStore.getState();
    expect(state.map).toBe(null);
    expect(state.projectRoot).toBe(null);
    expect(fetches()).toHaveLength(1);
  });
});

describe("what of thelab.toml counts", () => {
  it("is its saved content, or that it is absent", () => {
    expect(configKey([], "x")).toBe("absent");
    expect(configKey([file("thelab.toml", { content: "a" })], "x")).toBe(
      "saved:a",
    );
    expect(
      configKey(
        [file("thelab.toml", { content: "b", isDirty: true })],
        "saved:a",
      ),
    ).toBe("saved:a");
  });
});
