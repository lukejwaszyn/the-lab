import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { strFromU8, strToU8, unzipSync, zipSync } from "fflate";
import { describe, expect, it } from "vitest";
import { preserveUnchanged, splitPart } from "@/lib/docx/preserve";
import { repairComment } from "@/lib/docx/preserve-comments";

// comments.docx is synthetic (make-comments.py): comments as Word 2016+
// writes them, with a thread, a resolved comment and the extended parts.
// Each pair of the others is SuperDoc 1.47.2's export of a file as it
// loaded and after one change made in its own comment panel.
const dir = resolve(__dirname, "../fixtures/docx");
const read = (name: string) => new Uint8Array(readFileSync(resolve(dir, name)));

type Files = Record<string, string>;

function parts(bytes: Uint8Array): Files {
  const out: Files = {};
  for (const [name, data] of Object.entries(unzipSync(bytes))) {
    if (!name.endsWith("/")) out[name] = strFromU8(data);
  }
  return out;
}

function all(xml: string, re: RegExp): string[] {
  return [...xml.matchAll(re)].map((m) => m[1]);
}

function count<T>(items: T[]): Map<T, number> {
  const out = new Map<T, number>();
  for (const i of items) out.set(i, (out.get(i) ?? 0) + 1);
  return out;
}

/** What Word would trip over in the comments of a saved file. */
function commentProblems(files: Files): string[] {
  const problems: string[] = [];
  const doc = files["word/document.xml"] ?? "";
  const comments = files["word/comments.xml"] ?? "";
  const ids = all(comments, /<w:comment\b[^>]*?\sw:id="([^"]*)"/g);
  for (const [id, n] of count(ids)) {
    if (n > 1) problems.push(`comment ${id} defined ${n} times`);
  }
  for (const kind of [
    "commentRangeStart",
    "commentRangeEnd",
    "commentReference",
  ]) {
    const used = all(
      doc,
      new RegExp(`<w:${kind}\\b[^>]*?\\sw:id="([^"]*)"`, "g"),
    );
    for (const [id, n] of count(used)) {
      if (n > 1) problems.push(`${kind} ${id} appears ${n} times`);
      if (!ids.includes(id)) problems.push(`${kind} ${id} has no comment`);
    }
  }
  const starts = new Set(
    all(doc, /<w:commentRangeStart\b[^>]*?\sw:id="([^"]*)"/g),
  );
  const ends = new Set(all(doc, /<w:commentRangeEnd\b[^>]*?\sw:id="([^"]*)"/g));
  for (const id of starts)
    if (!ends.has(id)) problems.push(`range ${id} not closed`);
  for (const id of ends)
    if (!starts.has(id)) problems.push(`range ${id} not opened`);
  for (const block of comments.match(/<w:comment\b[\s\S]*?<\/w:comment>/g) ??
    []) {
    const id = /\sw:id="([^"]*)"/.exec(block)?.[1];
    if (/\scustom:/.test(block))
      problems.push(`comment ${id} has custom: attributes`);
    const refs = block.match(/<w:annotationRef\s*\/>/g)?.length ?? 0;
    if (refs > 1) problems.push(`comment ${id} has ${refs} annotationRef runs`);
    for (const p of block.match(/<w:p\b[\s\S]*?<\/w:p>/g) ?? []) {
      const inner = p.replace(/^<w:p\b[^>]*>/, "");
      if (inner.includes("<w:pPr") && !inner.startsWith("<w:pPr")) {
        problems.push(`comment ${id}: w:pPr is not first`);
      }
    }
  }
  const paras = new Set(
    all(comments, /\sw14:paraId="([0-9A-Fa-f]+)"/g).map((p) => p.toUpperCase()),
  );
  const ext = files["word/commentsExtended.xml"] ?? "";
  for (const p of all(ext, /\sw15:(?:paraId|paraIdParent)="([0-9A-Fa-f]+)"/g)) {
    if (!paras.has(p.toUpperCase()))
      problems.push(`commentsExtended names ${p}`);
  }
  const commentIds = files["word/commentsIds.xml"] ?? "";
  for (const p of all(commentIds, /\sw16cid:paraId="([0-9A-Fa-f]+)"/g)) {
    if (!paras.has(p.toUpperCase())) problems.push(`commentsIds names ${p}`);
  }
  const durable = new Set(all(commentIds, /\sw16cid:durableId="([^"]*)"/g));
  const extensible = files["word/commentsExtensible.xml"] ?? "";
  for (const d of all(extensible, /\sw16cex:durableId="([^"]*)"/g)) {
    if (commentIds && !durable.has(d))
      problems.push(`commentsExtensible names ${d}`);
  }
  // Every part is listed, and every relationship's target exists.
  const types = files["[Content_Types].xml"] ?? "";
  for (const name of Object.keys(files)) {
    if (!name.startsWith("word/") || !name.endsWith(".xml")) continue;
    if (!types.includes(`PartName="/${name}"`))
      problems.push(`no content type for ${name}`);
  }
  for (const target of all(
    files["word/_rels/document.xml.rels"] ?? "",
    /\sTarget="([^"]*)"/g,
  )) {
    if (!files[`word/${target}`])
      problems.push(`relationship to missing ${target}`);
  }
  return problems;
}

