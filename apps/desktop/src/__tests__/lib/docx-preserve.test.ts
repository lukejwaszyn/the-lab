import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { strFromU8, strToU8, unzipSync, zipSync } from "fflate";
import { describe, expect, it } from "vitest";
import {
  alignBlocks,
  blockText,
  mergeRootTag,
  preserveUnchanged,
  splitPart,
  withoutVolatileIds,
} from "@/lib/docx/preserve";

const W =
  'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';
const W14 = 'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"';
const MC =
  'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"';
const R =
  'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';

const CONTENT_TYPES =
  '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>';
const ROOT_RELS =
  '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>';

function docRels(extra = "") {
  return `<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${extra}</Relationships>`;
}

function pkg(files: Record<string, string | Uint8Array>): Uint8Array {
  const entries: Record<string, Uint8Array> = {};
  for (const [name, data] of Object.entries(files)) {
    entries[name] = typeof data === "string" ? strToU8(data) : data;
  }
  return zipSync(entries);
}

function part(bytes: Uint8Array, name: string): string {
  return strFromU8(unzipSync(bytes)[name]);
}

/** Word writes paragraphs its own way... */
const wordP = (text: string) =>
  `<w:p w:rsidR="00A1B2C3"><w:r><w:t xml:space="preserve">${text}</w:t></w:r></w:p>`;
/** ...and SuperDoc re-serializes them its way, with random paragraph ids. */
let seed = 0;
const sdP = (text: string) =>
  `<w:p w14:paraId="${(++seed).toString(16).padStart(8, "0")}"><w:r><w:t>${text}</w:t></w:r></w:p>`;

