//! The tools the MCP server offers. Each one returns text written for the
//! model plus the same information as structured JSON.
//!
//! Claude Code gives the model the structured JSON when a result has one,
//! and the text otherwise. So anything the model must be told (above all,
//! not to invent a citation) goes into the JSON as a `note` as well.
//!
//! There are deliberately no tools that edit files: Claude Code edits files
//! itself, and the app picks the changes up from the filesystem.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use thelab_core::compile::{CompileOptions, Engine, compile, pdf_path};
use thelab_core::render::{DEFAULT_DPI, MIN_DPI, RenderedPage, page_count, render_page};
use thelab_core::scan::{Project, language_of, scan};
use thelab_core::{bib, check, outline, overview, state};

use crate::report;

/// Largest PNG (in bytes, before base64) that `render_page` returns; bigger
/// pages are rendered again at a lower resolution. Claude Code caps MCP tool
/// output at 25,000 tokens by default but does not count an image by its
/// base64 length: version 2.1.216 passed a 168 KB page through to the model
/// unchanged. A dense page of text at the default 110 dpi is about 120 KB.
const IMAGE_BUDGET_BYTES: usize = 160 * 1024;
/// Overrides `IMAGE_BUDGET_BYTES`.
const IMAGE_BUDGET_VARIABLE: &str = "THELAB_RENDER_MAX_BYTES";

/// Diagnostics of each kind included in a tool result.
const MAX_DIAGNOSTICS: usize = 50;

/// Tools come in groups, so that a session can load only the ones it needs
/// (`thelab mcp --tools latex,editor`). A short tool list helps the model
/// pick the right tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Latex,
    Notebook,
    Sysml,
    Editor,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Latex, Group::Notebook, Group::Sysml, Group::Editor];

    pub fn name(self) -> &'static str {
        match self {
            Group::Latex => "latex",
            Group::Notebook => "notebook",
            Group::Sysml => "sysml",
            Group::Editor => "editor",
        }
    }

    /// The group a tool belongs to. New tools carry their group as a prefix
    /// (`notebook_*`, `sysml_*`); the first seven keep the names they had.
    pub fn of_tool(name: &str) -> Option<Group> {
        Some(match name {
            "project_overview" | "compile" | "check_references" | "find_citation"
            | "render_page" => Group::Latex,
            "editor_context" | "reveal_in_editor" => Group::Editor,
            _ => return None,
        })
    }

    /// Whether this version of the server has any tools in the group.
    pub fn has_tools(self) -> bool {
        matches!(self, Group::Latex | Group::Editor)
    }
}

/// The groups a server was started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Groups(Vec<Group>);

impl Groups {
    /// Every group: the default.
    pub fn all() -> Self {
        Self(Group::ALL.to_vec())
    }

    /// Parses the value of `--tools`, a comma-separated list of group names.
    pub fn parse(list: &str) -> Result<Self, String> {
        let known: Vec<&str> = Group::ALL.iter().map(|g| g.name()).collect();
        let mut groups = Vec::new();
        for name in list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let lower = name.to_ascii_lowercase();
            let group = Group::ALL
                .into_iter()
                .find(|g| g.name() == lower)
                .ok_or_else(|| {
                    format!(
                        "unknown tool group \"{name}\" (the groups are {})",
                        known.join(", ")
                    )
                })?;
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        if groups.is_empty() {
            return Err(format!(
                "--tools needs at least one group ({})",
                known.join(", ")
            ));
        }
        Ok(Self(groups))
    }

    pub fn contains(&self, group: Group) -> bool {
        self.0.contains(&group)
    }

    /// Requested groups that have no tools in this version.
    pub fn empty_ones(&self) -> Vec<Group> {
        self.0
            .iter()
            .copied()
            .filter(|group| !group.has_tools())
            .collect()
    }
}

