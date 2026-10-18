/**
 * The Lab: reading a Word package's XML parts without re-serializing them.
 * Everything here works on the text as written, so that what a save does
 * not change keeps its exact bytes (`preserve.ts`, `preserve-comments.ts`).
 */

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
export function tagEnd(xml: string, start: number): number {
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

export function tagName(xml: string, start: number): string {
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

export function decodeEntities(text: string): string {
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

/**
 * A block's text with all whitespace (and zero-width spaces) removed, to
 * match blocks up.
 */
export function blockText(block: string): string {
  let text = "";
  const re =
    /<(w:t|m:t|w:delText|w:instrText)(?:\s[^>]*)?>([^<]*)<\/(?:w:t|m:t|w:delText|w:instrText)>/g;
  for (let m = re.exec(block); m; m = re.exec(block)) text += m[2];
  return decodeEntities(text).replace(/[\s\u200B\uFEFF]/g, "");
}

// ─── Namespaces the edited blocks need on the original's root ───

export function attributes(tag: string): Map<string, string> {
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

/** The start tag that `element` (an element as written) opens with. */
export function startTag(element: string): string {
  const end = tagEnd(element, 0);
  return end < 0 ? "" : element.slice(0, end + 1);
}

/** `tag` with `name` set to `value`: replaced in place, or added at its end. */
export function setAttribute(tag: string, name: string, value: string): string {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`(\\s${escaped}\\s*=\\s*)("[^"]*"|'[^']*')`);
  if (re.test(tag)) return tag.replace(re, `$1"${value}"`);
  const close = tag.endsWith("/>") ? tag.length - 2 : tag.length - 1;
  const before = tag.slice(0, close).replace(/\s+$/, "");
  return `${before} ${name}="${value}"${tag.slice(close)}`;
}

/** `tag` without the attributes whose names `drop` picks. */
export function removeAttributes(
  tag: string,
  drop: (name: string) => boolean,
): string {
  return tag.replace(
    /\s([^\s=/>]+)\s*=\s*("[^"]*"|'[^']*')/g,
    (whole, name: string) => (drop(name) ? "" : whole),
  );
}

/** `split` put back together with `blocks` (each after its gap). */
export function joinPart(
  split: SplitPart,
  blocks: { gap: string; xml: string }[],
  end = split.gaps[split.blocks.length] ?? "",
): string {
  let body = "";
  for (const b of blocks) body += b.gap + b.xml;
  return split.head + body + end + split.tail;
}
