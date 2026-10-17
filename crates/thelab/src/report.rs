//! Plain-text renderings of results, shared by the CLI and the MCP tools.

use std::fmt::Write as _;

use thelab_core::bib::SearchResult;
use thelab_core::check::CheckReport;
use thelab_core::compile::CompileResult;
use thelab_core::logparse::{BoxSummary, Diagnostic, Severity};
use thelab_core::outline::Occurrence;
use thelab_core::overview::Overview;
use thelab_core::python::{Interpreter, PythonMap, Source};

/// How many diagnostics of each kind to spell out before summarizing.
const SHOWN: usize = 20;

fn diagnostic_line(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let location = match (&diagnostic.file, diagnostic.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: "),
        (Some(file), None) => format!("{file}: "),
        _ => String::new(),
    };
    format!("{severity}: {location}{}", diagnostic.message)
}

fn list(out: &mut String, diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics.iter().take(SHOWN) {
        writeln!(out, "{}", diagnostic_line(diagnostic)).unwrap();
    }
    if diagnostics.len() > SHOWN {
        writeln!(out, "... and {} more", diagnostics.len() - SHOWN).unwrap();
    }
}

fn boxes_line(boxes: &BoxSummary) -> Option<String> {
    if boxes.total() == 0 {
        return None;
    }
    let mut parts = Vec::new();
    for (count, name) in [
        (boxes.overfull_hbox, "overfull hbox"),
        (boxes.underfull_hbox, "underfull hbox"),
        (boxes.overfull_vbox, "overfull vbox"),
        (boxes.underfull_vbox, "underfull vbox"),
    ] {
        if count > 0 {
            parts.push(format!(
                "{count} {name}{}",
                if count == 1 { "" } else { "es" }
            ));
        }
    }
    let mut line = format!("boxes: {}", parts.join(", "));
    if let Some(worst) = &boxes.worst_overfull {
        write!(line, " (widest overflow {}pt", worst.points).unwrap();
        if let (Some(file), Some(number)) = (&worst.file, worst.line) {
            write!(line, " at {file}:{number}").unwrap();
        }
        line.push(')');
    }
    Some(line)
}

pub fn compile_text(result: &CompileResult) -> String {
    let mut out = String::new();
    let seconds = result.duration_ms as f64 / 1000.0;
    let engine = result.engine.name();
    match (&result.pdf_path, result.ok) {
        (Some(pdf), true) => writeln!(out, "ok: {pdf} ({engine}, {seconds:.1} s)").unwrap(),
        (Some(pdf), false) => writeln!(
            out,
            "failed ({engine}, {seconds:.1} s); a PDF was still written: {pdf}"
        )
        .unwrap(),
        (None, _) => writeln!(out, "failed: no PDF ({engine}, {seconds:.1} s)").unwrap(),
    }
    list(&mut out, &result.errors);
    list(&mut out, &result.warnings);
    for (label, names) in [
        ("undefined references", &result.undefined_references),
        ("undefined citations", &result.undefined_citations),
        ("duplicate labels", &result.duplicate_labels),
    ] {
        if !names.is_empty() {
            writeln!(out, "{label}: {}", names.join(", ")).unwrap();
        }
    }
    if let Some(line) = boxes_line(&result.boxes) {
        writeln!(out, "{line}").unwrap();
    }
    if result.ok && result.warnings.is_empty() && result.boxes.total() == 0 {
        writeln!(out, "no errors, no warnings").unwrap();
    }
    out.trim_end().to_string()
}

pub fn overview_text(overview: &Overview) -> String {
    let mut out = String::new();
    writeln!(out, "project: {}", overview.root).unwrap();
    writeln!(out, "main file: {}", overview.main).unwrap();
    writeln!(
        out,
        "document files: {}",
        overview.document_files.join(", ")
    )
    .unwrap();
    let other: Vec<&str> = overview
        .tex_files
        .iter()
        .filter(|file| !overview.document_files.contains(file))
        .map(String::as_str)
        .collect();
    if !other.is_empty() {
        writeln!(out, "other .tex files: {}", other.join(", ")).unwrap();
    }
    if !overview.bib_files.is_empty() {
        writeln!(out, "bibliography: {}", overview.bib_files.join(", ")).unwrap();
    }
    let counts = &overview.counts;
    writeln!(
        out,
        "{} words, {} labels, {} references, {} distinct citations, {} bibliography entries",
        overview.total_words,
        counts.labels,
        counts.references,
        counts.citations,
        counts.bib_entries
    )
    .unwrap();

    if !overview.outline.is_empty() {
        writeln!(out, "\noutline:").unwrap();
        let top = overview.outline.iter().map(|h| h.level).min().unwrap();
        for heading in &overview.outline {
            let indent = "  ".repeat(heading.level as usize - top as usize);
            // Chapters (or top-level sections) show the words of everything
            // inside them; other headings have no total of their own.
            let words = overview
                .units
                .iter()
                .find(|u| u.file == heading.file && u.line == heading.line)
                .map_or(String::new(), |u| format!(", {} words", u.words));
            writeln!(
                out,
                "{indent}{} \"{}\" ({}:{}{words})",
                heading.level.name(),
                heading.title,
                heading.file,
                heading.line
            )
            .unwrap();
        }
    }
    out.trim_end().to_string()
}