/// Guidance shown to the model, covering only the groups that are on.
/// Claude Code truncates it at 2,048 characters, so the rules that matter
/// most come first.
pub fn instructions(groups: &Groups) -> String {
    let mut text = String::from(
        "The Lab gives you tools for the document project you are working in. Edit files with \
         your normal file tools; this server does not edit.",
    );
    if groups.contains(Group::Latex) {
        text.push_str(
            "\n\nLaTeX: use these whenever you read or edit .tex or .bib files.\n\
             - After editing any .tex or .bib file, call `compile` and fix every error before you \
             say the work is done.\n\
             - Before you write any \\cite, call `find_citation` and use only a key it returns. \
             Never invent a bibliography entry or guess a key. If a source is missing, tell the \
             user.\n\
             - In a project you do not know yet, call `project_overview` first.\n\
             - After compiling, use `render_page` to look at figures, tables and slides.",
        );
    }
    if groups.contains(Group::Editor) {
        text.push_str(
            "\n\nEditor: `editor_context` says what the user has open and selected in The \
             Lab, and `reveal_in_editor` shows them a place in a file.",
        );
    }
    text
}

pub struct Output {
    pub content: Vec<Value>,
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl Output {
    fn text(text: impl Into<String>, structured: Value) -> Self {
        Self {
            content: vec![text_block(text)],
            structured: Some(structured),
            is_error: false,
        }
    }

    /// A failure the model can act on, reported as a tool result.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![text_block(message)],
            structured: None,
            is_error: true,
        }
    }
}

fn text_block(text: impl Into<String>) -> Value {
    json!({ "type": "text", "text": text.into() })
}

/// The `tools/list` entries for the given groups, in a fixed order so that
/// clients can cache them.
pub fn definitions(groups: &Groups) -> Value {
    let mut tools = all_definitions();
    if let Some(list) = tools.as_array_mut() {
        list.retain(|tool| {
            tool["name"]
                .as_str()
                .and_then(Group::of_tool)
                .is_some_and(|group| groups.contains(group))
        });
    }
    tools
}

fn all_definitions() -> Value {
    let root = json!({
        "type": "string",
        "description": "Project folder. Defaults to the folder Claude Code was started in; \
                        only needed for a project somewhere else."
    });
    let main = json!({
        "type": "string",
        "description": "Main .tex file relative to the project folder, for example \
                        slides/slides.tex. Defaults to the detected main file."
    });
    let read_only = json!({
        "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false
    });

    json!([
        {
            "name": "project_overview",
            "title": "Project overview",
            "description": "Get oriented in a LaTeX project in one call: the main file, every \
                .tex and .bib file, the outline (parts, chapters, sections, each with file:line), \
                word counts per chapter, and how many labels, citations and bibliography entries \
                there are. Call this first in a project you have not seen, and whenever you need \
                to find where a chapter or section lives. Reads files only; does not compile.",
            "inputSchema": {
                "type": "object",
                "properties": { "root": root, "main": main },
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "compile",
            "title": "Compile LaTeX",
            "description": "Compile the LaTeX project and report what the engine said. Run this \
                after editing any .tex or .bib file, and fix every error before reporting the \
                work as complete. Returns whether the build succeeded, the PDF path, errors and \
                warnings with file and line, undefined references and citations, duplicate \
                labels, and a count of overfull and underfull boxes. Output goes to the \
                project's build/ folder, which The Lab previews. Uses Tectonic if \
                installed, otherwise latexmk. The very first compile can take a minute while \
                Tectonic downloads packages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": root,
                    "main": main,
                    "engine": {
                        "type": "string",
                        "enum": ["tectonic", "latexmk"],
                        "description": "Engine to use. Leave unset unless the default fails."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true
            }
        },
        {
            "name": "check_references",
            "title": "Check references and citations",
            "description": "Check cross-references and citations without compiling, which is \
                fast. Finds \\ref to labels that do not exist, \\cite keys missing from the .bib \
                files, labels defined twice and \\input of missing files; also lists unused \
                labels and uncited bibliography entries for information. Good for a quick check \
                while editing. `compile` remains the final word: labels created by unusual \
                packages can show up here as false alarms.",
            "inputSchema": {
                "type": "object",
                "properties": { "root": root, "main": main },
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "find_citation",
            "title": "Find a citation",
            "description": "Search the project's bibliography (.bib files) by author, title \
                words, year or key, and get the matching entries with their citation keys. Call \
                this before writing any \\cite, \\citep or \\citet, and cite only keys it \
                returns. If nothing matches, the source is not in the bibliography: tell the \
                user instead of inventing an entry or guessing a key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you know about the source, for example \"Knuth 1984\" \
                                        or \"breaking paragraphs into lines\". Every word must match."
                    },
                    "root": root,
                    "main": main
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "render_page",
            "title": "Render a PDF page",
            "description": "Render one page of the compiled PDF as a PNG image so that you can \
                see the layout: figures, tables, equations, slides. Shows the PDF from the last \
                `compile`, so compile first. Pages are counted from 1 in the PDF, which can \
                differ from the numbers printed on the pages. Set `main` to look at another \
                document in the project, such as slides/slides.tex.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "page": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Page of the PDF, starting at 1."
                    },
                    "dpi": {
                        "type": "integer",
                        "minimum": 36,
                        "maximum": 300,
                        "description": "Resolution. Defaults to 110, which is enough to read body \
                                        text. Large images are scaled down to stay within the \
                                        output limit."
                    },
                    "root": root,
                    "main": main
                },
                "required": ["page"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "editor_context",
            "title": "Editor context",
            "description": "Find out what the user is looking at in The Lab: the open \
                file, the cursor line and column, and the selected text. Use it when the user \
                says things like \"this paragraph\", \"what I selected\" or \"here\". Says so if \
                the app is not running.",
            "inputSchema": { "type": "object", "additionalProperties": false },
            "annotations": read_only
        },
        {
            "name": "reveal_in_editor",
            "title": "Reveal in editor",
            "description": "Scroll The Lab to a line of a file, to show the user where \
                something is: an error, a paragraph you changed, a label. Has no effect unless \
                the app is running.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "File to show, relative to the project folder or absolute."
                    },
                    "line": { "type": "integer", "minimum": 1, "description": "Line to scroll to." },
                    "root": root
                },
                "required": ["file", "line"],
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false
            }
        }
    ])
}

