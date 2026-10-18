/**
 * The Lab: keep a Word file's unchanged parts byte for byte when a Word tab
 * saves.
 *
 * SuperDoc (1.x, the AGPL line) writes a whole new package on every export:
 * it re-serializes every paragraph, drops spacing inside equations and
 * unwraps display equations from their paragraphs, even when nothing was
 * edited. So a save compares two of SuperDoc's own exports of the same
 * document, `baseline` (taken right after the file loaded) and `edited`
 * (taken now). What is the same in both was not touched, and the original
 * bytes of it are written; only what differs comes from `edited`. Within
 * the body, footnotes, endnotes, headers and footers this works paragraph by
 * paragraph (each top-level block of the part), mapped to the original by
 * position once their text is checked to match. Comments go one by one
 * (`preserve-comments.ts`). The package's own lists (content types and
 * relationships) stay the original's, with only what the edit added or
 * removed added or removed.
 *
 * Ids that SuperDoc numbers afresh on each export (comments, tracked
 * changes) do not count as a difference, and blocks taken from the edit get
 * the ids the saved file uses.
 *
 * When the mapping cannot be trusted, or the result would not hold
 * together (a relationship, comment or note it names is missing), the
 * result is SuperDoc's whole export, with its comments repaired, marked
 * `preserved: false`. That rewrites what was not edited, so the tab asks
 * before writing it (`docx-controller.ts`).
 */
import { strFromU8, strToU8, unzipSync, zipSync, type Zippable } from "fflate";
import {
  attributes,
  blockText,
  mergeRootTag,
  prefixesUsed,
  splitPart,
  withoutVolatileIds,
  type SplitPart,
} from "./docx-xml";
import {
  COMMENT_PART_TYPES,
  COMMENT_PARTS,
  COMMENTS,
  CommentsMismatch,
  repairCommentsPart,
  spliceComments,
} from "./preserve-comments";

export {
  blockText,
  mergeRootTag,
  prefixesUsed,
  splitPart,
  withoutVolatileIds,
  type SplitPart,
} from "./docx-xml";

export interface PreserveResult {
  bytes: Uint8Array;
  /** True when unchanged parts and blocks were taken from the original. */
  preserved: boolean;
  /** Why the save fell back to SuperDoc's whole export. */
  reason?: string;
  /** Parts not written byte for byte from the original. */
  changedParts: string[];
}

type Files = Record<string, Uint8Array>;

const CONTENT_TYPES = "[Content_Types].xml";

/**
 * The package's properties stay as they were: SuperDoc recounts pages and
 * words wrongly, drops fields such as Company, and adds its own version
 * and a random id.
 */
const KEPT_PROPERTIES = new Set([
  "docProps/app.xml",
  "docProps/core.xml",
  "docProps/custom.xml",
]);

/** Root elements whose top-level children are spliced one by one. */
const SPLICED_ROOTS: Record<string, "body" | "root"> = {
  "w:document": "body",
  "w:footnotes": "root",
  "w:endnotes": "root",
  "w:hdr": "root",
  "w:ftr": "root",
};

class Fallback extends Error {}

function unzip(bytes: Uint8Array, what: string): Files {
  try {
    const files = unzipSync(bytes);
    for (const name of Object.keys(files)) {
      if (name.endsWith("/")) delete files[name];
    }
    return files;
  } catch (err) {
    throw new Fallback(`${what} is not a readable package (${String(err)})`);
  }
}