/** The parts of `after` that differ from `before`. */
function changed(before: Files, after: Files): string[] {
  return [...new Set([...Object.keys(before), ...Object.keys(after)])]
    .filter((n) => before[n] !== after[n])
    .sort();
}

function save(original: string, baseline: string, edited: string) {
  const o = read(original);
  const result = preserveUnchanged(o, read(baseline), read(edited));
  return { result, before: parts(o), after: parts(result.bytes) };
}

/** The original's comments, one by one. */
function commentBlocks(files: Files): string[] {
  return (
    files["word/comments.xml"]?.match(/<w:comment\b[\s\S]*?<\/w:comment>/g) ??
    []
  );
}

describe("SuperDoc 1.47.2's own export of comments", () => {
  it("writes what Word does not: custom attributes and a second annotationRef", () => {
    const exported = parts(read("comments.superdoc-reply.docx"));
    const problems = commentProblems(exported);
    expect(problems.some((p) => p.includes("custom: attributes"))).toBe(true);
    expect(problems.some((p) => p.includes("2 annotationRef runs"))).toBe(true);
  });
});

describe("saving comments changed in SuperDoc", () => {
  it("adds a reply, threaded under its comment, and keeps every other comment", () => {
    const { result, before, after } = save(
      "comments.docx",
      "comments.superdoc-reply-baseline.docx",
      "comments.superdoc-reply.docx",
    );
    expect(result.preserved).toBe(true);
    expect(commentProblems(after)).toEqual([]);
    expect(changed(before, after)).toEqual([
      "word/comments.xml",
      "word/commentsExtended.xml",
      "word/commentsExtensible.xml",
      "word/commentsIds.xml",
      "word/document.xml",
    ]);
    for (const c of commentBlocks(before)) {
      expect(after["word/comments.xml"]).toContain(c);
    }
    const added = commentBlocks(after).filter(
      (c) => !commentBlocks(before).includes(c),
    );
    expect(added).toHaveLength(1);
    expect(added[0]).toMatch(/w:id="5"/);
    expect(added[0]).toContain("Added the source in the caption.");
    const paraId = /w14:paraId="([0-9A-F]+)"/.exec(added[0])?.[1];
    expect(after["word/commentsExtended.xml"]).toContain(
      `w15:paraId="${paraId}" w15:paraIdParent="1A2B3C03"`,
    );
    // Only the commented paragraph changed, and it holds both ranges.
    const blocks = (files: Files) =>
      splitPart(files["word/document.xml"], "body")?.blocks ?? [];
    const fresh = blocks(after).filter((b) => !blocks(before).includes(b));
    expect(fresh).toHaveLength(1);
    expect(fresh[0]).toMatch(
      /commentRangeStart w:id="2"[\s\S]*commentRangeStart w:id="5"/,
    );
  });

  it("resolves a thread by changing only commentsExtended.xml", () => {
    const { result, before, after } = save(
      "comments.docx",
      "comments.superdoc-resolve-baseline.docx",
      "comments.superdoc-resolve.docx",
    );
    expect(result.preserved).toBe(true);
    expect(commentProblems(after)).toEqual([]);
    expect(changed(before, after)).toEqual(["word/commentsExtended.xml"]);
    const ext = after["word/commentsExtended.xml"];
    expect(ext).toContain('w15:paraId="1A2B3C01" w15:done="1"');
    expect(ext).toContain(
      'w15:paraId="1A2B3C02" w15:paraIdParent="1A2B3C01" w15:done="1"',
    );
    // The comment resolved in Word before stays resolved.
    expect(ext).toContain('w15:paraId="1A2B3C04" w15:done="1"');
  });

  it("adds a new comment with its entries in every comment part", () => {
    const { result, before, after } = save(
      "comments.docx",
      "comments.superdoc-add-baseline.docx",
      "comments.superdoc-add.docx",
    );
    expect(result.preserved).toBe(true);
    expect(commentProblems(after)).toEqual([]);
    for (const c of commentBlocks(before)) {
      expect(after["word/comments.xml"]).toContain(c);
    }
    const added = commentBlocks(after).filter(
      (c) => !commentBlocks(before).includes(c),
    );
    expect(added).toHaveLength(1);
    const paraId = /w14:paraId="([0-9A-F]+)"/.exec(added[0])?.[1];
    expect(after["word/commentsIds.xml"]).toContain(
      `w16cid:paraId="${paraId}"`,
    );
    expect(after["word/commentsExtended.xml"]).toContain(
      `w15:paraId="${paraId}"`,
    );
    expect(after["word/people.xml"]).toBe(before["word/people.xml"]);
  });

  it("deletes a comment from every part, and its range from the text", () => {
    const { result, before, after } = save(
      "comments.docx",
      "comments.superdoc-delete-baseline.docx",
      "comments.superdoc-delete.docx",
    );
    expect(result.preserved).toBe(true);
    expect(commentProblems(after)).toEqual([]);
    expect(after["word/comments.xml"]).not.toContain('w:id="4"');
    expect(after["word/document.xml"]).not.toMatch(/w:id="4"/);
    for (const name of ["word/commentsExtended.xml", "word/commentsIds.xml"]) {
      expect(after[name]).not.toContain("1A2B3C05");
    }
    expect(after["word/commentsExtensible.xml"]).not.toContain("5A1B2C05");
    // The other four are the original's.
    expect(commentBlocks(after)).toEqual(commentBlocks(before).slice(0, 4));
  });

  it("adds commentsExtended.xml, listed and related, to a file without one", () => {
    const { result, before, after } = save(
      "hard.docx",
      "hard.superdoc-reply-baseline.docx",
      "hard.superdoc-reply.docx",
    );
    expect(result.preserved).toBe(true);
    expect(commentProblems(after)).toEqual([]);
    expect(before["word/commentsExtended.xml"]).toBeUndefined();
    expect(after["[Content_Types].xml"]).toContain(
      'PartName="/word/commentsExtended.xml"',
    );
    expect(after["word/_rels/document.xml.rels"]).toContain(
      'Target="commentsExtended.xml"',
    );
    // The advisor's comment gets the paragraph id the reply points to.
    const [advisor] = commentBlocks(after);
    const paraId = /w14:paraId="([0-9A-F]+)"/.exec(advisor)?.[1];
    expect(paraId).toBeTruthy();
    expect(after["word/commentsExtended.xml"]).toContain(
      `w15:paraIdParent="${paraId}"`,
    );
    for (const name of ["word/footnotes.xml", "word/styles.xml"]) {
      expect(after[name]).toBe(before[name]);
    }
  });
});