/// Runs a tool. `None` means there is no tool of that name.
pub fn call(name: &str, arguments: &Value) -> Option<Output> {
    let arguments = Arguments(arguments);
    let outcome = match name {
        "project_overview" => project_overview(&arguments),
        "compile" => run_compile(&arguments),
        "check_references" => check_references(&arguments),
        "find_citation" => find_citation(&arguments),
        "render_page" => render(&arguments),
        "editor_context" => editor_context(),
        "reveal_in_editor" => reveal_in_editor(&arguments),
        _ => return None,
    };
    Some(outcome.unwrap_or_else(Output::error))
}

/// Tool arguments, with errors phrased for the model to correct.
struct Arguments<'a>(&'a Value);

impl Arguments<'_> {
    fn string(&self, name: &str) -> Result<Option<&str>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.as_str()).filter(|t| !t.trim().is_empty())),
            Some(_) => Err(format!("`{name}` must be a string")),
        }
    }

    fn required_string(&self, name: &str) -> Result<&str, String> {
        self.string(name)?
            .ok_or_else(|| format!("`{name}` is required"))
    }

    /// A whole number. Numeric strings are accepted, since models sometimes
    /// quote numbers.
    fn integer(&self, name: &str) -> Result<Option<u32>, String> {
        let invalid = || format!("`{name}` must be a positive whole number");
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => match number.as_u64() {
                Some(whole) => u32::try_from(whole).map(Some).map_err(|_| invalid()),
                None => match number.as_f64() {
                    Some(float)
                        if float >= 0.0 && float.fract() == 0.0 && float <= f64::from(u32::MAX) =>
                    {
                        Ok(Some(float as u32))
                    }
                    _ => Err(invalid()),
                },
            },
            Some(Value::String(text)) => text.trim().parse().map(Some).map_err(|_| invalid()),
            Some(_) => Err(invalid()),
        }
    }

    /// The project folder: the `root` argument, else the folder Claude Code
    /// says it is working in, else the server's working directory.
    fn root(&self) -> Result<PathBuf, String> {
        if let Some(root) = self.string("root")? {
            return Ok(expand_home(root));
        }
        Ok(
            match std::env::var_os("CLAUDE_PROJECT_DIR").filter(|dir| !dir.is_empty()) {
                Some(dir) => PathBuf::from(dir),
                None => PathBuf::from("."),
            },
        )
    }

    fn project(&self) -> Result<Project, String> {
        scan(&self.root()?, self.string("main")?).map_err(|e| e.to_string())
    }
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn message(error: thelab_core::Error) -> String {
    error.to_string()
}

