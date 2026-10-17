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
    expect(DEFAULT_CLAUDE_MD).toContain("uv pip install");
    expect(DEFAULT_CLAUDE_MD).toContain("Tectonic");
  });
});
