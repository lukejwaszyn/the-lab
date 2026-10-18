"""A synthetic Word file with comments as Word 2016+ writes them: a thread
(a comment and a reply), an open comment, a comment already resolved, and
a comment to delete, with commentsExtended, commentsIds,
commentsExtensible and people parts. Usage: make_comments.py OUT.docx"""
import sys
import zipfile

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
NS = (
    'xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" '
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    f'xmlns:w="{W}" '
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" '
    'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" '
    'xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" '
    'xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" '
    'mc:Ignorable="w14 w15 w16cex w16cid"'
)
DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\r\n'

# id, author, initials, date, paraId, durableId, text, parent id, done
COMMENTS = [
    (0, "Reviewer One", "RO", "2026-10-01T09:00:00Z", "1A2B3C01", "5A1B2C01", "Which antenna is this?", None, False),
    (1, "Reviewer Two", "RT", "2026-10-01T10:30:00Z", "1A2B3C02", "5A1B2C02", "The log-periodic one, I think.", 0, False),
    (2, "Reviewer One", "RO", "2026-10-02T08:15:00Z", "1A2B3C03", "5A1B2C03", "Give the source for this figure.", None, False),
    (3, "Reviewer Two", "RT", "2026-10-02T11:00:00Z", "1A2B3C04", "5A1B2C04", "Fixed in the last draft.", None, True),
    (4, "Reviewer One", "RO", "2026-10-03T14:45:00Z", "1A2B3C05", "5A1B2C05", "This sentence can go.", None, False),
]


def run(text, rsid="00A1B2C3"):
    space = ' xml:space="preserve"' if text != text.strip() else ""
    return f'<w:r w:rsidR="{rsid}"><w:t{space}>{text}</w:t></w:r>'


def ref(i):
    return f'<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{i}"/></w:r>'


def para(inner, pid):
    return f'<w:p w14:paraId="{pid}" w14:textId="77777777" w:rsidR="00A1B2C3" w:rsidRDefault="00A1B2C3">{inner}</w:p>'


body = "".join([
    para('<w:pPr><w:pStyle w:val="Heading1"/></w:pPr>' + run("Design notes"), "0B000001"),
    para(run("This paragraph has no comments and is here to be edited."), "0B000002"),
    para(
        run("The receiver uses ")
        + '<w:commentRangeStart w:id="0"/><w:commentRangeStart w:id="1"/>'
        + run("a wideband antenna")
        + '<w:commentRangeEnd w:id="0"/>' + ref(0)
        + '<w:commentRangeEnd w:id="1"/>' + ref(1)
        + run(" fed through a short coaxial line."),
        "0B000003",
    ),
    para(
        '<w:commentRangeStart w:id="2"/>' + run("Figure 2 shows the measured return loss.")
        + '<w:commentRangeEnd w:id="2"/>' + ref(2),
        "0B000004",
    ),
    para(
        run("The sweep runs from 200 to 1000 MHz. ")
        + '<w:commentRangeStart w:id="3"/>' + run("Each step takes 5 ms.")
        + '<w:commentRangeEnd w:id="3"/>' + ref(3),
        "0B000005",
    ),
    para(
        '<w:commentRangeStart w:id="4"/>' + run("This sentence repeats the one before it.")
        + '<w:commentRangeEnd w:id="4"/>' + ref(4),
        "0B000006",
    ),
    para(run("A closing paragraph with no comments."), "0B000007"),
    '<w:sectPr w:rsidR="00A1B2C3"><w:pgSz w:w="12240" w:h="15840"/>'
    '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>'
    '<w:cols w:space="720"/><w:docGrid w:linePitch="360"/></w:sectPr>',
])
document = f"{DECL}<w:document {NS}><w:body>{body}</w:body></w:document>"

comments = "".join(
    f'<w:comment w:id="{i}" w:author="{a}" w:date="{d}" w:initials="{ini}">'
    f'<w:p w14:paraId="{pid}" w14:textId="77777777" w:rsidR="00B1C2D3" w:rsidRDefault="00B1C2D3">'
    '<w:pPr><w:pStyle w:val="CommentText"/></w:pPr>'
    '<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:annotationRef/></w:r>'
    f'<w:r><w:t>{t}</w:t></w:r></w:p></w:comment>'
    for i, a, ini, d, pid, _dur, t, _p, _done in COMMENTS
)
comments_xml = f"{DECL}<w:comments {NS}>{comments}</w:comments>"

