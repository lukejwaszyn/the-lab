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
 * position once their text is checked to match.
 *
 * When the mapping cannot be trusted, or the result would not hold
 * together (a relationship, comment or note it names is missing), the save
 * falls back to SuperDoc's whole export, which is what SuperDoc alone would
 * have written.
 */
import { strFromU8, strToU8, unzipSync, zipSync, type Zippable } from "fflate";

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
const COMMENT_PARTS = new Set([
  "word/comments.xml",
  "word/commentsExtended.xml",
  "word/commentsIds.xml",
  "word/commentsExtensible.xml",
  "word/people.xml",
]);

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

/**
 * The part with what differs between two exports of the same document
 * removed: the ids SuperDoc draws at random on each export (paragraph ids,
 * GUIDs).
 */
export function withoutVolatileIds(xml: string): string {
  return xml
    .replace(
      /\s(?:w14|w15|w16cid|w16du):(?:paraId|textId|paraIdParent|durableId)="[^"]*"/g,
      "",
    )
    .replace(
      /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/g,
      "",
    );
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

// ─── Splitting a part into its top-level blocks, keeping the bytes ───

export interface SplitPart {
  /** Everything up to and including the container's start tag. */
  head: string;
  /** The root element's start tag (for its namespace declarations). */
  rootTag: string;
  /** The container's element children, exactly as written. */
  blocks: string[];
  /** What came before each block (whitespace, comments). */
  gaps: string[];
  /** The container's end tag and everything after it. */
  tail: string;
}

/** The index of the `>` that ends the tag opening at `start`. */
function tagEnd(xml: string, start: number): number {
  let quote = "";
  for (let i = start + 1; i < xml.length; i++) {
    const c = xml[i];
    if (quote) {
      if (c === quote) quote = "";
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === ">") {
      return i;
    }
  }
  return -1;
}

function tagName(xml: string, start: number): string {
  const m = /^<\/?([^\s/>]+)/.exec(xml.slice(start, start + 200));
  return m ? m[1] : "";
}

/**
 * Splits `xml` into the top-level children of its container: `w:body` for
 * a document, the root element for notes, headers and footers. Null when
 * it is not well-formed enough to split.
 */
export function splitPart(
  xml: string,
  container: "body" | "root",
): SplitPart | null {
  let depth = 0;
  const open: string[] = [];
  let containerDepth = -1;
  let head = "";
  let rootTag = "";
  const blocks: string[] = [];
  const gaps: string[] = [];
  let lastEnd = -1;
  let blockStart = -1;
  let i = 0;
  while (i < xml.length) {
    const lt = xml.indexOf("<", i);
    if (lt < 0) break;
    if (xml.startsWith("<!--", lt)) {
      const e = xml.indexOf("-->", lt + 4);
      if (e < 0) return null;
      i = e + 3;
      continue;
    }
    if (xml.startsWith("<![CDATA[", lt)) {
      const e = xml.indexOf("]]>", lt);
      if (e < 0) return null;
      i = e + 3;
      continue;
    }
    if (xml.startsWith("<?", lt)) {
      const e = xml.indexOf("?>", lt);
      if (e < 0) return null;
      i = e + 2;
      continue;
    }
    if (xml.startsWith("<!", lt)) {
      const e = xml.indexOf(">", lt);
      if (e < 0) return null;
      i = e + 1;
      continue;
    }
    const end = tagEnd(xml, lt);
    if (end < 0) return null;
    const closing = xml[lt + 1] === "/";
    const selfClosing = !closing && xml[end - 1] === "/";
    if (closing) {
      depth--;
      if (depth < 0 || open.pop() !== tagName(xml, lt)) return null;
      if (containerDepth >= 0 && depth === containerDepth && blockStart >= 0) {
        blocks.push(xml.slice(blockStart, end + 1));
        lastEnd = end + 1;
        blockStart = -1;
      } else if (containerDepth >= 0 && depth === containerDepth - 1) {
        if (blockStart >= 0) return null;
        gaps.push(xml.slice(lastEnd, lt));
        return { head, rootTag, blocks, gaps, tail: xml.slice(lt) };
      }
    } else {
      if (depth === 0 && !rootTag) rootTag = xml.slice(lt, end + 1);
      if (containerDepth < 0) {
        const isContainer =
          container === "root"
            ? depth === 0
            : depth === 1 && tagName(xml, lt) === "w:body";
        if (isContainer) {
          if (selfClosing) {
            return {
              head: xml.slice(0, lt),
              rootTag,
              blocks: [],
              gaps: [],
              tail: xml.slice(lt),
            };
          }
          containerDepth = depth + 1;
          head = xml.slice(0, end + 1);
          lastEnd = end + 1;
        }
      } else if (depth === containerDepth) {
        gaps.push(xml.slice(lastEnd, lt));
        if (selfClosing) {
          blocks.push(xml.slice(lt, end + 1));
          lastEnd = end + 1;
        } else {
          blockStart = lt;
        }
      }
      if (!selfClosing) {
        depth++;
        open.push(tagName(xml, lt));
      }
    }
    i = end + 1;
  }
  return null;
}

