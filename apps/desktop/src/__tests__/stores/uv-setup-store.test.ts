import { describe, it, expect, beforeEach, vi } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { invoke } from "@tauri-apps/api/core";
import { useUvSetupStore } from "@/stores/uv-setup-store";

const SRC = resolve(__dirname, "../..");

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      return name === "__tests__" ? [] : sourceFiles(path);
    }
    return /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}

describe("opening a project creates no Python environment", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUvSetupStore.setState({
      venvReady: false,
      venvPath: null,
      pythonPath: null,
    });
  });

  it("looks for the project's .venv without asking uv to make one", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(null);

    await useUvSetupStore.getState().refreshVenv("/thesis");

    expect(vi.mocked(invoke)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("project_venv_status", {
      projectPath: "/thesis",
    });
    expect(useUvSetupStore.getState().venvReady).toBe(false);
  });

  it("reports a .venv that is already there", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      venv_path: "/thesis/.venv",
      python_path: "/thesis/.venv/bin/python",
      created: false,
    });

    await useUvSetupStore.getState().refreshVenv("/thesis");

    const state = useUvSetupStore.getState();
    expect(state.venvReady).toBe(true);
    expect(state.venvPath).toBe("/thesis/.venv");
    expect(state.pythonPath).toBe("/thesis/.venv/bin/python");
  });

  it("keeps the one-click setup: Setup .venv still creates it", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      venv_path: "/project/.venv",
      python_path: "/project/.venv/bin/python",
      created: true,
    });

    await useUvSetupStore.getState().setupVenv("/project");

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("setup_project_venv", {
      projectPath: "/project",
    });
    expect(useUvSetupStore.getState().venvReady).toBe(true);
  });

  it("App.tsx looks on project open, and only the uv dialog creates a venv", () => {
    const app = readFileSync(join(SRC, "App.tsx"), "utf-8");
    expect(app).toContain("refreshVenv(projectRoot)");

    // An upstream merge that brings back the auto setup fails here.
    const callers = sourceFiles(SRC)
      .filter((file) =>
        /setupVenv\(|setup_project_venv/.test(readFileSync(file, "utf-8")),
      )
      .map((file) => relative(SRC, file))
      .sort();
    expect(callers).toEqual([
      "components/uv-setup.tsx",
      "stores/uv-setup-store.ts",
    ]);
  });
});
