import { describe, expect, it } from "vitest";
import { firstDraftPrompt, wantsFirstDraft } from "@/lib/first-draft";

const template = {
  name: "Research Paper",
  documentClass: "article",
  mainFileName: "main.tex",
};

describe("the AI first draft is opt-in", () => {
  it("sends nothing when the checkbox is off, whatever was typed", () => {
    expect(wantsFirstDraft(false, "A paper on protein folding")).toBe(false);
    expect(
      firstDraftPrompt({
        draftWithClaude: false,
        purpose: "A paper on protein folding",
        template,
        attachmentSection: "",
      }),
    ).toBeNull();
  });

  it("sends nothing when the checkbox is on but the description is empty", () => {
    expect(wantsFirstDraft(true, "   ")).toBe(false);
    expect(
      firstDraftPrompt({
        draftWithClaude: true,
        purpose: "  ",
        template,
        attachmentSection: "",
      }),
    ).toBeNull();
  });

  it("asks for a draft when the checkbox is on and there is a description", () => {
    const prompt = firstDraftPrompt({
      draftWithClaude: true,
      purpose: "  A paper on protein folding  ",
      template,
      attachmentSection: "\n### Reference files\n\n- attachments/notes.pdf\n",
    });

    expect(prompt).toContain("## New Research Paper Project");
    expect(prompt).toContain("**Template:** `article`");
    expect(prompt).toContain(
      "### What I want to create\n\nA paper on protein folding\n",
    );
    expect(prompt).toContain("attachments/notes.pdf");
    expect(prompt).toContain("I asked for a first draft");
    expect(prompt).toContain("Do not invent citations");
  });
});