function isXml(name: string): boolean {
  return name.endsWith(".xml") || name.endsWith(".rels");
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function samePart(name: string, a: Uint8Array, b: Uint8Array): boolean {
  if (sameBytes(a, b)) return true;
  if (!isXml(name)) return false;
  return withoutVolatileIds(strFromU8(a)) === withoutVolatileIds(strFromU8(b));
}

// ─── Ids SuperDoc numbers afresh on each export ───

const COMMENT_MARK =
  /(<w:(?:commentRangeStart|commentRangeEnd|commentReference)\b[^>]*?\sw:id=")([^"]*)(")/g;
/** Whether a part marks a comment (no `g` flag: `test` keeps no state). */
const HAS_COMMENT_MARK =
  /<w:(?:commentRangeStart|commentRangeEnd|commentReference)\b/;

/**
 * Tracked changes and the like that stand alone (unlike range starts and
 * ends, whose ids pair them up across blocks).
 */
const REVISION =
  /(<w:(?:ins|del|moveFrom|moveTo|rPrChange|pPrChange|sectPrChange|tblPrChange|tblGridChange|tblPrExChange|trPrChange|tcPrChange|numberingChange|cellIns|cellDel|cellMerge)\b[^>]*?\sw:id=")([^"]*)(")/g;

/** `xml` with its comment marks numbered as `ids` says. */
export function renumberComments(
  xml: string,
  ids: Map<string, string>,
): string {
  return xml.replace(
    COMMENT_MARK,
    (whole, before: string, id: string, after: string) =>
      ids.has(id) ? before + ids.get(id) + after : whole,
  );
}

function revisionIds(xml: string): string[] {
  return [...xml.matchAll(REVISION)].map((m) => m[2]);
}

/** What a block is compared by: no ids that SuperDoc draws or renumbers. */
function comparable(block: string, commentIds: Map<string, string>): string {
  return withoutVolatileIds(
    renumberComments(block, commentIds).replace(
      REVISION,
      (_w, before: string, _id: string, after: string) => before + after,
    ),
  );
}
// ─── Aligning the baseline export's blocks with the edited export's ───

type Op =
  | { kind: "same"; a: number; b: number }
  | { kind: "new"; b: number }
  | { kind: "gone"; a: number };

/** An edit script from `a` to `b` (common prefix and suffix, then LCS). */
export function alignBlocks(a: string[], b: string[]): Op[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) {
    start++;
  }
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const ops: Op[] = [];
  for (let i = 0; i < start; i++) ops.push({ kind: "same", a: i, b: i });
  const n = endA - start;
  const m = endB - start;
  if (n * m > 4_000_000) {
    for (let i = 0; i < n; i++) ops.push({ kind: "gone", a: start + i });
    for (let j = 0; j < m; j++) ops.push({ kind: "new", b: start + j });
  } else {
    // lcs[i][j]: the longest common run of a[start+i..] and b[start+j..].
    const lcs = new Uint32Array((n + 1) * (m + 1));
    const at = (i: number, j: number) => i * (m + 1) + j;
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lcs[at(i, j)] =
          a[start + i] === b[start + j]
            ? lcs[at(i + 1, j + 1)] + 1
            : Math.max(lcs[at(i + 1, j)], lcs[at(i, j + 1)]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && a[start + i] === b[start + j]) {
        ops.push({ kind: "same", a: start + i, b: start + j });
        i++;
        j++;
      } else if (j < m && (i === n || lcs[at(i, j + 1)] >= lcs[at(i + 1, j)])) {
        ops.push({ kind: "new", b: start + j });
        j++;
      } else {
        ops.push({ kind: "gone", a: start + i });
        i++;
      }
    }
  }
  for (let k = 0; endA + k < a.length; k++) {
    ops.push({ kind: "same", a: endA + k, b: endB + k });
  }
  return ops;
}

function spliceKind(xml: string): "body" | "root" | null {
  // The root element's name: the first tag that is not a declaration,
  // processing instruction or comment.
  const m = /<([A-Za-z_][\w.:-]*)[\s/>]/.exec(xml);
  return m ? (SPLICED_ROOTS[m[1]] ?? null) : null;
}

interface SplicePlan {
  name: string;
  o: SplitPart;
  e: SplitPart;
  ops: Op[];
}

/** Which blocks of `original` an edit kept, and which it replaced. */
function planSplice(
  name: string,
  original: string,
  baseline: string,
  edited: string,
  baselineIds: Map<string, string>,
  editedIds: Map<string, string>,
): SplicePlan {
  const kind = spliceKind(original);
  if (!kind) throw new Fallback(`${name} cannot be split`);
  const o = splitPart(original, kind);
  const b = splitPart(baseline, kind);
  const e = splitPart(edited, kind);
  if (!o || !b || !e) throw new Fallback(`${name} cannot be split`);
  if (o.blocks.length !== b.blocks.length) {
    throw new Fallback(
      `${name}: ${o.blocks.length} blocks in the file, ${b.blocks.length} as loaded`,
    );
  }
  for (let i = 0; i < o.blocks.length; i++) {
    if (blockText(o.blocks[i]) !== blockText(b.blocks[i])) {
      throw new Fallback(`${name}: block ${i + 1} does not match as loaded`);
    }
  }
  const ops = alignBlocks(
    b.blocks.map((block) => comparable(block, baselineIds)),
    e.blocks.map((block) => comparable(block, editedIds)),
  );
  return { name, o, e, ops };
}

