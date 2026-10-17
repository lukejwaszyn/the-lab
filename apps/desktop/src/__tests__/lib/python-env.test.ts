import { describe, it, expect } from "vitest";
import {
  describeInterpreter,
  environmentName,
  interpreterFor,
  isPythonFile,
  type PythonInterpreter,
  type PythonMap,
} from "@/lib/python-env";

function interpreter(
  overrides: Partial<PythonInterpreter> = {},
): PythonInterpreter {
  return {
    path: "/opt/homebrew/bin/python3",
    source: "config",
    configured: "/opt/homebrew/bin/python3",
    version: "3.14.7",
    virtualEnv: null,
    problem: null,
    ...overrides,
  };
}

/** Luke's thesis, as the drafted thelab.toml maps it. */
const thesis: PythonMap = {
  root: "/Users/dev/radar-thesis",
  default: interpreter(),
  folders: [
    {
      folder: "sim",
      ...interpreter({
        path: "/Users/dev/opt/openEMS/venv/bin/python",
        configured: "~/opt/openEMS/venv/bin/python",
        virtualEnv: "/Users/dev/opt/openEMS/venv",
      }),
    },
    {
      folder: "src/lunar_gpr/ml",
      ...interpreter({
        path: "/Users/dev/opt/gpr-ml/bin/python",
        configured: "~/opt/gpr-ml/bin/python",
        virtualEnv: "/Users/dev/opt/gpr-ml",
      }),
    },
  ],
  notes: [],
};

const label = (file: string) =>
  describeInterpreter(interpreterFor(thesis, file)).label;

describe("the interpreter of the open file", () => {
  it("is the one of the longest folder that holds it, else the default", () => {
    expect(interpreterFor(thesis, "sim/bowtie_pair.py").folder).toBe("sim");
    expect(interpreterFor(thesis, "src/lunar_gpr/ml/train.py").folder).toBe(
      "src/lunar_gpr/ml",
    );
    expect(interpreterFor(thesis, "./sim/run.py").folder).toBe("sim");
    expect(interpreterFor(thesis, "src/lunar_gpr/processing.py").folder).toBe(
      null,
    );
    expect(interpreterFor(thesis, "tools/build_gantt.py").folder).toBe(null);
    expect(interpreterFor(thesis, "notebooks/common.py").folder).toBe(null);
    // A folder's name is not a prefix of another folder's.
    expect(interpreterFor(thesis, "simulation/run.py").folder).toBe(null);
    expect(interpreterFor(thesis, "src/lunar_gpr/mlx/run.py").folder).toBe(
      null,
    );
  });

  it("is labelled by its virtual environment and version", () => {
    expect(label("sim/bowtie_pair.py")).toBe("openEMS venv · Python 3.14.7");
    expect(label("src/lunar_gpr/ml/train.py")).toBe(
      "gpr-ml venv · Python 3.14.7",
    );
    expect(label("tests/test_balun.py")).toBe("Python 3.14.7");
  });

  it("puts its path and where it was chosen in the tooltip", () => {
    const sim = describeInterpreter(interpreterFor(thesis, "sim/run.py"));
    expect(sim.problem).toBe(false);
    expect(sim.detail).toBe(
      "/Users/dev/opt/openEMS/venv/bin/python\nchosen by thelab.toml, for sim/",
    );
    const tools = describeInterpreter(interpreterFor(thesis, "tools/x.py"));
    expect(tools.detail).toBe(
      "/opt/homebrew/bin/python3\nchosen by thelab.toml",
    );
    const found = describeInterpreter({
      folder: null,
      interpreter: interpreter({ source: "path", configured: null }),
    });
    expect(found.detail).toBe(
      "/opt/homebrew/bin/python3\nchosen by python3 on PATH",
    );
  });

  it("shows a problem in orange with the reason in the tooltip", () => {
    const broken = describeInterpreter({
      folder: "sim",
      interpreter: interpreter({
        path: null,
        version: null,
        problem:
          '/Users/dev/opt/openEMS/venv/bin/python does not exist (folders."sim" in thelab.toml)',
      }),
    });
    expect(broken.problem).toBe(true);
    expect(broken.label).toBe("Python: problem");
    expect(broken.detail).toContain("does not exist");
  });

  it("names a venv called venv or .venv by the folder above it", () => {
    expect(environmentName("/Users/me/opt/openEMS/venv")).toBe("openEMS");
    expect(environmentName("/Users/me/thesis/.venv/")).toBe("thesis");
    expect(environmentName("/Users/me/opt/gpr-ml")).toBe("gpr-ml");
  });

  it("knows a Python file by its extension", () => {
    expect(isPythonFile("sim/run.py")).toBe(true);
    expect(isPythonFile("sim/RUN.PY")).toBe(true);
    expect(isPythonFile("main.tex")).toBe(false);
    expect(isPythonFile("notebooks/01.ipynb")).toBe(false);
  });
});