const ENTITIES: Record<string, string> = {
  lt: "<",
  gt: ">",
  amp: "&",
  quot: '"',
  apos: "'",
};

function decodeEntities(text: string): string {
  return text.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-z]+);/g, (whole, body) => {
    if (body[0] === "#") {
      const code =
        body[1] === "x"
          ? Number.parseInt(body.slice(2), 16)
          : Number.parseInt(body.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : whole;
    }
    return ENTITIES[body] ?? whole;
  });
}

/** A block's text with all whitespace removed, to match blocks up. */
export function blockText(block: string): string {
  let text = "";
  const re =
    /<(w:t|m:t|w:delText|w:instrText)(?:\s[^>]*)?>([^<]*)<\/(?:w:t|m:t|w:delText|w:instrText)>/g;
  for (let m = re.exec(block); m; m = re.exec(block)) text += m[2];
  return decodeEntities(text).replace(/[\s​﻿]/g, "");
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

// ─── Namespaces the edited blocks need on the original's root ───

function attributes(tag: string): Map<string, string> {
  const out = new Map<string, string>();
  const re = /\s([^\s=/>]+)\s*=\s*("[^"]*"|'[^']*')/g;
  for (let m = re.exec(tag); m; m = re.exec(tag)) {
    out.set(m[1], m[2].slice(1, -1));
  }
  return out;
}

/** The namespace prefixes that elements and attributes in `xml` use. */
export function prefixesUsed(xml: string): Set<string> {
  const out = new Set<string>();
  const re = /(?:<\/?|\s)([A-Za-z_][\w.-]*):[A-Za-z_][\w.-]*(?=[\s=>/])/g;
  for (let m = re.exec(xml); m; m = re.exec(xml)) {
    if (m[1] !== "xmlns" && m[1] !== "xml") out.add(m[1]);
  }
  return out;
}

/**
 * The original root start tag, with the namespace declarations (and
 * mc:Ignorable entries) from the edited export's root for the prefixes in
 * `used` that it lacks, so the edited blocks it now holds stay
 * well-formed. Nothing else about it changes.
 */
export function mergeRootTag(
  original: string,
  edited: string,
  used: Set<string>,
): string {
  const have = attributes(original);
  const want = attributes(edited);
  const add: string[] = [];
  const declared = (prefix: string) => have.has(`xmlns:${prefix}`);
  const needed = [...used].filter(
    (p) => !declared(p) && want.has(`xmlns:${p}`),
  );
  const ignorableWanted = (want.get("mc:Ignorable") ?? "")
    .split(/\s+/)
    .filter((p) => p && needed.includes(p));
  if (ignorableWanted.length > 0 && !declared("mc") && !needed.includes("mc")) {
    needed.push("mc");
  }
  for (const prefix of needed) {
    add.push(`xmlns:${prefix}="${want.get(`xmlns:${prefix}`)}"`);
  }
  let tag = original;
  if (ignorableWanted.length > 0) {
    const current = have.get("mc:Ignorable");
    const tokens = new Set((current ?? "").split(/\s+/).filter(Boolean));
    const before = tokens.size;
    for (const t of ignorableWanted) tokens.add(t);
    if (current === undefined) {
      add.push(`mc:Ignorable="${[...tokens].join(" ")}"`);
    } else if (tokens.size !== before) {
      tag = tag.replace(
        /(\smc:Ignorable\s*=\s*)("[^"]*"|'[^']*')/,
        `$1"${[...tokens].join(" ")}"`,
      );
    }
  }
  if (add.length === 0) return tag;
  const close = tag.endsWith("/>") ? tag.length - 2 : tag.length - 1;
  return `${tag.slice(0, close)} ${add.join(" ")}${tag.slice(close)}`;
}

function spliceKind(xml: string): "body" | "root" | null {
  // The root element's name: the first tag that is not a declaration,
  // processing instruction or comment.
  const m = /<([A-Za-z_][\w.:-]*)[\s/>]/.exec(xml);
  return m ? (SPLICED_ROOTS[m[1]] ?? null) : null;
}

/** `original` with the blocks the edit changed taken from `edited`. */
function splicePart(
  name: string,
  original: string,
  baseline: string,
  edited: string,
): string {
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
    b.blocks.map(withoutVolatileIds),
    e.blocks.map(withoutVolatileIds),
  );
  let body = "";
  let fromEdit = "";
  for (const op of ops) {
    if (op.kind === "same") {
      body += o.gaps[op.a] + o.blocks[op.a];
    } else if (op.kind === "new") {
      body += e.gaps[op.b] + e.blocks[op.b];
      fromEdit += e.blocks[op.b];
    }
  }
  body += o.gaps[o.blocks.length] ?? "";
  const rootTag = mergeRootTag(o.rootTag, e.rootTag, prefixesUsed(fromEdit));
  const head = o.head.replace(o.rootTag, rootTag);
  return head + body + o.tail;
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

/**
 * The bytes to save for a Word file opened from `original`, whose state
 * SuperDoc exported as `baseline` when it loaded and as `edited` now.
 */
export function preserveUnchanged(
  original: Uint8Array,
  baseline: Uint8Array,
  edited: Uint8Array,
): PreserveResult {
  try {
    const o = unzip(original, "The file");
    const b = unzip(baseline, "The export as loaded");
    const e = unzip(edited, "The export");
    const out: Files = {};
    const changed: string[] = [];
    const editedOnly: string[] = [];

    const commentsTouched = [...COMMENT_PARTS].some(
      (n) => n in b !== n in e || (n in e && !samePart(n, b[n], e[n])),
    );
    let packageFromEdited = false;

    for (const name of new Set([...Object.keys(o), ...Object.keys(e)])) {
      const inO = name in o;
      const inB = name in b;
      const inE = name in e;
      if (KEPT_PROPERTIES.has(name)) {
        if (inO) out[name] = o[name];
        else if (inE) editedOnly.push(name);
        continue;
      }
      if (
        inE &&
        inB &&
        samePart(name, b[name], e[name]) &&
        !(commentsTouched && COMMENT_PARTS.has(name))
      ) {
        // Not touched by the edit.
        if (inO) out[name] = o[name];
        else editedOnly.push(name);
      } else if (inE) {
        if (
          inO &&
          !COMMENT_PARTS.has(name) &&
          isXml(name) &&
          spliceKind(strFromU8(o[name])) &&
          inB
        ) {
          out[name] = strToU8(
            splicePart(
              name,
              strFromU8(o[name]),
              strFromU8(b[name]),
              strFromU8(e[name]),
            ),
          );
        } else {
          out[name] = e[name];
          if (!inO) packageFromEdited = true;
          if (name === CONTENT_TYPES || name.endsWith(".rels")) {
            packageFromEdited = true;
          }
        }
        changed.push(name);
      } else if (inO && !inB) {
        // SuperDoc never writes this part: keep it.
        out[name] = o[name];
      }
      // In the original and the baseline but not the edit: the edit removed it.
    }

    if (packageFromEdited) {
      // A part was added, or the package's own lists changed: its lists
      // (content types, relationships) and every part they name come from
      // the edit.
      for (const name of Object.keys(e)) {
        if (
          name === CONTENT_TYPES ||
          name.endsWith(".rels") ||
          editedOnly.includes(name)
        ) {
          if (!out[name] || !sameBytes(out[name], e[name])) {
            out[name] = e[name];
            if (!changed.includes(name)) changed.push(name);
          }
        }
      }
    }

    checkWhole(out, o, changed);

    const unchanged =
      changed.length === 0 &&
      Object.keys(out).length === Object.keys(o).length &&
      Object.keys(o).every((n) => out[n] === o[n]);
    if (unchanged) {
      return { bytes: original, preserved: true, changedParts: [] };
    }
    return {
      bytes: zip(out, Object.keys(o)),
      preserved: true,
      changedParts: changed.sort(),
    };
  } catch (err) {
    if (!(err instanceof Fallback)) throw err;
    return {
      bytes: edited,
      preserved: false,
      reason: err.message,
      changedParts: ["(the whole file)"],
    };
  }
}