/**
 * Numbers for the tracked changes in blocks taken from the edit, so that
 * none repeats one in a block kept from the original.
 */
class RevisionIds {
  private readonly taken: Set<string>;
  private next: number;
  private readonly given = new Map<string, string>();

  constructor(kept: string[], all: string[]) {
    this.taken = new Set(kept);
    const numbers = all.map((id) => Number.parseInt(id, 10));
    this.next = Math.max(0, ...numbers.filter(Number.isFinite)) + 1;
  }

  apply(xml: string): string {
    return xml.replace(
      REVISION,
      (whole, before: string, id: string, after: string) => {
        if (!this.taken.has(id)) return whole;
        let fresh = this.given.get(id);
        if (!fresh) {
          while (this.taken.has(String(this.next))) this.next++;
          fresh = String(this.next++);
          this.given.set(id, fresh);
        }
        return before + fresh + after;
      },
    );
  }
}

/** `original` with the blocks the edit changed taken from `edited`. */
function applySplice(
  plan: SplicePlan,
  editedIds: Map<string, string>,
  revisions: RevisionIds,
): string {
  const { o, e, ops } = plan;
  let body = "";
  let fromEdit = "";
  for (const op of ops) {
    if (op.kind === "same") {
      body += o.gaps[op.a] + o.blocks[op.a];
    } else if (op.kind === "new") {
      const block = revisions.apply(
        renumberComments(e.blocks[op.b], editedIds),
      );
      body += e.gaps[op.b] + block;
      fromEdit += block;
    }
  }
  body += o.gaps[o.blocks.length] ?? "";
  const rootTag = mergeRootTag(o.rootTag, e.rootTag, prefixesUsed(fromEdit));
  const head = o.head.replace(o.rootTag, rootTag);
  return head + body + o.tail;
}

// ─── The package's lists: content types and relationships ───

