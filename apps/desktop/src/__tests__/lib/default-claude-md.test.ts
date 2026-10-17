import { describe, expect, it } from "vitest";
import { DEFAULT_CLAUDE_MD } from "@/lib/default-claude-md";

describe("DEFAULT_CLAUDE_MD", () => {
  it("sets the checker stance", () => {
    expect(DEFAULT_CLAUDE_MD).toContain("The author writes.");
    expect(DEFAULT_CLAUDE_MD).toContain("Edit only when asked.");
    expect(DEFAULT_CLAUDE_MD).toContain("Never invent sources.");
    expect(DEFAULT_CLAUDE_MD).toContain("not a ghostwriter");
  });

  it("keeps the build and structure notes", () => {
    expect(DEFAULT_CLAUDE_MD).toContain("## Project Structure");
    expect(DEFAULT_CLAUDE_MD).toContain(".prism/build/");
    expect(DEFAULT_CLAUDE_MD).toContain("Tectonic");
  });

  it("says thelab.toml sets the interpreters and no .venv is made unasked", () => {
    expect(DEFAULT_CLAUDE_MD).toContain("[python.folders]");
    expect(DEFAULT_CLAUDE_MD).toContain("thelab python");
    expect(DEFAULT_CLAUDE_MD).toContain(
      "A `.venv` is made only when the author asks for one in the Python Environment dialog.",
    );
    expect(DEFAULT_CLAUDE_MD).not.toContain("managed automatically");
    expect(DEFAULT_CLAUDE_MD).not.toContain("uv run");
    expect(DEFAULT_CLAUDE_MD).not.toContain("uv pip install");
    expect(DEFAULT_CLAUDE_MD).not.toContain("auto-detected");
  });
});
