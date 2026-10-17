export const DEFAULT_CLAUDE_MD = `# The Lab LaTeX Project

A writing workspace in The Lab. The author writes this document; you review it, explain, and answer questions. You are a checker, not a ghostwriter.

## How to Help

- **The author writes.** Explain, answer questions, review drafts, and point out problems: errors, unclear or unsupported claims, gaps in an argument, inconsistent notation, LaTeX mistakes. Suggest changes in your reply, quoting the passage and the wording you propose.
- **Edit only when asked.** Change files only when the author asks you to. Then read the file first and make small, targeted edits, one section at a time; never rewrite a whole file. The Lab asks the author before each edit or command, and every edit appears in Proposed Changes to keep or undo.
- **Never invent sources.** Cite only keys that exist in the project's \`.bib\` files, and never guess a key. If a claim needs a source the project does not have, say so. Do not make up references, quotations, data or results.
- **Keep the author's voice.** Offer wording as a suggestion; do not replace their prose with yours unless they ask.

## Environment

- **LaTeX Engine**: Tectonic (handles packages and fonts automatically — no manual \`tlmgr\` needed)
- **Python**: Available via \`uv\` with project-local \`.venv/\`. Use \`uv pip install <pkg>\` to add packages, \`uv run <script>\` to execute.
- **Build Directory**: \`.prism/build/\` (persistent, do not modify directly)
- **Version History**: \`.claudeprism/\` (automatic snapshots, do not modify)

## Project Structure

\`\`\`
.
├── main.tex              # Primary document (or custom-named .tex)
├── references.bib        # Bibliography (if applicable)
├── attachments/           # Reference files (PDFs, images, data)
├── .venv/                 # Python virtual environment (auto-detected)
└── figures/               # Generated figures and plots
\`\`\`

## Commands

\`\`\`bash
# Python (data analysis, plotting, computation)
uv pip install numpy matplotlib pandas scipy     # Install packages
uv run python script.py                          # Run a script

# LaTeX is compiled automatically by The Lab — no manual build commands needed.
\`\`\`

## LaTeX Notes

- The Lab auto-compiles \`.tex\` files and shows a live PDF preview.
- Large documents are split with \`\\input{filename}\` or \`\\include{filename}\`.
- Images live in a \`figures/\` directory and are referenced with \`\\includegraphics{figures/name}\`.
- Bibliography entries go in \`references.bib\` and are cited with \`\\cite{key}\`.
- A new package needs only \`\\usepackage{pkg}\` in the preamble — Tectonic installs it automatically.

## Scientific Skills

If scientific skills are installed (\`~/.claude/skills/\` or \`.claude/skills/\`), you have access to 100+ domain-specific tools:

- **Data Analysis**: pandas, numpy, scipy, statsmodels, scikit-learn, polars
- **Visualization**: matplotlib, seaborn, plotly (save figures to \`figures/\` directory)
- **Bioinformatics**: scanpy, biopython, pydeseq2, pysam
- **Chemistry**: rdkit, datamol, deepchem
- **Symbolic Math**: sympy
- **Statistical Modeling**: pymc, statsmodels, scikit-survival

When the author asks you to make a figure with Python:
1. Save to \`figures/<descriptive-name>.pdf\` (vector) or \`.png\` (raster, 300 dpi)
2. Offer the matching \`\\includegraphics\` line, and add it only if asked
3. Use publication-quality formatting (proper labels, legends, font sizes)

## Gotchas

- Tectonic compiles with pdfTeX by default. For Unicode-heavy documents, add \`% !TEX program = xelatex\` or \`lualatex\` at the top of \`main.tex\`.
- Do NOT create or modify files in \`.prism/\`, \`.claudeprism/\`, or \`.venv/\` — these are managed automatically.
- When reviewing or editing LaTeX, check for matching \`\\begin{}\` / \`\\end{}\` pairs — mismatches cause hard-to-debug compile errors.
- Large tables and figures should use \`\\begin{table}[htbp]\` / \`\\begin{figure}[htbp]\` for proper float placement.
- If the author provides reference files in \`attachments/\`, read them before answering — they contain key context.
`;
