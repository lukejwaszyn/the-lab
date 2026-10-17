/**
 * Which Python runs which folder of a project, as thelab-core's
 * `python_map` reports it through the `python_interpreters` command, and
 * how the Output panel names the interpreter of the open file.
 */

export type InterpreterSource = "config" | "venv" | "path";

export interface PythonInterpreter {
  path: string | null;
  source: InterpreterSource;
  /** What thelab.toml said, for a configured interpreter. */
  configured: string | null;
  version: string | null;
  /** The virtual environment it belongs to. */
  virtualEnv: string | null;
  /** Why it cannot be used; null when it can. */
  problem: string | null;
}

export interface FolderInterpreter extends PythonInterpreter {
  /** Relative to the project root, without a trailing slash. */
  folder: string;
}

export interface PythonMap {
  root: string;
  default: PythonInterpreter;
  folders: FolderInterpreter[];
  notes: string[];
}

export interface ChosenInterpreter {
  /** The `[python.folders]` entry that chose it; null for the default. */
  folder: string | null;
  interpreter: PythonInterpreter;
}

export function isPythonFile(path: string): boolean {
  return /\.py$/i.test(path);
}

/**
 * The interpreter for a file relative to the project root: the one of the
 * longest folder that holds it, else the default. The same rule as
 * thelab-core's `PythonMap::interpreter_for`, which Run uses; this one
 * serves the label, so switching tabs asks no interpreter anything.
 */
export function interpreterFor(
  map: PythonMap,
  file: string,
): ChosenInterpreter {
  const path = file.replace(/^(\.\/)+/, "");
  let best: FolderInterpreter | null = null;
  for (const entry of map.folders) {
    if (
      path.startsWith(`${entry.folder}/`) &&
      (!best || entry.folder.length > best.folder.length)
    ) {
      best = entry;
    }
  }
  return best
    ? { folder: best.folder, interpreter: best }
    : { folder: null, interpreter: map.default };
}

const GENERIC_VENV_NAMES = new Set(["venv", ".venv", "env", ".env"]);

/** A virtual environment's name. One called `venv` or `.venv` says
 *  nothing, so then the folder above names it: ~/opt/openEMS/venv is
 *  "openEMS". */
export function environmentName(virtualEnv: string): string {
  const parts = virtualEnv.replace(/\/+$/, "").split("/");
  const name = parts[parts.length - 1] ?? virtualEnv;
  const above = parts[parts.length - 2];
  return GENERIC_VENV_NAMES.has(name) && above ? above : name;
}

export interface InterpreterLabel {
  /** "openEMS venv · Python 3.14.7", or "Python 3.14.7". */
  label: string;
  /** The tooltip: its path and where it was chosen, or what is wrong. */
  detail: string;
  /** Shown in orange: the file cannot run with it. */
  problem: boolean;
}

export function describeInterpreter({
  folder,
  interpreter,
}: ChosenInterpreter): InterpreterLabel {
  if (interpreter.problem) {
    return {
      label: "Python: problem",
      detail: interpreter.problem,
      problem: true,
    };
  }
  const python = `Python ${interpreter.version ?? "?"}`;
  const label = interpreter.virtualEnv
    ? `${environmentName(interpreter.virtualEnv)} venv · ${python}`
    : python;
  const origin =
    interpreter.source === "config"
      ? folder
        ? `thelab.toml, for ${folder}/`
        : "thelab.toml"
      : interpreter.source === "venv"
        ? "the project's virtual environment"
        : "python3 on PATH";
  return {
    label,
    detail: `${interpreter.path ?? "?"}\nchosen by ${origin}`,
    problem: false,
  };
}
