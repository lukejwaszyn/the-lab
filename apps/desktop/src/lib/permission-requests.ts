/**
 * The Lab: Claude's permission requests.
 *
 * Claude runs with `--permission-prompt-tool stdio`, so when a tool needs the
 * user's permission its stdout carries a `control_request` line with subtype
 * `can_use_tool`. The backend keeps the request's input and writes the answer;
 * the chat only needs to show what Claude wants and send back allow or deny.
 */

export interface PermissionRequest {
  requestId: string;
  /** The tool's name, used for "Allow for this chat". */
  toolName: string;
  /** The name to show, e.g. "Write". */
  displayName: string;
  /** What Claude wants to do, e.g. "Edit chapters/intro.tex". */
  summary: string;
  /** A second line when it helps: the command, the URL, Claude's description. */
  detail: string | null;
}

/**
 * Tools with their own chat widget. The run is paused for them anyway, so
 * they get no permission card.
 */
export const UI_PAUSE_TOOLS: ReadonlySet<string> = new Set([
  "AskUserQuestion",
  "ExitPlanMode",
]);

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function asText(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** Show a path relative to the project when it is inside it. */
export function displayPath(path: string, projectRoot?: string | null): string {
  if (projectRoot) {
    const root = projectRoot.replace(/[\\/]+$/, "");
    if (path === root) return ".";
    if (path.startsWith(`${root}/`) || path.startsWith(`${root}\\`)) {
      return path.slice(root.length + 1);
    }
  }
  return path;
}

/** A short, readable account of what a tool call wants to do. */
export function describeToolRequest(
  toolName: string,
  input: Record<string, unknown>,
  description?: unknown,
  projectRoot?: string | null,
): { summary: string; detail: string | null } {
  const rawPath =
    asText(input.file_path) ??
    asText(input.notebook_path) ??
    asText(input.path);
  const path = rawPath ? displayPath(rawPath, projectRoot) : null;
  const claudeDescription = asText(description);

  switch (toolName) {
    case "Write":
      return {
        summary: path ? `Write ${path}` : "Write a file",
        detail: null,
      };
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return {
        summary: path ? `Edit ${path}` : "Edit a file",
        detail: null,
      };
    case "Read":
      return {
        summary: path ? `Read ${path}` : "Read a file",
        detail: null,
      };
    case "Bash":
    case "PowerShell": {
      const command = asText(input.command);
      return {
        summary: asText(input.description) ?? "Run a command",
        detail: command ? truncate(command, 400) : null,
      };
    }
    case "WebFetch": {
      const url = asText(input.url);
      return {
        summary: url ? `Fetch ${truncate(url, 120)}` : "Fetch a web page",
        detail: null,
      };
    }
    case "WebSearch": {
      const query = asText(input.query);
      return {
        summary: query
          ? `Search the web for "${truncate(query, 120)}"`
          : "Search the web",
        detail: null,
      };
    }
    case "Glob":
    case "Grep": {
      const pattern = asText(input.pattern);
      return {
        summary: pattern
          ? `Search files for ${truncate(pattern, 120)}`
          : "Search files",
        detail: path,
      };
    }
    default: {
      let detail = claudeDescription;
      if (!detail && Object.keys(input).length > 0) {
        try {
          detail = truncate(JSON.stringify(input), 300);
        } catch {
          detail = null;
        }
      }
      return { summary: `Use ${toolName}`, detail };
    }
  }
}

/**
 * Read a `can_use_tool` control request from one parsed stdout message.
 * Anything else returns null.
 */
export function parsePermissionRequest(
  msg: unknown,
  projectRoot?: string | null,
): PermissionRequest | null {
  if (!msg || typeof msg !== "object") return null;
  const message = msg as Record<string, any>;
  if (message.type !== "control_request") return null;
  if (typeof message.request_id !== "string" || !message.request_id) {
    return null;
  }
  const request = message.request;
  if (!request || request.subtype !== "can_use_tool") return null;

  const toolName = asText(request.tool_name) ?? "a tool";
  const displayName = asText(request.display_name) ?? toolName;
  const input =
    request.input && typeof request.input === "object" ? request.input : {};
  const { summary, detail } = describeToolRequest(
    toolName,
    input,
    request.description,
    projectRoot,
  );
  return {
    requestId: message.request_id,
    toolName,
    displayName,
    summary,
    detail,
  };
}
