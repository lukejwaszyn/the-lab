/**
 * The Lab: a Word file's comments on save, one comment at a time.
 *
 * SuperDoc (1.x) writes every comment anew on each export, and not as
 * Word does: each comment paragraph gets a second `w:annotationRef` run
 * and its `w:pPr` after a run (out of the schema's order), every comment
 * gets SuperDoc's own `custom:*` attributes, and the ids are renumbered.
 * So when the comments changed (one added, resolved, answered or deleted),
 * this writes the original's comments byte for byte, except those the
 * edit touched, which come from SuperDoc's export with those faults
 * repaired. Resolving and threading live in `commentsExtended.xml`, which
 * is changed entry by entry the same way; `commentsIds.xml` and
 * `commentsExtensible.xml` keep their entries and gain or lose only those
 * of comments added or deleted.
 *
 * Comments are matched up by position between the file and SuperDoc's
 * export as loaded (`baseline`), and by SuperDoc's internal id between
 * that and the export now (`edited`).
 */
import { strFromU8, strToU8 } from "fflate";
import {
  attributes,
  blockText,
  joinPart,
  mergeRootTag,
  prefixesUsed,
  removeAttributes,
  setAttribute,
  splitPart,
  startTag,
  tagName,
  withoutVolatileIds,
  type SplitPart,
} from "./docx-xml";

type Files = Record<string, Uint8Array>;

export const COMMENTS = "word/comments.xml";
export const COMMENTS_EXTENDED = "word/commentsExtended.xml";
export const COMMENTS_IDS = "word/commentsIds.xml";
export const COMMENTS_EXTENSIBLE = "word/commentsExtensible.xml";
export const PEOPLE = "word/people.xml";

export const COMMENT_PARTS = new Set([
  COMMENTS,
  COMMENTS_EXTENDED,
  COMMENTS_IDS,
  COMMENTS_EXTENSIBLE,
  PEOPLE,
]);

/** For a comment part the export does not list itself. */
export const COMMENT_PART_TYPES: Record<
  string,
  { contentType: string; relType: string }
> = {
  [COMMENTS]: {
    contentType:
      "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml",
    relType:
      "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments",
  },
  [COMMENTS_EXTENDED]: {
    contentType:
      "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml",
    relType:
      "http://schemas.microsoft.com/office/2011/relationships/commentsExtended",
  },
};

const NS: Record<string, string> = {
  w14: "http://schemas.microsoft.com/office/word/2010/wordml",
  w15: "http://schemas.microsoft.com/office/word/2012/wordml",
  mc: "http://schemas.openxmlformats.org/markup-compatibility/2006",
};

/** The comments cannot be matched up; the caller falls back. */
export class CommentsMismatch extends Error {}

interface Comment {
  /** Its position among the part's blocks. */
  index: number;
  id: string;
  /** SuperDoc's own id for it (`custom:internalId`), in its exports. */
  internalId: string | null;
  author: string;
  xml: string;
  /** The `w14:paraId` of its last paragraph, which other parts refer to. */
  lastParaId: string | null;
  /** What two exports of the same comment have alike. */
  key: string;
  text: string;
}

interface CommentsPart {
  split: SplitPart;
  comments: Comment[];
  /** Comment by block index. */
  atBlock: Map<number, Comment>;
}

function text(files: Files, name: string): string | null {
  return files[name] ? strFromU8(files[name]) : null;
}

/** `xml` with an empty root (`<x/>`) opened, so that children can go in. */
function openEmptyRoot(xml: string): string {
  const split = splitPart(xml, "root");
  if (!split || split.blocks.length > 0) return xml;
  const tag = split.rootTag;
  if (!tag.endsWith("/>")) return xml;
  const name = tagName(tag, 0);
  const at = xml.indexOf(tag);
  return `${xml.slice(0, at)}${tag.slice(0, -2).replace(/\s+$/, "")}></${name}>${xml.slice(at + tag.length)}`;
}

function split(xml: string, what: string): SplitPart {
  const s = splitPart(openEmptyRoot(xml), "root");
  if (!s) throw new CommentsMismatch(`${what} cannot be split`);
  return s;
}

function children(element: string): SplitPart | null {
  return splitPart(element, "root");
}

function lastParagraph(element: string): number {
  const c = children(element);
  if (!c) return -1;
  for (let i = c.blocks.length - 1; i >= 0; i--) {
    if (tagName(c.blocks[i], 0) === "w:p") return i;
  }
  return -1;
}