function partNameOf(element: string): string | null {
  const name = attributes(element).get("PartName");
  return name ? decodeURIComponent(name.replace(/^\//, "")) : null;
}

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

function contentTypeElements(xml: string): string[] {
  return (
    xml.match(
      /<(?:Default|Override)\b[^>]*?(?:\/>|>\s*<\/(?:Default|Override)>)/g,
    ) ?? []
  );
}

function insertBefore(xml: string, closing: string, added: string[]): string {
  if (added.length === 0) return xml;
  const at = xml.lastIndexOf(closing);
  if (at < 0) throw new Fallback(`no ${closing} to add to`);
  return xml.slice(0, at) + added.join("") + xml.slice(at);
}

function removeElements(xml: string, gone: string[]): string {
  let out = xml;
  for (const element of gone) out = out.replace(element, "");
  return out;
}

/** The original's content types, covering what was added and not removed. */
function mergeContentTypes(
  original: string,
  edited: string | null,
  out: Files,
  originalNames: Set<string>,
): string {
  const elements = contentTypeElements(original);
  const overrides = new Map<string, string>();
  const defaults = new Set<string>();
  for (const el of elements) {
    const part = el.startsWith("<Override") ? partNameOf(el) : null;
    if (part) overrides.set(part, el);
    const ext = attributes(el).get("Extension");
    if (el.startsWith("<Default") && ext) defaults.add(ext.toLowerCase());
  }
  const theirs = edited ? contentTypeElements(edited) : [];
  const added: string[] = [];
  for (const name of Object.keys(out)) {
    if (originalNames.has(name) || name === CONTENT_TYPES) continue;
    if (overrides.has(name)) continue;
    const override = theirs.find(
      (el) => el.startsWith("<Override") && partNameOf(el) === name,
    );
    if (override) {
      added.push(override);
      overrides.set(name, override);
      continue;
    }
    const known = COMMENT_PART_TYPES[name];
    if (known) {
      added.push(
        `<Override PartName="/${name}" ContentType="${known.contentType}"/>`,
      );
      continue;
    }
    const ext = extensionOf(name);
    if (defaults.has(ext)) continue;
    const def = theirs.find(
      (el) =>
        el.startsWith("<Default") &&
        attributes(el).get("Extension")?.toLowerCase() === ext,
    );
    if (!def) throw new Fallback(`no content type for ${name}`);
    added.push(def);
    defaults.add(ext);
  }
  const gone = [...overrides]
    .filter(([name]) => originalNames.has(name) && !(name in out))
    .map(([, el]) => el);
  return insertBefore(removeElements(original, gone), "</Types>", added);
}

function relationshipElements(xml: string): string[] {
  return xml.match(/<Relationship\b[^>]*?(?:\/>|>\s*<\/Relationship>)/g) ?? [];
}

/** The part whose relationships `relsPath` lists ("" for the package). */
function sourceOf(relsPath: string): string {
  return relsPath.replace(/_rels\/([^/]*)\.rels$/, "$1");
}

/**
 * The original's relationships, without those to parts the edit removed,
 * and with those the edit added (to parts in `out`, or outside the file).
 */
function mergeRelationships(
  relsPath: string,
  original: string | null,
  edited: string | null,
  out: Files,
  alreadyDangling: Set<string>,
): string | null {
  const target = (el: string) => {
    const attrs = attributes(el);
    return attrs.get("TargetMode") === "External"
      ? `external:${attrs.get("Target") ?? ""}`
      : relationshipTarget(relsPath, el);
  };
  const missing = (el: string) => {
    const t = target(el);
    return (
      t != null &&
      !t.startsWith("external:") &&
      !(t in out) &&
      !alreadyDangling.has(`${relsPath} -> ${t}`)
    );
  };
  const theirs = edited ? relationshipElements(edited) : [];
  if (original == null) {
    if (edited == null) return null;
    return removeElements(edited, theirs.filter(missing));
  }
  const mine = relationshipElements(original);
  const byId = new Map(mine.map((el) => [attributes(el).get("Id"), el]));
  const targets = new Set(
    mine.map((el) => `${attributes(el).get("Type")} ${target(el)}`),
  );
  const added: string[] = [];
  for (const el of theirs) {
    const id = attributes(el).get("Id");
    const existing = id ? byId.get(id) : undefined;
    if (existing) {
      if (target(existing) !== target(el)) {
        throw new Fallback(`${relsPath}: ${id} names another part`);
      }
      continue;
    }
    if (missing(el)) continue;
    if (targets.has(`${attributes(el).get("Type")} ${target(el)}`)) continue;
    added.push(el);
  }
  const merged = insertBefore(
    removeElements(original, mine.filter(missing)),
    "</Relationships>",
    added,
  );
  return merged;
}

/** The package's lists for `out`, from the original's. */
function mergeLists(out: Files, o: Files, e: Files): void {
  const originalNames = new Set(Object.keys(o));
  const dangling = danglingTargets(o);
  const relsNames = new Set(
    [...Object.keys(o), ...Object.keys(e)].filter((n) => n.endsWith(".rels")),
  );
  for (const name of relsNames) {
    const source = sourceOf(name);
    if (source !== "" && !(source in out)) continue;
    const merged = mergeRelationships(
      name,
      o[name] ? strFromU8(o[name]) : null,
      e[name] ? strFromU8(e[name]) : null,
      out,
      dangling,
    );
    if (merged == null) continue;
    out[name] =
      o[name] && merged === strFromU8(o[name]) ? o[name] : strToU8(merged);
  }
  if (o[CONTENT_TYPES]) {
    const original = strFromU8(o[CONTENT_TYPES]);
    const merged = mergeContentTypes(
      original,
      e[CONTENT_TYPES] ? strFromU8(e[CONTENT_TYPES]) : null,
      out,
      originalNames,
    );
    out[CONTENT_TYPES] =
      merged === original ? o[CONTENT_TYPES] : strToU8(merged);
  }
}
// ─── Checks that the result holds together ───

function relsPathFor(part: string): string {
  const slash = part.lastIndexOf("/");
  return `${part.slice(0, slash + 1)}_rels/${part.slice(slash + 1)}.rels`;
}

function ids(xml: string, element: string, attribute = "w:id"): Set<string> {
  const out = new Set<string>();
  const re = new RegExp(`<${element}\\s[^>]*?${attribute}="([^"]*)"`, "g");
  for (let m = re.exec(xml); m; m = re.exec(xml)) out.add(m[1]);
  return out;
}

/** The part a relationship in `relsPath` points to, or null if external. */
function relationshipTarget(relsPath: string, tag: string): string | null {
  const attrs = attributes(tag);
  if (attrs.get("TargetMode") === "External") return null;
  const target = attrs.get("Target");
  if (!target) return null;
  // "word/_rels/document.xml.rels" belongs to "word/document.xml".
  const sourceDir = relsPath.replace(/_rels\/[^/]*$/, "");
  const joined = target.startsWith("/") ? target.slice(1) : sourceDir + target;
  const parts: string[] = [];
  for (const piece of joined.split("/")) {
    if (piece === "..") parts.pop();
    else if (piece !== "." && piece !== "") parts.push(piece);
  }
  return decodeURIComponent(parts.join("/"));
}

/** Internal relationships whose target part is not in `files`. */
function danglingTargets(files: Files): Set<string> {
  const out = new Set<string>();
  for (const name of Object.keys(files)) {
    if (!name.endsWith(".rels")) continue;
    const xml = strFromU8(files[name]);
    for (const tag of xml.match(/<Relationship\s[^>]*>/g) ?? []) {
      let target: string | null = null;
      try {
        target = relationshipTarget(name, tag);
      } catch {
        continue;
      }
      if (target && !(target in files)) out.add(`${name} -> ${target}`);
    }
  }
  return out;
}

function checkWhole(out: Files, original: Files, changed: string[]): void {
  const text = (name: string) => (out[name] ? strFromU8(out[name]) : null);
  const alreadyDangling = danglingTargets(original);
  for (const missing of danglingTargets(out)) {
    if (!alreadyDangling.has(missing)) {
      throw new Fallback(
        `a relationship points to a missing part (${missing})`,
      );
    }
  }
  for (const name of Object.keys(out)) {
    if (!name.endsWith(".xml") || !name.startsWith("word/")) continue;
    const xml = text(name) ?? "";
    const used = new Set<string>();
    const re = /\sr:(?:id|embed|link|pict|dm|lo|qs|cs|href)="([^"]*)"/g;
    for (let m = re.exec(xml); m; m = re.exec(xml)) used.add(m[1]);
    if (used.size === 0) continue;
    const rels = text(relsPathFor(name));
    if (!rels) throw new Fallback(`${name} names relationships it has none of`);
    const known = ids(rels, "Relationship", "Id");
    for (const id of used) {
      if (!known.has(id)) throw new Fallback(`${name} names a missing ${id}`);
    }
  }
  const doc = text("word/document.xml") ?? "";
  const checks: [string, string, string][] = [
    ["w:commentReference", "word/comments.xml", "w:comment"],
    ["w:footnoteReference", "word/footnotes.xml", "w:footnote"],
    ["w:endnoteReference", "word/endnotes.xml", "w:endnote"],
  ];
  for (const [ref, part, element] of checks) {
    const used = ids(doc, ref);
    if (used.size === 0) continue;
    const defined = ids(text(part) ?? "", element);
    for (const id of used) {
      if (!defined.has(id)) throw new Fallback(`${element} ${id} is missing`);
    }
  }
  if (typeof DOMParser !== "undefined") {
    const parser = new DOMParser();
    for (const name of changed) {
      if (!isXml(name) || !out[name]) continue;
      const parsed = parser.parseFromString(
        text(name) ?? "",
        "application/xml",
      );
      if (parsed.getElementsByTagName("parsererror").length > 0) {
        throw new Fallback(`${name} is not well-formed`);
      }
    }
  }
}

