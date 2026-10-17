/**
 * The Lab: SuperDoc (AGPL-3.0) edits Word files. It is large, so it loads
 * with a dynamic import the first time a Word tab opens, and a session that
 * never opens a .docx never loads it. Nothing else may import "superdoc"
 * statically (`superdoc-lazy.test.ts` checks).
 */
export type SuperDocModule = typeof import("superdoc");

let loading: Promise<SuperDocModule> | null = null;

export function loadSuperDoc(): Promise<SuperDocModule> {
  if (!loading) {
    loading = Promise.all([
      import("superdoc"),
      import("superdoc/style.css"),
    ]).then(([module]) => module);
    // A failed load (say, a chunk that could not be read) can be retried.
    loading.catch(() => {
      loading = null;
    });
  }
  return loading;
}