by_id = {c[0]: c for c in COMMENTS}
ext = "".join(
    f'<w15:commentEx w15:paraId="{pid}"'
    + (f' w15:paraIdParent="{by_id[p][4]}"' if p is not None else "")
    + f' w15:done="{1 if done else 0}"/>'
    for _i, _a, _ini, _d, pid, _dur, _t, p, done in COMMENTS
)
ext_xml = f"{DECL}<w15:commentsEx {NS}>{ext}</w15:commentsEx>"
ids_xml = DECL + f"<w16cid:commentsIds {NS}>" + "".join(
    f'<w16cid:commentId w16cid:paraId="{pid}" w16cid:durableId="{dur}"/>'
    for _i, _a, _ini, _d, pid, dur, _t, _p, _done in COMMENTS
) + "</w16cid:commentsIds>"
extensible_xml = DECL + f"<w16cex:commentsExtensible {NS}>" + "".join(
    f'<w16cex:commentExtensible w16cex:durableId="{dur}" w16cex:dateUtc="{d}"/>'
    for _i, _a, _ini, d, _pid, dur, _t, _p, _done in COMMENTS
) + "</w16cex:commentsExtensible>"
people_xml = DECL + f"<w15:people {NS}>" + "".join(
    f'<w15:person w15:author="{a}"><w15:presenceInfo w15:providerId="None" w15:userId="{a}"/></w15:person>'
    for a in ["Reviewer One", "Reviewer Two"]
) + "</w15:people>"

styles_xml = DECL + f"""<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Times New Roman"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style w:type="character" w:styleId="CommentReference"><w:name w:val="annotation reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:sz w:val="16"/><w:szCs w:val="16"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="CommentText"><w:name w:val="annotation text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style></w:styles>"""
settings_xml = DECL + f'<w:settings {NS}><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat><w:rsids><w:rsidRoot w:val="00A1B2C3"/><w:rsid w:val="00A1B2C3"/><w:rsid w:val="00B1C2D3"/></w:rsids></w:settings>'

R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
doc_rels = DECL + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + "".join([
    f'<Relationship Id="rId1" Type="{R}/styles" Target="styles.xml"/>',
    f'<Relationship Id="rId2" Type="{R}/settings" Target="settings.xml"/>',
    f'<Relationship Id="rId3" Type="{R}/comments" Target="comments.xml"/>',
    '<Relationship Id="rId4" Type="http://schemas.microsoft.com/office/2011/relationships/commentsExtended" Target="commentsExtended.xml"/>',
    '<Relationship Id="rId5" Type="http://schemas.microsoft.com/office/2016/09/relationships/commentsIds" Target="commentsIds.xml"/>',
    '<Relationship Id="rId6" Type="http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible" Target="commentsExtensible.xml"/>',
    '<Relationship Id="rId7" Type="http://schemas.microsoft.com/office/2011/relationships/people" Target="people.xml"/>',
]) + "</Relationships>"
root_rels = DECL + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + (
    f'<Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/>'
    '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
    f'<Relationship Id="rId3" Type="{R}/extended-properties" Target="docProps/app.xml"/>'
) + "</Relationships>"
WML = "application/vnd.openxmlformats-officedocument.wordprocessingml"
content_types = DECL + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">' + "".join([
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>',
    '<Default Extension="xml" ContentType="application/xml"/>',
    f'<Override PartName="/word/document.xml" ContentType="{WML}.document.main+xml"/>',
    f'<Override PartName="/word/styles.xml" ContentType="{WML}.styles+xml"/>',
    f'<Override PartName="/word/settings.xml" ContentType="{WML}.settings+xml"/>',
    f'<Override PartName="/word/comments.xml" ContentType="{WML}.comments+xml"/>',
    f'<Override PartName="/word/commentsExtended.xml" ContentType="{WML}.commentsExtended+xml"/>',
    f'<Override PartName="/word/commentsIds.xml" ContentType="{WML}.commentsIds+xml"/>',
    f'<Override PartName="/word/commentsExtensible.xml" ContentType="{WML}.commentsExtensible+xml"/>',
    f'<Override PartName="/word/people.xml" ContentType="{WML}.people+xml"/>',
    '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>',
    '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>',
]) + "</Types>"
core = DECL + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Comments test</dc:title><dc:creator>Reviewer One</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">2026-10-01T09:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2026-10-03T14:45:00Z</dcterms:modified></cp:coreProperties>'
app = DECL + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Microsoft Office Word</Application><Company>Example Lab</Company></Properties>'

parts = [
    ("[Content_Types].xml", content_types),
    ("_rels/.rels", root_rels),
    ("word/document.xml", document),
    ("word/_rels/document.xml.rels", doc_rels),
    ("word/styles.xml", styles_xml),
    ("word/settings.xml", settings_xml),
    ("word/comments.xml", comments_xml),
    ("word/commentsExtended.xml", ext_xml),
    ("word/commentsIds.xml", ids_xml),
    ("word/commentsExtensible.xml", extensible_xml),
    ("word/people.xml", people_xml),
    ("docProps/core.xml", core),
    ("docProps/app.xml", app),
]
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    for name, xml in parts:
        info = zipfile.ZipInfo(name, date_time=(2026, 10, 3, 14, 45, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, xml.encode("utf-8"))