function lastParaIdOf(element: string): string | null {
  const c = children(element);
  const at = lastParagraph(element);
  if (!c || at < 0) return null;
  return attributes(startTag(c.blocks[at])).get("w14:paraId") ?? null;
}

/** `element` (a comment) with its last paragraph's `w14:paraId` set. */
export function withLastParaId(element: string, paraId: string): string {
  const c = children(element);
  const at = lastParagraph(element);
  if (!c || at < 0) return element;
  const p = c.blocks[at];
  const tag = startTag(p);
  const blocks = c.blocks.map((xml, i) => ({
    gap: c.gaps[i],
    xml:
      i === at
        ? setAttribute(tag, "w14:paraId", paraId) + p.slice(tag.length)
        : xml,
  }));
  return joinPart(c, blocks);
}

function parseComments(xml: string | null, what: string): CommentsPart | null {
  if (xml == null) return null;
  const s = split(xml, what);
  const comments: Comment[] = [];
  const atBlock = new Map<number, Comment>();
  s.blocks.forEach((block, index) => {
    if (tagName(block, 0) !== "w:comment") return;
    const tag = startTag(block);
    const attrs = attributes(tag);
    const inner = block.slice(tag.length);
    const comment: Comment = {
      index,
      id: attrs.get("w:id") ?? "",
      internalId: attrs.get("custom:internalId") ?? null,
      author: attrs.get("w:author") ?? "",
      xml: block,
      lastParaId: lastParaIdOf(block),
      key: [
        attrs.get("w:author") ?? "",
        attrs.get("w:date") ?? "",
        attrs.get("w:initials") ?? "",
        withoutVolatileIds(inner),
      ].join("\u0000"),
      text: blockText(block),
    };
    comments.push(comment);
    atBlock.set(index, comment);
  });
  return { split: s, comments, atBlock };
}

// ─── Repairing SuperDoc's comments ───

function isAnnotationRefRun(run: string): boolean {
  const c = children(run);
  if (!c || c.blocks.length === 0) return false;
  let refs = 0;
  for (const b of c.blocks) {
    const name = tagName(b, 0);
    if (name === "w:annotationRef") refs++;
    else if (name !== "w:rPr") return false;
  }
  return refs > 0;
}

function repairParagraph(p: string): string {
  const c = children(p);
  if (!c) return p;
  let blocks = c.blocks.map((xml, i) => ({ gap: c.gaps[i], xml }));
  const pPrAt = blocks.findIndex((b) => tagName(b.xml, 0) === "w:pPr");
  if (pPrAt > 0) {
    const [pPr] = blocks.splice(pPrAt, 1);
    blocks.unshift(pPr);
  }
  let seenRef = false;
  blocks = blocks.filter((b) => {
    if (tagName(b.xml, 0) !== "w:r" || !isAnnotationRefRun(b.xml)) return true;
    if (seenRef) return false;
    seenRef = true;
    return true;
  });
  return joinPart(c, blocks);
}

/**
 * A comment as SuperDoc wrote it, made what Word writes: `w:pPr` first in
 * each paragraph, one `w:annotationRef` run, and none of SuperDoc's own
 * attributes.
 */
export function repairComment(element: string): string {
  const c = children(element);
  if (!c) return element;
  const tag = startTag(element);
  const clean = removeAttributes(
    tag,
    (name) => name.startsWith("custom:") || name === "xmlns:custom",
  );
  const blocks = c.blocks.map((xml, i) => ({
    gap: c.gaps[i],
    xml: tagName(xml, 0) === "w:p" ? repairParagraph(xml) : xml,
  }));
  return joinPart({ ...c, head: c.head.replace(tag, clean) }, blocks);
}

/** A whole comments part from SuperDoc, every comment repaired. */
export function repairCommentsPart(xml: string): string {
  const s = splitPart(xml, "root");
  if (!s) return xml;
  return joinPart(
    s,
    s.blocks.map((b, i) => ({
      gap: s.gaps[i],
      xml: tagName(b, 0) === "w:comment" ? repairComment(b) : b,
    })),
  );
}

// ─── commentsExtended.xml: resolved, and replies ───

interface ExtEntry {
  xml: string;
  paraId: string;
  done: boolean;
  parentParaId: string | null;
}

