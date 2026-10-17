import { describe, expect, it } from "vitest";
import { COMPILED_PDF_DIR, PROJECT_PDF_CANDIDATES } from "@/lib/compiled-pdf";

// src-tauri's test_the_frontend_looks_for_the_pdf_where_compiles_leave_it
// checks compiled-pdf-dir.json against where compile_project writes.
describe("where the project picker looks for a compiled PDF", () => {
  it("looks where the app's compiles leave it first", () => {
    expect(PROJECT_PDF_CANDIDATES[0]).toEqual([
      ...COMPILED_PDF_DIR,
      "main.pdf",
    ]);
    expect(PROJECT_PDF_CANDIDATES[1]).toEqual([
      ...COMPILED_PDF_DIR,
      "document.pdf",
    ]);
  });

  it("is a folder inside the project's .prism folder", () => {
    expect(COMPILED_PDF_DIR[0]).toBe(".prism");
    expect(COMPILED_PDF_DIR.length).toBeGreaterThan(1);
  });
});
