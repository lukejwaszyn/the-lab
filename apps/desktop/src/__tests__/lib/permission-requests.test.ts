import { describe, expect, it } from "vitest";
import {
  describeToolRequest,
  displayPath,
  parsePermissionRequest,
  UI_PAUSE_TOOLS,
} from "@/lib/permission-requests";

// The shape Claude Code 2.1.302 prints, verified live on this Mac.
const WRITE_REQUEST = {
  type: "control_request",
  request_id: "aeac4028-1",
  request: {
    subtype: "can_use_tool",
    tool_name: "Write",
    display_name: "Write",
    input: { file_path: "/projects/thesis/hello.txt", content: "hi\n" },
    description: "hello.txt",
    permission_suggestions: [
      { type: "setMode", mode: "acceptEdits", destination: "session" },
    ],
    tool_use_id: "toolu_1",
  },
};

describe("parsePermissionRequest", () => {
  it("reads a can_use_tool request", () => {
    expect(parsePermissionRequest(WRITE_REQUEST, "/projects/thesis")).toEqual({
      requestId: "aeac4028-1",
      toolName: "Write",
      displayName: "Write",
      summary: "Write hello.txt",
      detail: null,
    });
  });

  it("keeps an absolute path outside the project", () => {
    expect(parsePermissionRequest(WRITE_REQUEST, "/elsewhere")?.summary).toBe(
      "Write /projects/thesis/hello.txt",
    );
  });

  it("ignores stream messages, other subtypes and requests without ids", () => {
    expect(
      parsePermissionRequest({ type: "assistant", message: {} }),
    ).toBeNull();
    expect(
      parsePermissionRequest({
        type: "control_request",
        request_id: "h1",
        request: { subtype: "hook_callback" },
      }),
    ).toBeNull();
    expect(
      parsePermissionRequest({
        type: "control_request",
        request: { subtype: "can_use_tool", tool_name: "Write" },
      }),
    ).toBeNull();
    expect(parsePermissionRequest(null)).toBeNull();
    expect(parsePermissionRequest("control_request")).toBeNull();
  });

  it("copes with a request that has no input", () => {
    const request = parsePermissionRequest({
      type: "control_request",
      request_id: "r",
      request: { subtype: "can_use_tool", tool_name: "Bash" },
    });
    expect(request).toMatchObject({
      toolName: "Bash",
      summary: "Run a command",
      detail: null,
    });
  });
});

describe("describeToolRequest", () => {
  it("names the file for edits", () => {
    expect(
      describeToolRequest(
        "Edit",
        {
          file_path: "/p/chapters/intro.tex",
          old_string: "a",
          new_string: "b",
        },
        undefined,
        "/p/",
      ),
    ).toEqual({ summary: "Edit chapters/intro.tex", detail: null });
  });

  it("shows the command for Bash, with Claude's description as the summary", () => {
    expect(
      describeToolRequest("Bash", {
        command: "latexmk -pdf main.tex",
        description: "Compile the thesis",
      }),
    ).toEqual({
      summary: "Compile the thesis",
      detail: "latexmk -pdf main.tex",
    });
  });

  it("shows the URL and the search query", () => {
    expect(
      describeToolRequest("WebFetch", { url: "https://arxiv.org/abs/1" })
        .summary,
    ).toBe("Fetch https://arxiv.org/abs/1");
    expect(
      describeToolRequest("WebSearch", { query: "transformer survey" }).summary,
    ).toBe('Search the web for "transformer survey"');
  });

  it("falls back to Claude's description, then the input, for other tools", () => {
    expect(
      describeToolRequest("mcp__thelab__compile", {}, "Compile the project"),
    ).toEqual({
      summary: "Use mcp__thelab__compile",
      detail: "Compile the project",
    });
    expect(
      describeToolRequest("mcp__thelab__render_page", { page: 3 }),
    ).toEqual({
      summary: "Use mcp__thelab__render_page",
      detail: '{"page":3}',
    });
  });

  it("truncates long commands", () => {
    const detail = describeToolRequest("Bash", {
      command: "x".repeat(1000),
    }).detail;
    expect(detail?.length).toBe(400);
    expect(detail?.endsWith("…")).toBe(true);
  });
});

describe("displayPath", () => {
  it("makes project paths relative", () => {
    expect(displayPath("/p/main.tex", "/p")).toBe("main.tex");
    expect(displayPath("/p", "/p")).toBe(".");
    expect(displayPath("/pq/main.tex", "/p")).toBe("/pq/main.tex");
    expect(displayPath("C:\\p\\main.tex", "C:\\p")).toBe("main.tex");
    expect(displayPath("/p/main.tex", null)).toBe("/p/main.tex");
  });
});

describe("UI_PAUSE_TOOLS", () => {
  it("leaves the question and plan widgets to their own flow", () => {
    expect(UI_PAUSE_TOOLS.has("AskUserQuestion")).toBe(true);
    expect(UI_PAUSE_TOOLS.has("ExitPlanMode")).toBe(true);
    expect(UI_PAUSE_TOOLS.has("Write")).toBe(false);
  });
});