function parseExt(xml: string | null): {
  split: SplitPart | null;
  entries: (ExtEntry | null)[];
  byPara: Map<string, ExtEntry>;
} {
  const byPara = new Map<string, ExtEntry>();
  if (xml == null) return { split: null, entries: [], byPara };
  const s = split(xml, "commentsExtended.xml");
  const entries = s.blocks.map((block) => {
    if (tagName(block, 0) !== "w15:commentEx") return null;
    const attrs = attributes(startTag(block));
    const entry: ExtEntry = {
      xml: block,
      paraId: attrs.get("w15:paraId") ?? "",
      done: attrs.get("w15:done") === "1",
      parentParaId: attrs.get("w15:paraIdParent") ?? null,
    };
    if (entry.paraId) byPara.set(entry.paraId, entry);
    return entry;
  });
  return { split: s, entries, byPara };
}

/** A comment's state in one export: resolved, and the comment it answers. */
function stateOf(
  comment: Comment,
  part: CommentsPart,
  ext: ReturnType<typeof parseExt>,
): { done: boolean; parent: Comment | null } {
  const entry = comment.lastParaId ? ext.byPara.get(comment.lastParaId) : null;
  if (!entry) return { done: false, parent: null };
  const parent = entry.parentParaId
    ? (part.comments.find((c) => c.lastParaId === entry.parentParaId) ?? null)
    : null;
  return { done: entry.done, parent };
}

/**
 * The file's comment for each comment of the export as loaded: the one at
 * the same position, or else the only one with the same author and text.
 */
function matchOriginals(
  oList: Comment[],
  bList: Comment[],
): Map<Comment, Comment> {
  const same = (o: Comment, b: Comment) =>
    o.text === b.text && (!o.author || o.author === b.author);
  const out = new Map<Comment, Comment>();
  if (bList.every((b, i) => same(oList[i], b))) {
    bList.forEach((b, i) => out.set(b, oList[i]));
    return out;
  }
  const claimed = new Set<Comment>();
  for (const b of bList) {
    const candidates = oList.filter((o) => same(o, b));
    if (candidates.length !== 1 || claimed.has(candidates[0])) {
      throw new CommentsMismatch("the comments do not match as loaded");
    }
    claimed.add(candidates[0]);
    out.set(b, candidates[0]);
  }
  return out;
}

/** `element` (a comment) numbered `id`. */
function withId(element: string, id: string): string {
  const tag = startTag(element);
  return setAttribute(tag, "w:id", id) + element.slice(tag.length);
}

// ─── The splice ───

export interface CommentsSplice {
  /** Comment ids in the export as loaded, as the saved file numbers them. */
  baselineIds: Map<string, string>;
  /** Comment ids in the export now, as the saved file numbers them. */
  editedIds: Map<string, string>;
  /**
   * The comment parts that differ from the file's: new bytes, or null for
   * a part to leave out. Empty when the comments were not changed.
   */
  parts: Record<string, Uint8Array | null>;
}

interface OutComment {
  outId: string;
  xml: string;
  lastParaId: string | null;
  done: boolean;
  parent: OutComment | null;
  isNew: boolean;
  original: Comment | null;
  edited: Comment;
}

/** Every `w14:paraId` in the package's Word parts. */
function paraIdsIn(...packages: Files[]): Set<string> {
  const out = new Set<string>();
  for (const files of packages) {
    for (const name of Object.keys(files)) {
      if (!name.startsWith("word/") || !name.endsWith(".xml")) continue;
      const xml = strFromU8(files[name]);
      for (const m of xml.matchAll(/\sw14:paraId="([0-9A-Fa-f]+)"/g)) {
        out.add(m[1].toUpperCase());
      }
    }
  }
  return out;
}

/** A new paragraph id: eight hex digits below 0x80000000, as Word's are. */
function newParaId(taken: Set<string>): string {
  for (;;) {
    const id = Math.floor(Math.random() * 0x7fffffff + 1)
      .toString(16)
      .toUpperCase()
      .padStart(8, "0");
    if (!taken.has(id)) {
      taken.add(id);
      return id;
    }
  }
}

function declare(rootTag: string, prefixes: Iterable<string>): string {
  let tag = rootTag;
  const have = attributes(tag);
  for (const p of prefixes) {
    if (!have.has(`xmlns:${p}`) && NS[p]) {
      tag = setAttribute(tag, `xmlns:${p}`, NS[p]);
    }
  }
  return tag;
}

