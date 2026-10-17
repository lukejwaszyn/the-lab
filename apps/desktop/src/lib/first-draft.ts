/**
 * The Lab: the AI first draft of a new project is opt-in. With "Draft a first
 * version with Claude" off (the default) a project is created from its
 * template only, and no prompt is sent.
 */

export interface FirstDraftTemplate {
  name: string;
  documentClass: string;
  mainFileName: string;
}

export interface FirstDraftOptions {
  /** The "Draft a first version with Claude" checkbox. */
  draftWithClaude: boolean;
  /** What the user described in "What are you writing?". */
  purpose: string;
  template: FirstDraftTemplate;
  /** From buildReferenceFilesSection; empty when there are no attachments. */
  attachmentSection: string;
}

/** True when creating the project will ask Claude for a first draft. */
export function wantsFirstDraft(
  draftWithClaude: boolean,
  purpose: string,
): boolean {
  return draftWithClaude && purpose.trim().length > 0;
}

/** The first-draft prompt, or null when the user did not ask for one. */
export function firstDraftPrompt({
  draftWithClaude,
  purpose,
  template,
  attachmentSection,
}: FirstDraftOptions): string | null {
  if (!wantsFirstDraft(draftWithClaude, purpose)) return null;

  return [
    `## New ${template.name} Project`,
    "",
    `**Template:** \`${template.documentClass}\`  `,
    `**File:** \`${template.mainFileName}\``,
    "",
    `> The file currently contains only the LaTeX preamble (packages, styling, custom commands) with an empty document body.`,
    "",
    `### What I want to create`,
    "",
    purpose.trim(),
    attachmentSection,
    `### Instructions`,
    "",
    `I asked for a first draft. Please write the document body (between \`\\begin{document}\` and \`\\end{document}\`) based on my description, keeping the existing preamble: a title, author placeholder, and a well-structured outline of sections for a **${template.name.toLowerCase()}**, with short draft text I will rewrite in my own words. Do not invent citations, data or results; mark anything that needs a source with a TODO.`,
  ].join("\n");
}