fn project_overview(arguments: &Arguments) -> Result<Output, String> {
    let overview = overview::overview(&arguments.project()?).map_err(message)?;
    Ok(Output::text(
        report::overview_text(&overview),
        json!(overview),
    ))
}

fn run_compile(arguments: &Arguments) -> Result<Output, String> {
    let engine = arguments
        .string("engine")?
        .map(Engine::parse)
        .transpose()
        .map_err(message)?;
    let options = CompileOptions {
        main: arguments.string("main")?.map(str::to_string),
        engine,
    };
    let result = compile(&arguments.root()?, &options).map_err(message)?;

    let mut structured = json!(result);
    for (list, count) in [("errors", "errorCount"), ("warnings", "warningCount")] {
        let total = structured[list].as_array().map_or(0, Vec::len);
        structured[count] = json!(total);
        if let Some(items) = structured[list].as_array_mut() {
            items.truncate(MAX_DIAGNOSTICS);
        }
    }
    let mut output = Output::text(report::compile_text(&result), structured);
    // A document that does not build is a failure the model must act on.
    output.is_error = !result.ok;
    Ok(output)
}

fn check_references(arguments: &Arguments) -> Result<Output, String> {
    let project = arguments.project()?;
    let outline = outline::read_outline(&project).map_err(message)?;
    let report = check::check(&project, &outline).map_err(message)?;
    let mut structured = json!(report);
    structured["hasProblems"] = json!(report.has_problems());
    Ok(Output::text(report::check_text(&report), structured))
}

fn find_citation(arguments: &Arguments) -> Result<Output, String> {
    let query = arguments.required_string("query")?;
    let project = arguments.project()?;
    let outline = outline::read_outline(&project).map_err(message)?;
    let files = check::bib_files(&project, &outline);
    if files.is_empty() {
        return Err(format!(
            "This project has no .bib file, so there is nothing to cite from. \
             Tell the user; do not make up a citation for \"{query}\"."
        ));
    }
    let entries = check::bibliography(&project, &outline).map_err(message)?;
    let found = bib::search(&entries, query, 10);

    let matches: Vec<Value> = found
        .matches
        .iter()
        .map(|m| {
            let mut entry = json!(m.entry);
            entry["matchedTerms"] = json!(m.matched_terms);
            entry
        })
        .collect();
    let mut structured = json!({
        "query": query,
        "terms": found.terms,
        "exact": found.exact && !found.matches.is_empty(),
        "bibFiles": files,
        "matches": matches,
    });
    if let Some(note) = report::citation_note(&found) {
        structured["note"] = json!(note);
    }
    Ok(Output::text(
        report::citations_text(query, &found),
        structured,
    ))
}

