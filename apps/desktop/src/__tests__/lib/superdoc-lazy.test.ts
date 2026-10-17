import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";

// SuperDoc is large: it must load only when a Word tab opens.
const loaded = vi.hoisted(() => ({ superdoc: 0, css: 0 }));
vi.mock("superdoc", () => {
  loaded.superdoc++;
  return { SuperDoc: class {}, Extensions: {} };
});
vi.mock("superdoc/style.css", () => {
  loaded.css++;
  return {};
});

const SRC = resolve(__dirname, "../..");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      return name === "__tests__" ? [] : sources(path);
    }
    return /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}

describe("loading SuperDoc", () => {
  it("is not loaded by opening the editor or the Word tab's modules", async () => {
    await import("@/components/workspace/editor/docx-editor");
    await import("@/lib/docx/docx-controller");
    await import("@/stores/document-store");
    expect(loaded.superdoc).toBe(0);
    expect(loaded.css).toBe(0);
  });

  it("loads, styles included, the first time a Word tab asks", async () => {
    const { loadSuperDoc } = await import("@/lib/docx/superdoc-loader");
    const first = loadSuperDoc();
    const again = loadSuperDoc();
    expect(again).toBe(first);
    await first;
    expect(loaded.superdoc).toBe(1);
    expect(loaded.css).toBe(1);
  });

  it("is imported, other than for types, only by the loader", () => {
    const importers = sources(SRC).filter((path) => {
      const text = readFileSync(path, "utf-8");
      // A value import or a side-effect import; `import type` is erased.
      return /import\s+(?!type\b)[^;]*?from\s+["']superdoc(\/[^"']*)?["']|import\s+["']superdoc(\/[^"']*)?["']|import\(\s*["']superdoc/.test(
        text,
      );
    });
    expect(importers.map((p) => relative(SRC, p))).toEqual([
      "lib/docx/superdoc-loader.ts",
    ]);
  });
});