function zip(out: Files, order: string[]): Uint8Array {
  const entries: Zippable = {};
  const names = [
    ...order.filter((n) => n in out),
    ...Object.keys(out).filter((n) => !order.includes(n)),
  ];
  for (const name of names) entries[name] = [out[name], { level: 6 }];
  return zipSync(entries);
}

/** Each `w15:commentEx` names a comment paragraph that exists. */
function checkCommentsExtended(out: Files): void {
  const ext = out["word/commentsExtended.xml"];
  if (!ext) return;
  const comments = out[COMMENTS] ? strFromU8(out[COMMENTS]) : "";
  const paras = new Set(
    [...comments.matchAll(/\sw14:paraId="([0-9A-Fa-f]+)"/g)].map((m) =>
      m[1].toUpperCase(),
    ),
  );
  for (const m of strFromU8(ext).matchAll(
    /\sw15:(?:paraId|paraIdParent)="([0-9A-Fa-f]+)"/g,
  )) {
    if (!paras.has(m[1].toUpperCase())) {
      throw new Fallback(
        `commentsExtended.xml names a missing paragraph ${m[1]}`,
      );
    }
  }
}

/** SuperDoc's whole export, its comments repaired (`preserve-comments.ts`). */
function wholeExport(edited: Uint8Array, files: Files | null): Uint8Array {
  if (!files?.[COMMENTS]) return edited;
  try {
    const repaired = repairCommentsPart(strFromU8(files[COMMENTS]));
    const order = Object.keys(files);
    return zip({ ...files, [COMMENTS]: strToU8(repaired) }, order);
  } catch {
    return edited;
  }
}

