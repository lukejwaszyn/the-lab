//! Drives `thelab mcp` over pipes, the way a client does, through both
//! protocol eras.

mod common;

use std::fs;
use std::path::Path;

use common::{
    MODERN, Session, TempDir, copy_project, decode_base64, example_thesis, have, have_engine,
    text_of,
};
use serde_json::{Value, json};

const TOOLS: [&str; 7] = [
    "project_overview",
    "compile",
    "check_references",
    "find_citation",
    "render_page",
    "editor_context",
    "reveal_in_editor",
];

const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

fn tool_names(result: &Value) -> Vec<&str> {
    result["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
}

// ------------------------------------------------------------ modern era

#[test]
fn modern_discover_list_compile_and_render() {
    if !have_engine() || !have("pdftoppm") {
        return;
    }
    let project = copy_project(&example_thesis(), "modern");
    let state = TempDir::new("modern-state");
    let mut session = Session::start(project.path(), state.path(), &[]);

    // server/discover
    let discover = session.modern("server/discover", Value::Null);
    let result = &discover["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(
        result["supportedVersions"],
        json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"])
    );
    assert_eq!(result["capabilities"], json!({ "tools": {} }));
    assert_eq!(
        result[SERVER_INFO],
        Value::Null,
        "server info lives in _meta"
    );
    assert_eq!(result["_meta"][SERVER_INFO]["name"], "thelab");
    assert_eq!(
        result["_meta"][SERVER_INFO]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(result["ttlMs"].as_u64().unwrap() > 0);
    assert_eq!(result["cacheScope"], "public");
    let instructions = result["instructions"].as_str().unwrap();
    for must_mention in ["compile", "find_citation", "Never invent"] {
        assert!(
            instructions.contains(must_mention),
            "instructions mention {must_mention}"
        );
    }

    // tools/list
    let list = session.modern("tools/list", Value::Null);
    let result = &list["result"];
    assert_eq!(tool_names(result), TOOLS, "tools come in a fixed order");
    assert_eq!(result["resultType"], "complete");
    assert!(result["ttlMs"].as_u64().unwrap() > 0);
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(
        session.modern("tools/list", Value::Null)["result"]["tools"],
        result["tools"]
    );

    // project_overview
    let overview = session.call("project_overview", json!({}));
    assert_eq!(overview["isError"], false);
    let structured = &overview["structuredContent"];
    assert_eq!(structured["main"], "main.tex");
    assert_eq!(structured["outline"].as_array().unwrap().len(), 20);
    assert_eq!(
        structured["outline"][0],
        json!({
            "level": "chapter", "title": "Introduction",
            "file": "chapters/introduction.tex", "line": 1, "words": 75
        })
    );
    assert_eq!(structured["units"].as_array().unwrap().len(), 5);
    assert_eq!(structured["counts"]["bibEntries"], 7);
    assert!(text_of(&overview).contains("chapter \"Design\" (chapters/design.tex:1"));

    // compile
    let compiled = session.call("compile", json!({}));
    assert_eq!(compiled["isError"], false, "{compiled}");
    let structured = &compiled["structuredContent"];
    assert_eq!(structured["ok"], true);
    assert_eq!(structured["errors"], json!([]));
    assert_eq!(structured["errorCount"], 0);
    let pdf = structured["pdfPath"].as_str().expect("pdfPath");
    assert_eq!(Path::new(pdf), project.path().join("build/main.pdf"));
    assert!(Path::new(pdf).is_file());
    assert!(structured["durationMs"].is_u64());
    assert!(
        text_of(&compiled).starts_with("ok: "),
        "{}",
        text_of(&compiled)
    );

    // render_page
    let page = session.call("render_page", json!({ "page": 1 }));
    assert_eq!(page["isError"], false, "{page}");
    let image = &page["content"][0];
    assert_eq!(image["type"], "image");
    assert_eq!(image["mimeType"], "image/png");
    let data = image["data"].as_str().unwrap();
    let png = decode_base64(data);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "the image is a PNG");
    assert!(
        text_of(&page).starts_with("Page 1 of "),
        "{}",
        text_of(&page)
    );
    assert!(
        text_of(&page).contains("110 dpi (935x1210 px)"),
        "{}",
        text_of(&page)
    );

    // A dense page of text is passed through at the resolution asked for.
    let dense = session.call("render_page", json!({ "page": 4 }));
    assert!(
        text_of(&dense).contains("at 110 dpi"),
        "{}",
        text_of(&dense)
    );
    assert!(
        page.get("structuredContent").is_none(),
        "nothing may displace the image"
    );

    let beyond = session.call("render_page", json!({ "page": 999 }));
    assert_eq!(beyond["isError"], true);
    assert!(
        text_of(&beyond).contains("Page 999 does not exist"),
        "{}",
        text_of(&beyond)
    );

    // The slides are a second document in the same project.
    let slides = session.call("compile", json!({ "main": "slides/slides.tex" }));
    assert_eq!(slides["structuredContent"]["ok"], true, "{slides}");
    let slide = session.call(
        "render_page",
        json!({ "page": 2, "main": "slides/slides.tex" }),
    );
    assert_eq!(slide["content"][0]["type"], "image", "{slide}");
    assert!(
        text_of(&slide).contains("build/slides.pdf"),
        "{}",
        text_of(&slide)
    );

    // Break a chapter the way an edit might, and compile again.
    let chapter = project.path().join("chapters/design.tex");
    let original = fs::read_to_string(&chapter).unwrap();
    let broken_line = original.lines().count() + 2;
    fs::write(
        &chapter,
        format!("{original}\nThis sentence uses \\undefinedmacro by mistake.\n"),
    )
    .unwrap();

    let failed = session.call("compile", json!({}));
    assert_eq!(failed["isError"], true);
    let structured = &failed["structuredContent"];
    assert_eq!(structured["ok"], false);
    assert_eq!(
        structured["errors"][0],
        json!({
            "severity": "error",
            "file": "chapters/design.tex",
            "line": broken_line,
            "message": "Undefined control sequence"
        })
    );
    let expected = format!("error: chapters/design.tex:{broken_line}: Undefined control sequence");
    assert!(text_of(&failed).contains(&expected), "{}", text_of(&failed));

    session.finish();
}

#[test]
fn modern_citation_and_reference_tools_need_no_tex() {
    let project = copy_project(&example_thesis(), "cite");
    let state = TempDir::new("cite-state");
    let mut session = Session::start(project.path(), state.path(), &[]);

    let found = session.call("find_citation", json!({ "query": "Knuth 1984" }));
    assert_eq!(found["isError"], false);
    let keys: Vec<&str> = found["structuredContent"]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["knuth1984texbook", "knuth1984literate"]);
    assert_eq!(found["structuredContent"]["exact"], true);
    assert!(text_of(&found).contains("knuth1984texbook: Donald E. Knuth (1984). The TeXbook."));

    assert!(found["structuredContent"].get("note").is_none());

    // Claude Code shows the model the structured content, so the warning
    // has to be there as well as in the text.
    let nothing = session.call("find_citation", json!({ "query": "Feynman lectures" }));
    assert_eq!(nothing["structuredContent"]["matches"], json!([]));
    assert_eq!(nothing["structuredContent"]["exact"], false);
    assert!(
        text_of(&nothing).contains("Do not invent a citation"),
        "{}",
        text_of(&nothing)
    );
    let note = nothing["structuredContent"]["note"]
        .as_str()
        .expect("a note");
    assert!(note.contains("Do not invent a citation"), "{note}");

    let partial = session.call("find_citation", json!({ "query": "Knuth 2017" }));
    assert_eq!(partial["structuredContent"]["exact"], false);
    assert_eq!(
        partial["structuredContent"]["matches"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let note = partial["structuredContent"]["note"]
        .as_str()
        .expect("a note");
    assert!(note.contains("partial matches"), "{note}");

    let check = session.call("check_references", json!({}));
    assert_eq!(check["structuredContent"]["hasProblems"], false);
    assert_eq!(check["structuredContent"]["undefinedReferences"], json!([]));

    // Introduce a dangling reference and a made-up citation key.
    let chapter = project.path().join("chapters/conclusion.tex");
    let mut text = fs::read_to_string(&chapter).unwrap();
    text.push_str("\nSee Section~\\ref{sec:nowhere} and \\citep{madeup2024}.\n");
    fs::write(&chapter, text).unwrap();

    let check = session.call("check_references", json!({}));
    assert_eq!(
        check["isError"], false,
        "findings are a result, not a failure"
    );
    let structured = &check["structuredContent"];
    assert_eq!(structured["hasProblems"], true);
    assert_eq!(structured["undefinedReferences"][0]["name"], "sec:nowhere");
    assert_eq!(
        structured["undefinedReferences"][0]["file"],
        "chapters/conclusion.tex"
    );
    assert_eq!(structured["undefinedCitations"][0]["name"], "madeup2024");
    assert!(text_of(&check).contains("madeup2024 (chapters/conclusion.tex:"));

    session.finish();
}

#[test]
fn an_unsupported_protocol_version_is_refused_with_32022() {
    let state = TempDir::new("version-state");
    let mut session = Session::start(state.path(), state.path(), &[]);

    for method in ["server/discover", "tools/list"] {
        let response = session.versioned("1900-01-01", method, Value::Null);
        assert!(response.get("result").is_none(), "{response}");
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(response["error"]["message"], "Unsupported protocol version");
        assert_eq!(response["error"]["data"]["requested"], "1900-01-01");
        assert_eq!(
            response["error"]["data"]["supported"],
            json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"])
        );
    }
    // The client retries with a version from the list, and that works.
    let retry = session.versioned(MODERN, "tools/list", Value::Null);
    assert_eq!(tool_names(&retry["result"]), TOOLS);

    session.finish();
}

#[test]
fn a_request_with_no_version_and_no_handshake_is_served_anyway() {
    let state = TempDir::new("lenient-state");
    let mut session = Session::start(state.path(), state.path(), &[]);

    let list = session.request("tools/list", Value::Null);
    assert_eq!(tool_names(&list["result"]), TOOLS);
    assert_eq!(list["result"]["resultType"], "complete");

    let unknown = session.request("prompts/list", Value::Null);
    assert_eq!(unknown["error"]["code"], -32601);

    session.send(&json!("not an object"));
    assert_eq!(session.receive()["error"]["code"], -32600);
    let stdin_garbage = session.request("tools/call", json!({ "name": "no_such_tool" }));
    assert_eq!(stdin_garbage["error"]["code"], -32602);

    session.finish();
}

// ------------------------------------------------------------ legacy era

#[test]
fn legacy_initialize_then_list_and_call() {
    let project = copy_project(&example_thesis(), "legacy");
    let state = TempDir::new("legacy-state");
    let mut session = Session::start(project.path(), state.path(), &[]);

    let init = session.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "thelab-tests", "version": "0" }
        }),
    );
    let result = &init["result"];
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert_eq!(result["capabilities"], json!({ "tools": {} }));
    assert_eq!(result["serverInfo"]["name"], "thelab");
    assert!(
        result["instructions"]
            .as_str()
            .unwrap()
            .contains("find_citation")
    );

    // A notification gets no reply: the next line on stdout answers the ping.
    session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(session.request("ping", Value::Null)["result"], json!({}));

    let list = session.request("tools/list", Value::Null);
    assert_eq!(tool_names(&list["result"]), TOOLS);
    for modern_only in ["resultType", "ttlMs", "cacheScope", "_meta"] {
        assert!(
            list["result"].get(modern_only).is_none(),
            "legacy result has no {modern_only}"
        );
    }

    let found = session.request(
        "tools/call",
        json!({ "name": "find_citation", "arguments": { "query": "lamport" } }),
    );
    assert_eq!(found["result"]["isError"], false);
    assert_eq!(
        found["result"]["structuredContent"]["matches"][0]["key"],
        "lamport1994latex"
    );
    assert!(found["result"].get("resultType").is_none());

    session.finish();
}