fn occurrences(out: &mut String, title: &str, items: &[Occurrence]) {
    if items.is_empty() {
        return;
    }
    writeln!(out, "{title}:").unwrap();
    for item in items.iter().take(SHOWN * 2) {
        writeln!(out, "  {} ({}:{})", item.name, item.file, item.line).unwrap();
    }
    if items.len() > SHOWN * 2 {
        writeln!(out, "  ... and {} more", items.len() - SHOWN * 2).unwrap();
    }
}

pub fn check_text(report: &CheckReport) -> String {
    let mut out = String::new();
    if report.has_problems() {
        writeln!(out, "problems found").unwrap();
    } else {
        writeln!(out, "no problems: every reference and citation resolves").unwrap();
    }
    occurrences(
        &mut out,
        "references to undefined labels",
        &report.undefined_references,
    );
    occurrences(
        &mut out,
        "citations missing from the bibliography",
        &report.undefined_citations,
    );
    if !report.duplicate_labels.is_empty() {
        writeln!(out, "labels defined more than once:").unwrap();
        for duplicate in &report.duplicate_labels {
            let places: Vec<String> = duplicate
                .locations
                .iter()
                .map(|l| format!("{}:{}", l.file, l.line))
                .collect();
            writeln!(out, "  {} ({})", duplicate.name, places.join(", ")).unwrap();
        }
    }
    occurrences(
        &mut out,
        "\\input or \\include of missing files",
        &report.missing_inputs,
    );
    if !report.missing_bib_files.is_empty() {
        writeln!(
            out,
            "missing .bib files: {}",
            report.missing_bib_files.join(", ")
        )
        .unwrap();
    }
    occurrences(
        &mut out,
        "labels never referenced (often fine)",
        &report.unused_labels,
    );
    occurrences(
        &mut out,
        "bibliography entries never cited (often fine)",
        &report.uncited_entries,
    );
    let counts = &report.counts;
    writeln!(
        out,
        "{} labels, {} references, {} distinct citations, {} bibliography entries",
        counts.labels, counts.references, counts.citations, counts.bib_entries
    )
    .unwrap();
    out.trim_end().to_string()
}

/// What the model must be told when a search does not cleanly succeed.
pub fn citation_note(result: &SearchResult) -> Option<&'static str> {
    if result.matches.is_empty() {
        Some(
            "No bibliography entry matches. Do not invent a citation: tell the user that the \
             source is missing so they can add it.",
        )
    } else if !result.exact {
        Some(
            "No entry contains every search term. These are the closest partial matches and may \
             not be the source you mean; check before citing.",
        )
    } else {
        None
    }
}

pub fn citations_text(query: &str, result: &SearchResult) -> String {
    let mut out = String::new();
    match citation_note(result) {
        Some(note) => writeln!(out, "Searched for \"{query}\". {note}").unwrap(),
        None => {
            let n = result.matches.len();
            let noun = if n == 1 { "match" } else { "matches" };
            writeln!(out, "{n} {noun} for \"{query}\":").unwrap();
        }
    }
    for found in &result.matches {
        let entry = found.entry;
        writeln!(
            out,
            "- {}: {} [{}, {}:{}]",
            entry.key,
            entry.summary(),
            entry.kind,
            entry.file,
            entry.line
        )
        .unwrap();
    }
    out.trim_end().to_string()
}

/// One line per interpreter, then any notes.
pub fn python_text(map: &PythonMap) -> String {
    let mut out = String::new();
    writeln!(out, "{}", interpreter_line("default", &map.default)).unwrap();
    for entry in &map.folders {
        writeln!(
            out,
            "{}",
            interpreter_line(&entry.folder, &entry.interpreter)
        )
        .unwrap();
    }
    for note in &map.notes {
        writeln!(out, "note: {note}").unwrap();
    }
    out.trim_end().to_string()
}

fn interpreter_line(label: &str, interpreter: &Interpreter) -> String {
    if let Some(problem) = &interpreter.problem {
        return format!("{label}: problem: {problem}");
    }
    let path = interpreter.path.as_deref().unwrap_or("?");
    let version = interpreter.version.as_deref().unwrap_or("?");
    let mut origin = match interpreter.source {
        Source::Config => format!(
            "{} in thelab.toml",
            interpreter.configured.as_deref().unwrap_or("set")
        ),
        Source::Venv => {
            let venv = std::path::Path::new(path)
                .parent()
                .and_then(|bin| bin.parent())
                .and_then(|venv| venv.file_name())
                .map_or("a venv".into(), |name| name.to_string_lossy());
            format!("{venv} in the project")
        }
        Source::Path => "python3 on PATH".to_string(),
    };
    if interpreter.source != Source::Venv
        && let Some(venv) = &interpreter.virtual_env
    {
        write!(origin, "; venv {venv}").unwrap();
    }
    format!("{label}: Python {version}, {path} ({origin})")
}