fn render(arguments: &Arguments) -> Result<Output, String> {
    let page = arguments.integer("page")?.ok_or("`page` is required")?;
    let requested_dpi = arguments.integer("dpi")?.unwrap_or(DEFAULT_DPI);
    let project = arguments.project()?;
    let pdf = pdf_path(&project).map_err(message)?;
    if !pdf.is_file() {
        return Err(format!(
            "There is no compiled PDF yet ({} does not exist). Call `compile` first.",
            pdf.display()
        ));
    }

    let rendered = render_within_budget(&pdf, page, requested_dpi).map_err(message)?;
    let mut caption = match page_count(&pdf) {
        Some(pages) => format!("Page {page} of {pages}"),
        None => format!("Page {page}"),
    };
    caption.push_str(&format!(
        " of {} at {} dpi ({}x{} px).",
        pdf.display(),
        rendered.dpi,
        rendered.width,
        rendered.height
    ));
    if rendered.dpi < requested_dpi.clamp(MIN_DPI, thelab_core::render::MAX_DPI) {
        caption.push_str(&format!(
            " Rendered below the requested {requested_dpi} dpi to keep the image within the output limit."
        ));
    }

    // No structuredContent here: the image is the result, and a client that
    // prefers structured content over content blocks must not lose it.
    Ok(Output {
        content: vec![
            json!({ "type": "image", "data": base64(&rendered.png), "mimeType": "image/png" }),
            text_block(caption),
        ],
        structured: None,
        is_error: false,
    })
}

/// Renders a page, lowering the resolution until the PNG fits the budget.
fn render_within_budget(pdf: &Path, page: u32, dpi: u32) -> thelab_core::Result<RenderedPage> {
    let budget = std::env::var(IMAGE_BUDGET_VARIABLE)
        .ok()
        .and_then(|bytes| bytes.parse().ok())
        .unwrap_or(IMAGE_BUDGET_BYTES);
    let mut rendered = render_page(pdf, page, dpi)?;
    for _ in 0..3 {
        if rendered.png.len() <= budget || rendered.dpi <= MIN_DPI {
            break;
        }
        // PNG size grows roughly with the pixel count, the square of the dpi.
        let scale = (budget as f64 / rendered.png.len() as f64).sqrt() * 0.9;
        let lower = ((f64::from(rendered.dpi) * scale) as u32).clamp(MIN_DPI, rendered.dpi - 1);
        rendered = render_page(pdf, page, lower)?;
    }
    Ok(rendered)
}

fn editor_context() -> Result<Output, String> {
    let state_file = state::state_dir().join(state::EDITOR_STATE_FILE);
    let Some(editor) = state::read_editor_state().map_err(message)? else {
        let note = "The Lab is not running, so there is no editor context. \
                    Ask the user which file and lines they mean.";
        return Ok(Output::text(
            note,
            json!({ "running": false, "note": note, "stateFile": state_file.display().to_string() }),
        ));
    };

    // An app that does not say which language the file is in still names
    // the file, and that is usually enough.
    let language = editor.language.clone().or_else(|| {
        editor
            .file
            .as_deref()
            .and_then(language_of)
            .map(str::to_string)
    });
    let kind = language
        .as_ref()
        .map_or(String::new(), |l| format!(", {l}"));

    let mut text = String::from("The Lab");
    match (&editor.file, &editor.project_root) {
        (Some(file), Some(root)) => {
            text.push_str(&format!(" has {file} open (project {root}{kind})"))
        }
        (Some(file), None) => text.push_str(&format!(" has {file} open{kind}")),
        (None, Some(root)) => {
            text.push_str(&format!(" has project {root} open with no file selected"))
        }
        (None, None) => text.push_str(" is running with nothing open"),
    }
    match (editor.line, editor.column) {
        (Some(line), Some(column)) => {
            text.push_str(&format!(", cursor at line {line}, column {column}."))
        }
        (Some(line), None) => text.push_str(&format!(", cursor at line {line}.")),
        _ => text.push('.'),
    }
    if let Some(cell) = &editor.cell_id {
        text.push_str(&format!(" The cursor is in notebook cell {cell}."));
    }
    match editor.selection_text() {
        Some(selection) => text.push_str(&format!("\nSelected text:\n{selection}")),
        None => text.push_str("\nNothing is selected."),
    }
    if let Some(updated) = &editor.updated_at {
        text.push_str(&format!("\n(Last updated {updated}.)"));
    }

    let selection_text = editor.selection_text().map(str::to_string);
    let mut structured = json!(editor);
    structured["language"] = json!(language);
    structured["running"] = json!(true);
    structured["selectionText"] = json!(selection_text);
    Ok(Output::text(text, structured))
}