function rebuild(
  s: SplitPart,
  blocks: { gap: string; xml: string }[],
  rootTag: string,
): string {
  return joinPart({ ...s, head: s.head.replace(s.rootTag, rootTag) }, blocks);
}

function extEntryXml(
  paraId: string,
  done: boolean,
  parentParaId: string | null,
): string {
  const parent = parentParaId ? ` w15:paraIdParent="${parentParaId}"` : "";
  return `<w15:commentEx w15:paraId="${paraId}"${parent} w15:done="${done ? "1" : "0"}"/>`;
}

const EMPTY_EXT = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w15:commentsEx xmlns:mc="${NS.mc}" xmlns:w15="${NS.w15}" mc:Ignorable="w15"></w15:commentsEx>`;

/**
 * The comment parts to save for a file opened from `o`, which SuperDoc
 * exported as `b` when it loaded and as `e` now.
 */
export function spliceComments(o: Files, b: Files, e: Files): CommentsSplice {
  const oc = parseComments(text(o, COMMENTS), "The file's comments");
  const bc = parseComments(text(b, COMMENTS), "The comments as loaded");
  const ec = parseComments(text(e, COMMENTS), "The comments");
  const oList = oc?.comments ?? [];
  const bList = bc?.comments ?? [];
  const eList = ec?.comments ?? [];

  // The file's comments and the export as loaded, by position.
  if (oList.length !== bList.length) {
    throw new CommentsMismatch(
      `${oList.length} comments in the file, ${bList.length} as loaded`,
    );
  }
  const originalOf = matchOriginals(oList, bList);
  const byInternal = new Map<string, Comment>();
  for (const bComment of bList) {
    if (!bComment.internalId || byInternal.has(bComment.internalId)) {
      throw new CommentsMismatch("the comments as loaded have no ids");
    }
    byInternal.set(bComment.internalId, bComment);
  }

  // The export now and the export as loaded, by SuperDoc's id.
  const baselineOf = new Map<Comment, Comment>();
  for (const c of eList) {
    if (!c.internalId) throw new CommentsMismatch("a comment has no id");
    const match = byInternal.get(c.internalId);
    if (match) baselineOf.set(c, match);
  }

  const oExt = parseExt(text(o, COMMENTS_EXTENDED));
  const bExt = parseExt(text(b, COMMENTS_EXTENDED));
  const eExt = parseExt(text(e, COMMENTS_EXTENDED));

  const baselineIds = new Map<string, string>();
  for (const [bComment, oComment] of originalOf) {
    baselineIds.set(bComment.id, oComment.id);
  }
  const used = new Set(oList.map((c) => c.id));
  let next =
    Math.max(
      -1,
      ...oList.map((c) => Number.parseInt(c.id, 10)).filter(Number.isFinite),
    ) + 1;
  const freshId = () => {
    while (used.has(String(next))) next++;
    used.add(String(next));
    return String(next++);
  };

  // What changed.
  const editedIds = new Map<string, string>();
  const out: OutComment[] = [];
  const outOfEdited = new Map<Comment, OutComment>();
  let changed = false;
  for (const c of eList) {
    const bComment = baselineOf.get(c);
    const original = bComment ? (originalOf.get(bComment) ?? null) : null;
    const now = stateOf(c, ec as CommentsPart, eExt);
    const x: OutComment = {
      outId: original ? original.id : freshId(),
      xml: "",
      lastParaId: null,
      done: now.done,
      parent: null,
      isNew: !original,
      original,
      edited: c,
    };
    if (!original || !bComment) {
      changed = true;
      x.xml = withId(repairComment(c.xml), x.outId);
      x.lastParaId = c.lastParaId;
    } else {
      const then = stateOf(bComment, bc as CommentsPart, bExt);
      const parentKey = (p: Comment | null) => p?.internalId ?? null;
      if (
        then.done !== now.done ||
        parentKey(then.parent) !== parentKey(now.parent)
      ) {
        changed = true;
      }
      if (c.key === bComment.key) {
        x.xml = original.xml;
        x.lastParaId = original.lastParaId;
      } else {
        changed = true;
        x.xml = withId(repairComment(c.xml), x.outId);
        x.lastParaId = original.lastParaId ?? c.lastParaId;
        if (original.lastParaId) {
          x.xml = withLastParaId(x.xml, original.lastParaId);
        }
      }
    }
    editedIds.set(c.id, x.outId);
    out.push(x);
    outOfEdited.set(c, x);
  }
  const kept = new Set(out.map((x) => x.original).filter(Boolean));
  if (oList.some((c) => !kept.has(c))) changed = true;
  if (!changed) return { baselineIds, editedIds, parts: {} };

  for (const x of out) {
    const parent = stateOf(x.edited, ec as CommentsPart, eExt).parent;
    x.parent = parent ? (outOfEdited.get(parent) ?? null) : null;
  }

  const parts: Record<string, Uint8Array | null> = {};
  const removeIfPresent = (name: string) => {
    if (o[name]) parts[name] = null;
  };
  if (out.length === 0) {
    removeIfPresent(COMMENTS);
    removeIfPresent(COMMENTS_EXTENDED);
    removeIfPresent(COMMENTS_IDS);
    removeIfPresent(COMMENTS_EXTENSIBLE);
    return { baselineIds, editedIds, parts };
  }

  // Paragraph ids where commentsExtended.xml needs them.
  const extWanted =
    oExt.split != null || out.some((x) => x.done || x.parent != null);
  const taken = paraIdsIn(o, e);
  const ensureParaId = (x: OutComment) => {
    if (x.lastParaId) return;
    const id = newParaId(taken);
    const xml = withLastParaId(x.xml, id);
    if (xml !== x.xml) {
      x.xml = xml;
      x.lastParaId = id;
    }
  };
  if (extWanted) {
    for (const x of out) {
      if (x.done || x.parent != null || x.isNew) ensureParaId(x);
      if (x.parent) ensureParaId(x.parent);
    }
  }

  // comments.xml
  const fromEdit = out
    .filter((x) => x.isNew || x.xml !== x.original?.xml)
    .map((x) => x.xml)
    .join("");
  if (oc) {
    const byOriginal = new Map(
      out.filter((x) => x.original).map((x) => [x.original, x]),
    );
    const blocks: { gap: string; xml: string }[] = [];
    oc.split.blocks.forEach((block, i) => {
      const comment = oc.atBlock.get(i);
      if (!comment) {
        blocks.push({ gap: oc.split.gaps[i], xml: block });
        return;
      }
      const x = byOriginal.get(comment);
      if (x) blocks.push({ gap: oc.split.gaps[i], xml: x.xml });
    });
    const gap = oc.split.gaps[0] ?? "";
    for (const x of out) if (x.isNew) blocks.push({ gap, xml: x.xml });
    let root = mergeRootTag(
      oc.split.rootTag,
      ec?.split.rootTag ?? "",
      prefixesUsed(fromEdit),
    );
    root = declare(root, prefixesUsed(blocks.map((b) => b.xml).join("")));
    parts[COMMENTS] = strToU8(rebuild(oc.split, blocks, root));
  } else if (ec) {
    const blocks = out.map((x) => ({ gap: "", xml: x.xml }));
    parts[COMMENTS] = strToU8(rebuild(ec.split, blocks, ec.split.rootTag));
  }

  // commentsExtended.xml
  const paraOf = (x: OutComment | null) => x?.lastParaId ?? null;
  if (extWanted) {
    const hasEntry = new Set<OutComment>();
    const blocks: { gap: string; xml: string }[] = [];
    let base: SplitPart;
    if (oExt.split) {
      base = oExt.split;
      const byOriginalPara = new Map<string, OutComment>();
      for (const x of out) {
        if (x.original?.lastParaId)
          byOriginalPara.set(x.original.lastParaId, x);
      }
      const gone = new Set(
        oList
          .filter((c) => !kept.has(c))
          .map((c) => c.lastParaId)
          .filter(Boolean),
      );
      base.blocks.forEach((block, i) => {
        const entry = oExt.entries[i];
        const gap = base.gaps[i];
        if (!entry) {
          blocks.push({ gap, xml: block });
          return;
        }
        if (gone.has(entry.paraId)) return;
        const x = byOriginalPara.get(entry.paraId);
        if (!x || !x.lastParaId) {
          blocks.push({ gap, xml: block });
          return;
        }
        hasEntry.add(x);
        const parentPara = paraOf(x.parent);
        if (
          entry.done === x.done &&
          entry.parentParaId === parentPara &&
          entry.paraId === x.lastParaId
        ) {
          blocks.push({ gap, xml: block });
          return;
        }
        let tag = startTag(block);
        tag = setAttribute(tag, "w15:paraId", x.lastParaId);
        tag = setAttribute(tag, "w15:done", x.done ? "1" : "0");
        tag = parentPara
          ? setAttribute(tag, "w15:paraIdParent", parentPara)
          : removeAttributes(tag, (n) => n === "w15:paraIdParent");
        blocks.push({ gap, xml: tag + block.slice(startTag(block).length) });
      });
    } else {
      base = split(
        text(e, COMMENTS_EXTENDED) ?? EMPTY_EXT,
        "commentsExtended.xml",
      );
    }
    const gap = base.gaps[0] ?? "";
    for (const x of out) {
      if (hasEntry.has(x) || !x.lastParaId) continue;
      if (!(x.done || x.parent || x.isNew || !oExt.split)) continue;
      blocks.push({
        gap,
        xml: extEntryXml(x.lastParaId, x.done, paraOf(x.parent)),
      });
    }
    parts[COMMENTS_EXTENDED] = strToU8(
      rebuild(base, blocks, declare(base.rootTag, ["w15"])),
    );
  }

  // commentsIds.xml and commentsExtensible.xml: only where the file has them.
  const oIds = text(o, COMMENTS_IDS);
  if (oIds != null) {
    const s = split(oIds, "commentsIds.xml");
    const goneParas = new Set(
      oList
        .filter((c) => !kept.has(c))
        .map((c) => c.lastParaId)
        .filter(Boolean),
    );
    const droppedDurable = new Set<string>();
    const blocks: { gap: string; xml: string }[] = [];
    s.blocks.forEach((block, i) => {
      const attrs = attributes(startTag(block));
      if (goneParas.has(attrs.get("w16cid:paraId") ?? "")) {
        const durable = attrs.get("w16cid:durableId");
        if (durable) droppedDurable.add(durable);
        return;
      }
      blocks.push({ gap: s.gaps[i], xml: block });
    });
    // Added comments take the entries SuperDoc made for them.
    const eIds = text(e, COMMENTS_IDS);
    const eIdBlocks = eIds
      ? (splitPart(openEmptyRoot(eIds), "root")?.blocks ?? [])
      : [];
    const addedDurable = new Map<string, string>();
    for (const x of out) {
      if (!x.isNew || !x.lastParaId) continue;
      const entry = eIdBlocks.find(
        (bl) => attributes(startTag(bl)).get("w16cid:paraId") === x.lastParaId,
      );
      if (!entry) continue;
      blocks.push({ gap: s.gaps[0] ?? "", xml: entry });
      const durable = attributes(startTag(entry)).get("w16cid:durableId");
      if (durable) addedDurable.set(durable, entry);
    }
    const idsXml = rebuild(s, blocks, s.rootTag);
    if (idsXml !== oIds) parts[COMMENTS_IDS] = strToU8(idsXml);

    const oExtensible = text(o, COMMENTS_EXTENSIBLE);
    if (oExtensible != null) {
      const sx = split(oExtensible, "commentsExtensible.xml");
      const xBlocks = sx.blocks
        .map((block, i) => ({ gap: sx.gaps[i], xml: block }))
        .filter(
          (bl) =>
            !droppedDurable.has(
              attributes(startTag(bl.xml)).get("w16cex:durableId") ?? "",
            ),
        );
      const eExtensible = text(e, COMMENTS_EXTENSIBLE);
      const eBlocks = eExtensible
        ? (splitPart(openEmptyRoot(eExtensible), "root")?.blocks ?? [])
        : [];
      for (const durable of addedDurable.keys()) {
        const entry = eBlocks.find(
          (bl) => attributes(startTag(bl)).get("w16cex:durableId") === durable,
        );
        if (entry) xBlocks.push({ gap: sx.gaps[0] ?? "", xml: entry });
      }
      const xml = rebuild(sx, xBlocks, sx.rootTag);
      if (xml !== oExtensible) parts[COMMENTS_EXTENSIBLE] = strToU8(xml);
    }
  }

  // Leave out what is no different from the file.
  for (const [name, bytes] of Object.entries(parts)) {
    if (bytes && o[name] && strFromU8(bytes) === strFromU8(o[name])) {
      delete parts[name];
    }
  }
  return { baselineIds, editedIds, parts };
}