/**
 * The bytes to save for a Word file opened from `original`, whose state
 * SuperDoc exported as `baseline` when it loaded and as `edited` now.
 */
export function preserveUnchanged(
  original: Uint8Array,
  baseline: Uint8Array,
  edited: Uint8Array,
): PreserveResult {
  let e: Files | null = null;
  try {
    const o = unzip(original, "The file");
    const b = unzip(baseline, "The export as loaded");
    e = unzip(edited, "The export");
    const out: Files = {};
    const changed = new Set<string>();
    const comments = spliceComments(o, b, e);
    const plans: SplicePlan[] = [];

    for (const name of new Set([...Object.keys(o), ...Object.keys(e)])) {
      // The lists and the comments are settled below.
      if (name === CONTENT_TYPES || name.endsWith(".rels")) continue;
      if (COMMENT_PARTS.has(name)) continue;
      const inO = name in o;
      const inB = name in b;
      const inE = name in e;
      if (KEPT_PROPERTIES.has(name)) {
        if (inO) out[name] = o[name];
        continue;
      }
      if (inO && !inB) {
        // SuperDoc never writes this part: keep it.
        out[name] = o[name];
      } else if (inO && !inE) {
        // The edit removed it.
        changed.add(name);
      } else if (inO) {
        const same = samePart(name, b[name], e[name]);
        // A part with comment marks is spliced even when the two exports
        // agree, so that its comment ids are the saved file's.
        if (same && !HAS_COMMENT_MARK.test(strFromU8(e[name]))) {
          out[name] = o[name];
        } else if (isXml(name) && spliceKind(strFromU8(o[name]))) {
          plans.push(
            planSplice(
              name,
              strFromU8(o[name]),
              strFromU8(b[name]),
              strFromU8(e[name]),
              comments.baselineIds,
              comments.editedIds,
            ),
          );
        } else if (same) {
          out[name] = o[name];
        } else {
          out[name] = e[name];
          changed.add(name);
        }
      } else if (!inB || !samePart(name, b[name], e[name])) {
        // Added by the edit (SuperDoc's own additions on every export,
        // the same as loaded, are left out).
        out[name] = e[name];
        changed.add(name);
      }
    }

    // Blocks: tracked changes taken from the edit must not repeat an id
    // kept from the original.
    const kept: string[] = [];
    const all: string[] = [];
    for (const plan of plans) {
      for (const op of plan.ops) {
        if (op.kind === "same") kept.push(...revisionIds(plan.o.blocks[op.a]));
      }
      all.push(...revisionIds(plan.o.blocks.join("")));
      all.push(...revisionIds(plan.e.blocks.join("")));
    }
    const revisions = new RevisionIds(kept, all);
    for (const plan of plans) {
      const xml = applySplice(plan, comments.editedIds, revisions);
      const originalXml = strFromU8(o[plan.name]);
      if (xml === originalXml) {
        out[plan.name] = o[plan.name];
      } else {
        out[plan.name] = strToU8(xml);
        changed.add(plan.name);
      }
    }

    for (const name of COMMENT_PARTS) {
      if (name in comments.parts) {
        const bytes = comments.parts[name];
        if (bytes) out[name] = bytes;
        changed.add(name);
      } else if (name in o) {
        out[name] = o[name];
      }
    }

    mergeLists(out, o, e);
    for (const name of Object.keys(out)) {
      if (
        !(name in o) ||
        (out[name] !== o[name] && !sameBytes(out[name], o[name]))
      ) {
        changed.add(name);
      }
    }
    for (const name of Object.keys(o)) if (!(name in out)) changed.add(name);

    checkWhole(out, o, [...changed]);
    checkCommentsExtended(out);

    if (changed.size === 0) {
      return { bytes: original, preserved: true, changedParts: [] };
    }
    return {
      bytes: zip(out, Object.keys(o)),
      preserved: true,
      changedParts: [...changed].sort(),
    };
  } catch (err) {
    if (!(err instanceof Fallback) && !(err instanceof CommentsMismatch)) {
      throw err;
    }
    return {
      bytes: wholeExport(edited, e),
      preserved: false,
      reason: err.message,
      changedParts: ["(the whole file)"],
    };
  }
}