fn reveal_in_editor(arguments: &Arguments) -> Result<Output, String> {
    let file = arguments.required_string("file")?;
    let line = arguments.integer("line")?.ok_or("`line` is required")?;
    if line == 0 {
        return Err("`line` starts at 1".to_string());
    }

    let given = expand_home(file);
    let path = if given.is_absolute() {
        given
    } else {
        arguments.root()?.join(given)
    };
    let path = std::fs::canonicalize(&path)
        .map_err(|_| format!("{} does not exist, so it cannot be shown.", path.display()))?;

    let request_file = state::write_reveal_request(&path, line).map_err(message)?;
    let running = state::read_editor_state().ok().flatten().is_some();
    let mut text = format!("Asked The Lab to show {} at line {line}.", path.display());
    let mut structured = json!({
        "file": path.display().to_string(),
        "line": line,
        "appRunning": running,
        "requestFile": request_file.display().to_string(),
    });
    if !running {
        let note = "The Lab does not appear to be running, so nothing will happen until it starts.";
        text.push(' ');
        text.push_str(note);
        structured["note"] = json!(note);
    }
    Ok(Output::text(text, structured))
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(ALPHABET[(group >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_examples() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn tool_names_are_unique_and_schemas_are_objects() {
        let definitions = definitions(&Groups::all());
        let tools = definitions.as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "project_overview",
                "compile",
                "check_references",
                "find_citation",
                "render_page",
                "editor_context",
                "reveal_in_editor",
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
            assert!(!tool["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn integers_are_read_leniently_but_not_sloppily() {
        let arguments =
            json!({ "a": 3, "b": "4", "c": 5.0, "d": 2.5, "e": -1, "f": "x", "g": null });
        let arguments = Arguments(&arguments);
        assert_eq!(arguments.integer("a"), Ok(Some(3)));
        assert_eq!(arguments.integer("b"), Ok(Some(4)));
        assert_eq!(arguments.integer("c"), Ok(Some(5)));
        assert!(arguments.integer("d").is_err());
        assert!(arguments.integer("e").is_err());
        assert!(arguments.integer("f").is_err());
        assert_eq!(arguments.integer("g"), Ok(None));
        assert_eq!(arguments.integer("missing"), Ok(None));
    }

    #[test]
    fn every_tool_belongs_to_a_group_and_groups_filter_the_list() {
        let names = |groups: &Groups| -> Vec<String> {
            definitions(groups)
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_string())
                .collect()
        };
        for tool in all_definitions().as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert!(Group::of_tool(name).is_some(), "{name} has no group");
        }
        assert_eq!(names(&Groups::all()).len(), 7);
        assert_eq!(
            names(&Groups::parse("editor").unwrap()),
            ["editor_context", "reveal_in_editor"]
        );
        // The order of the list does not depend on the order of the flag.
        assert_eq!(
            names(&Groups::parse("editor, LaTeX").unwrap()),
            names(&Groups::all())
        );
        assert_eq!(
            names(&Groups::parse("notebook,sysml").unwrap()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn group_lists_are_checked() {
        assert_eq!(Groups::parse("latex,latex"), Groups::parse("latex"));
        let unknown = Groups::parse("latex,latx").unwrap_err();
        assert!(unknown.contains("unknown tool group \"latx\""), "{unknown}");
        assert!(
            unknown.contains("latex, notebook, sysml, editor"),
            "{unknown}"
        );
        assert!(
            Groups::parse(" , ")
                .unwrap_err()
                .contains("at least one group")
        );
        assert_eq!(
            Groups::parse("notebook,editor").unwrap().empty_ones(),
            [Group::Notebook]
        );
    }

    #[test]
    fn instructions_cover_only_the_groups_that_are_on() {
        let all = instructions(&Groups::all());
        assert!(all.contains("find_citation") && all.contains("editor_context"));
        let latex = instructions(&Groups::parse("latex").unwrap());
        assert!(latex.contains("Never invent") && !latex.contains("editor_context"));
        let editor = instructions(&Groups::parse("editor").unwrap());
        assert!(editor.contains("reveal_in_editor") && !editor.contains("compile"));
    }
}