describe("comment ids", () => {
  const W =
    'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"';
  const types = `<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/></Types>`;
  const rels = `<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/></Relationships>`;
  const para = (text: string, ids: string[]) =>
    `<w:p>${ids.map((i) => `<w:commentRangeStart w:id="${i}"/>`).join("")}<w:r><w:t>${text}</w:t></w:r>${ids
      .map(
        (i) =>
          `<w:commentRangeEnd w:id="${i}"/><w:r><w:commentReference w:id="${i}"/></w:r>`,
      )
      .join("")}</w:p>`;
  const comment = (id: string, text: string, extra = "") =>
    `<w:comment w:id="${id}" w:author="Reviewer"${extra}><w:p w14:paraId="0000000${id}"><w:r><w:annotationRef/></w:r><w:r><w:t>${text}</w:t></w:r></w:p></w:comment>`;
  const pkg = (files: Record<string, string>) =>
    zipSync(
      Object.fromEntries(
        Object.entries({
          "[Content_Types].xml": types,
          "word/_rels/document.xml.rels": rels,
          ...files,
        }).map(([n, x]) => [n, strToU8(x)]),
      ),
    );
  const doc = (body: string) =>
    `<?xml version="1.0"?><w:document ${W}><w:body>${body}<w:sectPr/></w:body></w:document>`;
  const comments = (body: string) =>
    `<?xml version="1.0"?><w:comments ${W}>${body}</w:comments>`;

  it("keeps the file's own numbers, and gives a new comment one it does not use", () => {
    // Word numbered these 7 and 9; SuperDoc numbers them 0 and 1.
    const original = pkg({
      "word/document.xml": doc(para("Alpha", ["7"]) + para("Beta", ["9"])),
      "word/comments.xml": comments(
        comment("7", "First note.") + comment("9", "Second note."),
      ),
    });
    const ours = (id: string, internal: string, text: string) =>
      comment(id, text, ` custom:internalId="${internal}"`);
    const baseline = pkg({
      "word/document.xml": doc(para("Alpha", ["0"]) + para("Beta", ["1"])),
      "word/comments.xml": comments(
        ours("0", "a", "First note.") + ours("1", "b", "Second note."),
      ),
    });
    const edited = pkg({
      "word/document.xml": doc(para("Alpha", ["0"]) + para("Beta", ["1", "2"])),
      "word/comments.xml": comments(
        ours("0", "a", "First note.") +
          ours("1", "b", "Second note.") +
          ours("2", "c", "A third."),
      ),
    });
    const result = preserveUnchanged(original, baseline, edited);
    expect(result.preserved).toBe(true);
    const after = parts(result.bytes);
    expect(commentProblems(after)).toEqual([]);
    const blocks = splitPart(after["word/document.xml"], "body")?.blocks ?? [];
    expect(blocks[0]).toBe(para("Alpha", ["7"]));
    expect(all(blocks[1], /commentRangeStart w:id="([^"]*)"/g)).toEqual([
      "9",
      "10",
    ]);
    expect(
      all(after["word/comments.xml"], /<w:comment w:id="([^"]*)"/g),
    ).toEqual(["7", "9", "10"]);
  });
});

describe("repairComment", () => {
  it("puts w:pPr first, keeps one annotationRef, and drops SuperDoc's attributes", () => {
    const superdoc =
      '<w:comment w:id="3" w:author="A" custom:internalId="x-1" custom:trackedChange="false" xmlns:custom="urn:superdoc">' +
      '<w:p w14:paraId="0A0B0C0D"><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>' +
      '<w:pPr><w:pStyle w:val="CommentText"/></w:pPr>' +
      "<w:r><w:annotationRef/></w:r><w:r><w:t>Text</w:t></w:r></w:p></w:comment>";
    expect(repairComment(superdoc)).toBe(
      '<w:comment w:id="3" w:author="A">' +
        '<w:p w14:paraId="0A0B0C0D"><w:pPr><w:pStyle w:val="CommentText"/></w:pPr>' +
        '<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>' +
        "<w:r><w:t>Text</w:t></w:r></w:p></w:comment>",
    );
  });
});