#[test]
fn legacy_initialize_with_an_older_version_is_echoed() {
    let state = TempDir::new("legacy-old-state");
    let mut session = Session::start(state.path(), state.path(), &[]);
    let init = session.request(
        "initialize",
        json!({ "protocolVersion": "2025-03-26", "capabilities": {} }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    session.finish();
}

// ------------------------------------------------- editor state and roots

#[test]
fn editor_context_and_reveal_talk_to_the_app_through_files() {
    let project = copy_project(&example_thesis(), "editor");
    let state = TempDir::new("editor-state");
    let mut session = Session::start(project.path(), state.path(), &[]);

    let closed = session.call("editor_context", json!({}));
    assert_eq!(closed["isError"], false);
    assert_eq!(closed["structuredContent"]["running"], false);
    assert!(text_of(&closed).contains("not running"));
    let note = closed["structuredContent"]["note"]
        .as_str()
        .expect("a note");
    assert!(note.contains("not running"), "{note}");

    let unseen = session.call("reveal_in_editor", json!({ "file": "main.tex", "line": 3 }));
    assert_eq!(unseen["structuredContent"]["appRunning"], false);
    let note = unseen["structuredContent"]["note"]
        .as_str()
        .expect("a note");
    assert!(note.contains("nothing will happen"), "{note}");

    // What the app writes when the user selects a sentence.
    state.write(
        "editor-state.json",
        &json!({
            "projectRoot": project.path(),
            "file": "chapters/design.tex",
            "line": 9,
            "column": 1,
            "selection": { "text": "A snapshot records the engine state at a page boundary.",
                           "startLine": 9, "startColumn": 1, "endLine": 9, "endColumn": 56 },
            "updatedAt": "2026-10-11T18:03:07Z"
        })
        .to_string(),
    );
    let open = session.call("editor_context", json!({}));
    let structured = &open["structuredContent"];
    assert_eq!(structured["running"], true);
    assert_eq!(structured["file"], "chapters/design.tex");
    assert_eq!(structured["line"], 9);
    assert_eq!(
        structured["selectionText"],
        "A snapshot records the engine state at a page boundary."
    );
    let text = text_of(&open);
    assert!(text.contains("has chapters/design.tex open"), "{text}");
    assert!(text.contains("cursor at line 9, column 1"), "{text}");
    assert!(
        text.contains("Selected text:\nA snapshot records the engine state"),
        "{text}"
    );

    let reveal = session.call(
        "reveal_in_editor",
        json!({ "file": "chapters/evaluation.tex", "line": 21 }),
    );
    assert_eq!(reveal["isError"], false, "{reveal}");
    assert_eq!(reveal["structuredContent"]["appRunning"], true);
    let request: Value = serde_json::from_str(
        &fs::read_to_string(state.path().join("reveal-request.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        Path::new(request["file"].as_str().unwrap()),
        project.path().join("chapters/evaluation.tex")
    );
    assert_eq!(request["line"], 21);
    assert!(request["requestedAt"].as_str().unwrap().ends_with('Z'));

    let missing = session.call(
        "reveal_in_editor",
        json!({ "file": "chapters/nope.tex", "line": 1 }),
    );
    assert_eq!(missing["isError"], true);
    assert!(text_of(&missing).contains("does not exist"));
    let no_line = session.call("reveal_in_editor", json!({ "file": "main.tex" }));
    assert_eq!(no_line["isError"], true);
    assert!(text_of(&no_line).contains("`line` is required"));

    session.finish();
}

#[test]
fn the_project_is_the_root_argument_then_claudes_project_dir_then_the_working_directory() {
    let project = copy_project(&example_thesis(), "roots");
    let elsewhere = TempDir::new("roots-elsewhere");
    elsewhere.write(
        "main.tex",
        "\\documentclass{article}\n\\begin{document}\n\\section{Elsewhere}\n\\end{document}\n",
    );
    let state = TempDir::new("roots-state");

    // Started outside the project, as a user-scope server can be.
    let project_dir = project.path().to_str().unwrap();
    let mut session = Session::start(
        elsewhere.path(),
        state.path(),
        &[("CLAUDE_PROJECT_DIR", project_dir)],
    );
    let from_env = session.call("project_overview", json!({}));
    assert_eq!(from_env["structuredContent"]["root"], project_dir);

    let explicit = session.call("project_overview", json!({ "root": elsewhere.path() }));
    assert_eq!(
        explicit["structuredContent"]["outline"][0]["title"],
        "Elsewhere"
    );
    session.finish();

    let mut session = Session::start(elsewhere.path(), state.path(), &[]);
    let from_cwd = session.call("project_overview", json!({}));
    assert_eq!(
        from_cwd["structuredContent"]["outline"][0]["title"],
        "Elsewhere"
    );

    let empty = TempDir::new("roots-empty");
    let none = session.call("project_overview", json!({ "root": empty.path() }));
    assert_eq!(none["isError"], true);
    assert!(
        text_of(&none).contains("No main .tex file found"),
        "{}",
        text_of(&none)
    );
    session.finish();
}

// --------------------------------------------------- missing external tools

#[test]
fn missing_programs_are_reported_with_how_to_install_them() {
    let project = copy_project(&example_thesis(), "notools");
    let state = TempDir::new("notools-state");
    let empty = TempDir::new("notools-path");
    let tools_path = empty.path().to_str().unwrap();
    let mut session = Session::start(
        project.path(),
        state.path(),
        &[("THELAB_TOOLS_PATH", tools_path)],
    );

    let compiled = session.call("compile", json!({}));
    assert_eq!(compiled["isError"], true);
    let text = text_of(&compiled);
    assert!(text.contains("No TeX engine found"), "{text}");
    assert!(text.contains("brew install tectonic"), "{text}");

    let forced = session.call("compile", json!({ "engine": "latexmk" }));
    assert!(
        text_of(&forced).contains("latexmk is not installed"),
        "{}",
        text_of(&forced)
    );

    let before = session.call("render_page", json!({ "page": 1 }));
    assert_eq!(before["isError"], true);
    assert!(
        text_of(&before).contains("Call `compile` first"),
        "{}",
        text_of(&before)
    );

    // With a PDF in place, the missing renderer is what stops it.
    project.write("build/main.pdf", "%PDF-1.5\n");
    let rendered = session.call("render_page", json!({ "page": 1 }));
    assert_eq!(rendered["isError"], true);
    assert!(
        text_of(&rendered).contains("No PDF renderer found"),
        "{}",
        text_of(&rendered)
    );
    assert!(
        text_of(&rendered).contains("brew install poppler"),
        "{}",
        text_of(&rendered)
    );

    // Everything that only reads sources still works.
    let overview = session.call("project_overview", json!({}));
    assert_eq!(overview["isError"], false);

    session.finish();
}

#[test]
fn a_page_too_large_for_the_budget_is_rendered_at_a_lower_resolution() {
    if !have_engine() || !have("pdftoppm") {
        return;
    }
    let project = copy_project(&example_thesis(), "budget");
    let state = TempDir::new("budget-state");
    let mut session = Session::start(
        project.path(),
        state.path(),
        &[("THELAB_RENDER_MAX_BYTES", "30000")],
    );
    assert_eq!(
        session.call("compile", json!({}))["structuredContent"]["ok"],
        true
    );

    let page = session.call("render_page", json!({ "page": 4, "dpi": 150 }));
    assert_eq!(page["isError"], false, "{page}");
    let png = decode_base64(page["content"][0]["data"].as_str().unwrap());
    assert!(png.len() <= 30_000, "{} bytes", png.len());
    let caption = text_of(&page);
    assert!(
        caption.contains("Rendered below the requested 150 dpi"),
        "{caption}"
    );
    session.finish();
}

// ------------------------------------------------------------- tool groups

#[test]
fn tools_flag_limits_the_server_to_the_named_groups() {
    let project = copy_project(&example_thesis(), "groups");
    let state = TempDir::new("groups-state");

    let mut session = Session::start_with(&["--tools", "latex"], project.path(), state.path(), &[]);
    let list = session.modern("tools/list", Value::Null);
    assert_eq!(tool_names(&list["result"]), TOOLS[..5]);
    let discover = session.modern("server/discover", Value::Null);
    let instructions = discover["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("find_citation"), "{instructions}");
    assert!(!instructions.contains("editor_context"), "{instructions}");

    let found = session.call("find_citation", json!({ "query": "lamport" }));
    assert_eq!(
        found["structuredContent"]["matches"][0]["key"],
        "lamport1994latex"
    );
    let off = session.modern(
        "tools/call",
        json!({ "name": "reveal_in_editor", "arguments": {} }),
    );
    assert_eq!(off["error"]["code"], -32602);
    let message = off["error"]["message"].as_str().unwrap();
    assert!(message.contains("editor tools are turned off"), "{message}");
    session.finish();

    // `--tools=` works too, and a group without tools yet is accepted.
    let mut session = Session::start_with(
        &["--tools=editor,notebook"],
        project.path(),
        state.path(),
        &[],
    );
    let list = session.modern("tools/list", Value::Null);
    assert_eq!(tool_names(&list["result"]), TOOLS[5..]);
    session.finish();
}

#[test]
fn a_misspelled_group_stops_the_server_before_it_speaks() {
    let state = TempDir::new("badgroup-state");
    for (arguments, expected) in [
        (
            vec!["mcp", "--tools", "latx"],
            "unknown tool group \"latx\"",
        ),
        (vec!["mcp", "--tools"], "--tools needs a value"),
        (vec!["mcp", "--tools="], "at least one group"),
        (vec!["mcp", "extra"], "unexpected argument extra"),
    ] {
        let output = common::run_cli(&arguments, state.path());
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert_eq!(output.stdout, b"", "{arguments:?}: stdout stays clean");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{arguments:?}: {stderr}");
    }
}

#[test]
fn editor_context_reports_the_language_and_the_notebook_cell() {
    let project = copy_project(&example_thesis(), "language");
    let state = TempDir::new("language-state");
    let mut session = Session::start(project.path(), state.path(), &[]);

    // The app has not said what language the file is in: go by its name.
    state.write(
        "editor-state.json",
        &json!({ "file": "analysis/fit.py", "line": 3 }).to_string(),
    );
    let python = session.call("editor_context", json!({}));
    assert_eq!(python["structuredContent"]["language"], "python");
    assert!(
        text_of(&python).contains("has analysis/fit.py open, python"),
        "{}",
        text_of(&python)
    );

    // The app's own word wins, and a notebook says which cell.
    state.write(
        "editor-state.json",
        &json!({ "file": "notes.txt", "language": "markdown", "cellId": "a1b2c3" }).to_string(),
    );
    let cell = session.call("editor_context", json!({}));
    assert_eq!(cell["structuredContent"]["language"], "markdown");
    assert_eq!(cell["structuredContent"]["cellId"], "a1b2c3");
    assert!(
        text_of(&cell).contains("notebook cell a1b2c3"),
        "{}",
        text_of(&cell)
    );

    state.write(
        "editor-state.json",
        &json!({ "file": "LICENSE" }).to_string(),
    );
    let unknown = session.call("editor_context", json!({}));
    assert_eq!(unknown["structuredContent"]["language"], Value::Null);
    session.finish();
}
