import compiledPdfDir from "./compiled-pdf-dir.json";

/** Where the app's compiles leave a project's PDF, relative to the project:
 *  thelab_core writes into build/ inside the .prism/build copy. The folder
 *  is in compiled-pdf-dir.json, which a src-tauri test checks against the
 *  place compile_project writes to and project_modified_ms reads. */
export const COMPILED_PDF_DIR: readonly string[] = compiledPdfDir;

/** Where a project's compiled PDF may be, relative to the project, the
 *  current place first. */
export const PROJECT_PDF_CANDIDATES: readonly string[][] = [
  [...COMPILED_PDF_DIR, "main.pdf"],
  [...COMPILED_PDF_DIR, "document.pdf"],
  // Where ClaudePrism's embedded Tectonic left it.
  [".prism", "build", "main.pdf"],
  [".prism", "build", "document.pdf"],
  ["main.pdf"],
  ["document.pdf"],
];