function wordDoc(blocks: string[]) {
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:document ${W} ${R}><w:body>${blocks.join("")}<w:sectPr w:rsidR="00A1B2C3"/></w:body></w:document>`;
}
function sdDoc(blocks: string[]) {
  return `<?xml version="1.0" encoding="UTF-8"?><w:document ${W} ${R} ${W14} ${MC} mc:Ignorable="w14"><w:body>${blocks.join("")}<w:sectPr/></w:body></w:document>`;
}

const TEXTS = ["First paragraph.", "Second paragraph.", "Third & last."];

function original(extra: Record<string, string> = {}) {
  return pkg({
    "[Content_Types].xml": CONTENT_TYPES,
    "_rels/.rels": ROOT_RELS,
    "word/_rels/document.xml.rels": docRels(),
    "word/document.xml": wordDoc(
      TEXTS.map((t) => wordP(t.replace("&", "&amp;"))),
    ),
    "docProps/app.xml":
      "<Properties><Words>6</Words><Company>Lab</Company></Properties>",
    ...extra,
  });
}

function exported(
  texts: string[],
  extra: Record<string, string | Uint8Array> = {},
) {
  return pkg({
    "[Content_Types].xml": CONTENT_TYPES,
    "_rels/.rels": ROOT_RELS,
    "word/_rels/document.xml.rels": docRels(),
    "word/document.xml": sdDoc(texts.map((t) => sdP(t.replace("&", "&amp;")))),
    "docProps/app.xml": `<Properties><Words>${texts.length * 9}</Words></Properties>`,
    ...extra,
  });
}

describe("splitPart", () => {
  it("splits a body into its blocks and puts them back exactly", () => {
    const xml = `<?xml version="1.0"?><w:document ${W}><w:body>\n  <w:p><w:r><w:t>a &gt; b</w:t></w:r></w:p><!-- note -->\n  <w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p w:x="1>2"/><w:sectPr/>\n</w:body></w:document>`;
    const split = splitPart(xml, "body");
    expect(split).not.toBeNull();
    if (!split) return;
    expect(split.blocks).toHaveLength(4);
    expect(split.blocks[1].startsWith("<w:tbl>")).toBe(true);
    expect(split.blocks[2]).toBe('<w:p w:x="1>2"/>');
    expect(split.rootTag).toBe(`<w:document ${W}>`);
    let rebuilt = split.head;
    split.blocks.forEach((b, i) => {
      rebuilt += split.gaps[i] + b;
    });
    rebuilt += split.gaps[split.blocks.length] + split.tail;
    expect(rebuilt).toBe(xml);
  });

  it("splits notes and headers under their root", () => {
    const xml = `<w:footnotes ${W}><w:footnote w:id="1"><w:p/></w:footnote><w:footnote w:id="2"/></w:footnotes>`;
    expect(splitPart(xml, "root")?.blocks).toEqual([
      '<w:footnote w:id="1"><w:p/></w:footnote>',
      '<w:footnote w:id="2"/>',
    ]);
  });

  it("refuses what is not well-formed", () => {
    expect(splitPart(`<w:document ${W}><w:body><w:p>`, "body")).toBeNull();
    expect(splitPart("<w:document><w:body></w:p></w:body>", "body")).toBeNull();
  });
});

describe("matching blocks", () => {
  it("reads a block's text with entities decoded and spacing ignored", () => {
    expect(
      blockText(
        '<w:p><w:r><w:t xml:space="preserve">A &amp; B </w:t></w:r><m:r><m:t>x y</m:t></m:r><w:del><w:r><w:delText>old</w:delText></w:r></w:del></w:p>',
      ),
    ).toBe("A&Bxyold");
  });

  it("ignores the ids SuperDoc draws anew on each export", () => {
    expect(
      withoutVolatileIds(
        '<w:p w14:paraId="0A1B2C3D" w14:textId="77777777"><vt:lpwstr>197d6a81-2389-49db-8c4a-39298c501719</vt:lpwstr></w:p>',
      ),
    ).toBe("<w:p><vt:lpwstr></vt:lpwstr></w:p>");
  });

  it("aligns an edit script by the longest common run", () => {
    expect(alignBlocks(["a", "b", "c"], ["a", "x", "c"])).toEqual([
      { kind: "same", a: 0, b: 0 },
      { kind: "new", b: 1 },
      { kind: "gone", a: 1 },
      { kind: "same", a: 2, b: 2 },
    ]);
    expect(alignBlocks(["a", "c"], ["a", "b", "c"])).toEqual([
      { kind: "same", a: 0, b: 0 },
      { kind: "new", b: 1 },
      { kind: "same", a: 1, b: 2 },
    ]);
  });

  it("declares on the original root only the namespaces the edit uses", () => {
    const merged = mergeRootTag(
      `<w:document ${W}>`,
      `<w:document ${W} ${W14} ${MC} xmlns:w15="urn:w15" mc:Ignorable="w14 w15">`,
      new Set(["w", "w14"]),
    );
    expect(merged).toBe(`<w:document ${W} ${W14} ${MC} mc:Ignorable="w14">`);
    // Already declared: unchanged.
    expect(
      mergeRootTag(`<w:document ${W}>`, `<w:document ${W}>`, new Set(["w"])),
    ).toBe(`<w:document ${W}>`);
  });
});

describe("preserveUnchanged", () => {
  it("gives back the original bytes when nothing was edited", () => {
    const o = original();
    const result = preserveUnchanged(o, exported(TEXTS), exported(TEXTS));
    expect(result.preserved).toBe(true);
    expect(result.bytes).toBe(o);
    expect(result.changedParts).toEqual([]);
  });

  it("changes only the edited paragraph; the others stay byte for byte", () => {
    const o = original();
    const edited = [TEXTS[0], "Second paragraph, edited.", TEXTS[2]];
    const result = preserveUnchanged(o, exported(TEXTS), exported(edited));
    expect(result.preserved).toBe(true);
    expect(result.changedParts).toEqual(["word/document.xml"]);
    const before = splitPart(part(o, "word/document.xml"), "body");
    const after = splitPart(part(result.bytes, "word/document.xml"), "body");
    expect(after?.blocks[0]).toBe(before?.blocks[0]);
    expect(after?.blocks[2]).toBe(before?.blocks[2]);
    expect(after?.blocks[3]).toBe(before?.blocks[3]); // sectPr
    expect(blockText(after?.blocks[1] ?? "")).toBe("Secondparagraph,edited.");
    // The edited paragraph's w14 id is declared; the rest of the root is Word's.
    expect(after?.rootTag).toBe(
      `<w:document ${W} ${R} ${W14} ${MC} mc:Ignorable="w14">`,
    );
    // Word's properties stay (SuperDoc's word count and missing Company do not).
    expect(part(result.bytes, "docProps/app.xml")).toContain(
      "<Company>Lab</Company>",
    );
  });

  it("adds a new paragraph in place and drops a deleted one", () => {
    const o = original();
    const edited = [TEXTS[0], "A new one.", TEXTS[2]];
    const result = preserveUnchanged(o, exported(TEXTS), exported(edited));
    const blocks =
      splitPart(part(result.bytes, "word/document.xml"), "body")?.blocks ?? [];
    expect(blocks.map(blockText)).toEqual([
      "Firstparagraph.",
      "Anewone.",
      "Third&last.",
      "",
    ]);
    const inserted = preserveUnchanged(
      o,
      exported(TEXTS),
      exported([TEXTS[0], "Inserted.", ...TEXTS.slice(1)]),
    );
    const orig = splitPart(part(o, "word/document.xml"), "body")?.blocks ?? [];
    const now =
      splitPart(part(inserted.bytes, "word/document.xml"), "body")?.blocks ??
      [];
    expect(now).toHaveLength(5);
    expect(now.filter((b) => orig.includes(b))).toHaveLength(4);
  });

  it("writes SuperDoc's whole export when the blocks do not match up", () => {
    const o = original();
    const baseline = exported(["Something", "else"]);
    const edited = exported(["Something", "else", "entirely"]);
    const result = preserveUnchanged(o, baseline, edited);
    expect(result.preserved).toBe(false);
    expect(result.bytes).toBe(edited);
    expect(result.reason).toMatch(/blocks/);
  });

  it("writes SuperDoc's whole export when a block's text does not match", () => {
    const o = original();
    const changedOnLoad = ["First paragraph.", "Second paragraph?", TEXTS[2]];
    const result = preserveUnchanged(
      o,
      exported(changedOnLoad),
      exported([...changedOnLoad.slice(0, 2), "Edited."]),
    );
    expect(result.preserved).toBe(false);
  });

  it("falls back when the result names a relationship nobody defines", () => {
    const o = original();
    const broken = exported([TEXTS[0], TEXTS[1], TEXTS[2]]);
    const edited = pkg({
      "[Content_Types].xml": CONTENT_TYPES,
      "_rels/.rels": ROOT_RELS,
      "word/_rels/document.xml.rels": docRels(),
      "word/document.xml": sdDoc([
        sdP(TEXTS[0]),
        '<w:p><w:hyperlink r:id="rId42"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p>',
        sdP("Third &amp; last."),
      ]),
      "docProps/app.xml": "<Properties/>",
    });
    const result = preserveUnchanged(o, broken, edited);
    expect(result.preserved).toBe(false);
    expect(result.reason).toMatch(/rId42/);
  });

  it("takes the package's lists from the edit when the edit adds a part", () => {
    const o = original();
    const image = new Uint8Array([137, 80, 78, 71]);
    const rel =
      '<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>';
    const edited = pkg({
      "[Content_Types].xml": CONTENT_TYPES.replace(
        "<Override",
        '<Default Extension="png" ContentType="image/png"/><Override',
      ),
      "_rels/.rels": ROOT_RELS,
      "word/_rels/document.xml.rels": docRels(rel),
      "word/document.xml": sdDoc([
        sdP(TEXTS[0]),
        '<w:p><w:r><w:drawing><a:blip xmlns:a="urn:a" r:embed="rId9"/></w:drawing></w:r></w:p>',
        sdP("Third &amp; last."),
      ]),
      "word/media/image1.png": image,
      "docProps/app.xml": "<Properties/>",
    });
    const result = preserveUnchanged(o, exported(TEXTS), edited);
    expect(result.preserved).toBe(true);
    const files = unzipSync(result.bytes);
    expect(files["word/media/image1.png"]).toEqual(image);
    expect(strFromU8(files["word/_rels/document.xml.rels"])).toContain("rId9");
    expect(strFromU8(files["[Content_Types].xml"])).toContain(
      'Extension="png"',
    );
  });

  it("keeps a part SuperDoc never writes", () => {
    const o = original({ "customXml/item1.xml": "<b:Sources/>" });
    const result = preserveUnchanged(
      o,
      exported(TEXTS),
      exported([TEXTS[0], "Edited.", TEXTS[2]]),
    );
    expect(part(result.bytes, "customXml/item1.xml")).toBe("<b:Sources/>");
  });

  it("leaves out parts SuperDoc adds when the edit did not need them", () => {
    const o = original();
    const extra = { "word/commentsExtended.xml": "<w15:commentsEx/>" };
    const result = preserveUnchanged(
      o,
      exported(TEXTS, extra),
      exported([TEXTS[0], "Edited.", TEXTS[2]], extra),
    );
    expect(
      unzipSync(result.bytes)["word/commentsExtended.xml"],
    ).toBeUndefined();
  });
});

describe("preserveUnchanged on SuperDoc 1.47.2's own exports", () => {
  // A synthetic document (fixtures/docx/hard.md, made with pandoc) with
  // equations, footnotes, a table, an image, an advisor's tracked changes
  // and a comment; SuperDoc's export of it as loaded; and its export after
  // a paragraph was edited and two were added.
  const dir = resolve(__dirname, "../fixtures/docx");
  const read = (name: string) =>
    new Uint8Array(readFileSync(resolve(dir, name)));
  const o = read("hard.docx");
  const baseline = read("hard.superdoc-baseline.docx");
  const edited = read("hard.superdoc-edited.docx");

  it("SuperDoc alone rewrites what was not edited", () => {
    const ours = splitPart(part(o, "word/document.xml"), "body")?.blocks ?? [];
    const its =
      splitPart(part(baseline, "word/document.xml"), "body")?.blocks ?? [];
    expect(its.filter((b) => ours.includes(b)).length).toBeLessThan(
      ours.length / 2,
    );
  });

  it("keeps every block that was not edited, byte for byte", () => {
    const result = preserveUnchanged(o, baseline, edited);
    expect(result.preserved).toBe(true);
    expect(result.changedParts).toEqual(["word/document.xml"]);
    const before =
      splitPart(part(o, "word/document.xml"), "body")?.blocks ?? [];
    const after =
      splitPart(part(result.bytes, "word/document.xml"), "body")?.blocks ?? [];
    expect(after).toHaveLength(before.length + 2);
    const kept = before.filter((b) => after.includes(b));
    expect(kept).toHaveLength(before.length - 1);
    const fresh = after.filter((b) => !before.includes(b)).map(blockText);
    expect(fresh).toEqual([
      "[EditedinTheLab]Thelosstangentisabout0.01.",
      "",
      "AnewparagraphwritteninTheLab.",
    ]);
    // Display equations stay in their paragraphs, with their spacing.
    const xml = part(result.bytes, "word/document.xml");
    expect(
      xml.match(/<w:p><w:pPr><w:pStyle w:val="\w+" \/><\/w:pPr><m:oMathPara>/g),
    ).toHaveLength(2);
    expect(xml).toContain("  ");
    // Footnotes and the advisor's comment are the original's.
    for (const name of [
      "word/footnotes.xml",
      "word/comments.xml",
      "word/styles.xml",
    ]) {
      expect(part(result.bytes, name)).toBe(part(o, name));
    }
  });
});
