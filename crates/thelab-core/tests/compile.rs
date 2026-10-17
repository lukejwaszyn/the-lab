//! End-to-end compiles with the real engines. Each test skips when its
//! engine is not installed (see `common::have`).

mod common;

use std::path::Path;

use common::{copy_project, example_thesis, fixture_project, have};
use thelab_core::compile::{CompileOptions, CompileResult, Engine, compile};
use thelab_core::logparse::{Diagnostic, Severity};
use thelab_core::render::render_page;

const ENGINES: [Engine; 2] = [Engine::Tectonic, Engine::Latexmk];

fn build(source: &Path, engine: Engine, main: Option<&str>) -> (common::TempDir, CompileResult) {
    let dir = copy_project(source, engine.name());
    let options = CompileOptions {
        main: main.map(str::to_string),
        engine: Some(engine),
    };
    let result = compile(dir.path(), &options).expect("compile could be attempted");
    (dir, result)
}

fn error(file: &str, line: Option<u32>, message: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        file: Some(file.to_string()),
        line,
        message: message.to_string(),
    }
}

#[test]
fn the_example_thesis_compiles_cleanly() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (dir, result) = build(&example_thesis(), engine, None);
        assert!(result.ok, "{engine:?}: {result:#?}");
        assert_eq!(result.main, "main.tex");
        assert_eq!(result.errors, [], "{engine:?}");
        assert_eq!(result.warnings, [], "{engine:?}");
        assert!(result.undefined_references.is_empty(), "{engine:?}");
        assert!(result.undefined_citations.is_empty(), "{engine:?}");

        let pdf = result.pdf_path.expect("a PDF path");
        assert!(Path::new(&pdf).is_file());
        assert!(pdf.ends_with("build/main.pdf"), "{pdf}");
        assert!(
            dir.path().join("build/main.synctex.gz").is_file(),
            "{engine:?}: SyncTeX data"
        );
        assert!(result.log_path.is_some());
    }
}

#[test]
fn an_error_in_an_included_file_is_reported_with_file_line_and_message() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (_dir, result) = build(&fixture_project("undefined-cs"), engine, None);
        assert!(!result.ok, "{engine:?}");
        assert_eq!(
            result.errors,
            [error(
                "chapters/one.tex",
                Some(5),
                "Undefined control sequence"
            )],
            "{engine:?}"
        );
    }
}

#[test]
fn a_missing_input_is_reported_at_the_input_command() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (_dir, result) = build(&fixture_project("missing-file"), engine, None);
        assert!(!result.ok, "{engine:?}");
        assert_eq!(
            result.errors,
            [error(
                "main.tex",
                Some(4),
                "LaTeX Error: File `chapters/nope.tex' not found"
            )],
            "{engine:?}"
        );
        assert_eq!(
            result.pdf_path, None,
            "{engine:?}: no PDF from a fatal error"
        );
    }
}

#[test]
fn an_error_without_a_location_falls_back_to_the_main_file() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (_dir, result) = build(&fixture_project("runaway"), engine, None);
        assert!(!result.ok, "{engine:?}");
        assert_eq!(
            result.errors,
            [error(
                "main.tex",
                None,
                r"File ended while scanning use of \@xdblarg"
            )],
            "{engine:?}"
        );
    }
}

#[test]
fn warnings_do_not_fail_the_build_and_are_summarized() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (_dir, result) = build(&fixture_project("warnings"), engine, None);
        assert!(result.ok, "{engine:?}: {result:#?}");
        assert_eq!(result.undefined_references, ["sec:missing"], "{engine:?}");
        assert_eq!(result.undefined_citations, ["nokey2020"], "{engine:?}");
        assert_eq!(result.duplicate_labels, ["chap:dup"], "{engine:?}");
        assert_eq!(result.boxes.overfull_hbox, 1, "{engine:?}");
        assert_eq!(result.boxes.underfull_vbox, 1, "{engine:?}");

        let located: Vec<(Option<&str>, Option<u32>)> = result
            .warnings
            .iter()
            .filter(|w| w.message.contains("undefined"))
            .map(|w| (w.file.as_deref(), w.line))
            .collect();
        assert_eq!(
            located,
            [(Some("chapters/body.tex"), Some(4)); 2],
            "{engine:?}"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("database entry for \"nokey2020\"")),
            "{engine:?}: BibTeX's own warning is included: {:#?}",
            result.warnings
        );
    }
}

#[test]
fn a_failed_build_does_not_report_references_it_never_resolved() {
    // The example thesis has dozens of references and citations, all fine.
    // An error stops Tectonic in its first pass, where none of them is
    // resolved yet, and the log is full of "undefined" warnings.
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let dir = copy_project(&example_thesis(), engine.name());
        let chapter = dir.path().join("chapters/introduction.tex");
        let text = std::fs::read_to_string(&chapter).unwrap();
        std::fs::write(
            &chapter,
            text.replacen("Writing a long", "\\undefinedmacro Writing a long", 1),
        )
        .unwrap();

        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let result = compile(dir.path(), &options).unwrap();
        assert!(!result.ok, "{engine:?}");
        assert_eq!(
            result.errors,
            [error(
                "chapters/introduction.tex",
                Some(4),
                "Undefined control sequence"
            )],
            "{engine:?}"
        );
        assert_eq!(
            result.undefined_references,
            Vec::<String>::new(),
            "{engine:?}"
        );
        assert_eq!(
            result.undefined_citations,
            Vec::<String>::new(),
            "{engine:?}"
        );
        assert_eq!(result.warnings, [], "{engine:?}");
    }
}

#[test]
fn compiling_again_without_changes_gives_the_same_answer() {
    // latexmk does nothing when no source has changed. The PDF and the log
    // it leaves alone are still the result.
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (dir, first) = build(&fixture_project("warnings"), engine, None);
        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let second = compile(dir.path(), &options).unwrap();
        assert!(second.ok, "{engine:?}: {second:#?}");
        assert_eq!(second.pdf_path, first.pdf_path, "{engine:?}");
        assert_eq!(second.warnings, first.warnings, "{engine:?}");
        assert_eq!(second.undefined_references, ["sec:missing"], "{engine:?}");
        assert_eq!(second.boxes, first.boxes, "{engine:?}");
    }
}

#[test]
fn a_failed_build_compiled_again_reports_the_same_errors() {
    // After a failure latexmk does not run pdflatex again until a source
    // changes. It exits with "gave an error in previous invocation", and the
    // log from that invocation is still the place to find out why.
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (dir, first) = build(&fixture_project("undefined-cs"), engine, None);
        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let second = compile(dir.path(), &options).unwrap();
        assert!(!second.ok, "{engine:?}");
        assert_eq!(second.errors, first.errors, "{engine:?}");
        // pdflatex continues past this error and writes a PDF, which the
        // app shows beside the error. Unchanged sources must show it again.
        assert_eq!(second.pdf_path, first.pdf_path, "{engine:?}");
        assert_eq!(second.warnings, first.warnings, "{engine:?}");
        assert_eq!(
            second.errors,
            [error(
                "chapters/one.tex",
                Some(5),
                "Undefined control sequence"
            )],
            "{engine:?}"
        );
    }
}

#[test]
fn a_failed_build_does_not_pass_off_the_previous_pdf_as_its_own() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (dir, first) = build(&example_thesis(), engine, None);
        assert!(first.ok, "{engine:?}");
        let pdf = dir.path().join("build/main.pdf");
        let before = std::fs::read(&pdf).unwrap();

        // Straight away, so that only a rewrite can tell the runs apart.
        let chapter = dir.path().join("chapters/conclusion.tex");
        let mut text = std::fs::read_to_string(&chapter).unwrap();
        text.push_str("\n\\input{chapters/nope}\n");
        std::fs::write(&chapter, text).unwrap();

        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let second = compile(dir.path(), &options).unwrap();
        assert!(!second.ok, "{engine:?}");
        assert_eq!(second.pdf_path, None, "{engine:?}");
        assert_eq!(second.errors.len(), 1, "{engine:?}: {:#?}", second.errors);
        assert_eq!(
            second.errors[0].file.as_deref(),
            Some("chapters/conclusion.tex"),
            "{engine:?}"
        );
        assert!(
            second.errors[0]
                .message
                .contains("chapters/nope.tex' not found"),
            "{engine:?}"
        );
        match engine {
            // Tectonic writes its output only when it succeeds, so the old
            // PDF stays on disk for the app to keep showing.
            Engine::Tectonic => assert_eq!(std::fs::read(&pdf).unwrap(), before),
            // pdflatex truncates the PDF when it starts and removes it
            // after a fatal error.
            Engine::Latexmk => assert!(!pdf.exists()),
        }
    }
}

#[test]
fn another_main_file_can_be_compiled_from_a_subdirectory() {
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (_dir, result) = build(&example_thesis(), engine, Some("slides/slides.tex"));
        assert!(result.ok, "{engine:?}: {result:#?}");
        assert_eq!(result.main, "slides/slides.tex");
        assert!(result.pdf_path.unwrap().ends_with("build/slides.pdf"));
    }
}

#[test]
fn a_page_of_the_compiled_thesis_renders_to_png() {
    if !have("tectonic") || !have("pdftoppm") {
        return;
    }
    let (_dir, result) = build(&example_thesis(), Engine::Tectonic, None);
    let pdf = result.pdf_path.expect("a PDF path");

    let page = render_page(Path::new(&pdf), 1, 110).expect("page 1 renders");
    assert_eq!(&page.png[..8], b"\x89PNG\r\n\x1a\n");
    // US letter at 110 dpi.
    assert_eq!((page.width, page.height), (935, 1210));
    assert_eq!(page.renderer, "pdftoppm");

    let small = render_page(Path::new(&pdf), 1, 50).expect("page 1 renders small");
    assert!(small.png.len() < page.png.len());

    let beyond = render_page(Path::new(&pdf), 500, 110)
        .unwrap_err()
        .to_string();
    assert!(beyond.contains("Page 500 does not exist"), "{beyond}");
    let zero = render_page(Path::new(&pdf), 0, 110)
        .unwrap_err()
        .to_string();
    assert!(zero.contains("numbered from 1"), "{zero}");
    let missing = render_page(Path::new("/nonexistent/x.pdf"), 1, 110)
        .unwrap_err()
        .to_string();
    assert!(missing.contains("Compile the document first"), "{missing}");
}

const BIBLATEX_MAIN: &str = "\\documentclass{article}
\\usepackage{biblatex}
\\addbibresource{refs.bib}
\\begin{document}
As shown by \\cite{knuth1984}.
\\printbibliography
\\end{document}
";

const KNUTH: &str = "@book{knuth1984,
  author = {Donald E. Knuth},
  title = {The {\\TeX}book},
  publisher = {Addison-Wesley},
  year = {1984},
}
";

/// A biblatex document in a scratch directory, with `bib` as its refs.bib.
fn biblatex_project(main: &str, bib: &str) -> common::TempDir {
    let dir = common::TempDir::new("biblatex");
    dir.write("main.tex", main);
    dir.write("refs.bib", bib);
    dir
}

#[test]
fn a_document_that_uses_biber_is_built_with_latexmk_unless_told_otherwise() {
    // Tectonic's bundle carries its own biblatex, which works only with the
    // biber release made for it. latexmk uses TeX Live's matching pair.
    if !have("latexmk") || !have("biber") {
        return;
    }
    let dir = biblatex_project(BIBLATEX_MAIN, KNUTH);
    let result = compile(dir.path(), &CompileOptions::default()).expect("compile attempted");
    assert_eq!(result.engine, Engine::Latexmk);
    assert!(result.ok, "{result:#?}");
    assert!(result.undefined_citations.is_empty(), "{result:#?}");
}

#[test]
fn a_document_that_uses_biblatex_with_bibtex_keeps_tectonic_first() {
    if !have("tectonic") {
        return;
    }
    let main = BIBLATEX_MAIN.replace("{biblatex}", "[backend=bibtex]{biblatex}");
    let dir = biblatex_project(&main, KNUTH);
    let result = compile(dir.path(), &CompileOptions::default()).expect("compile attempted");
    assert_eq!(result.engine, Engine::Tectonic);
}

#[test]
fn a_bibliography_biber_cannot_read_is_reported_at_its_line() {
    if !have("latexmk") || !have("biber") {
        return;
    }
    let broken = format!("{KNUTH}\n@article{{broken2026,\n  title = {{Unclosed,\n  year = 2026\n");
    let dir = biblatex_project(BIBLATEX_MAIN, &broken);
    let result = compile(dir.path(), &CompileOptions::default()).expect("compile attempted");
    assert!(!result.ok);
    let error = result
        .errors
        .iter()
        .find(|e| e.message.starts_with("biber: syntax error"))
        .unwrap_or_else(|| panic!("no biber error in {:#?}", result.errors));
    assert_eq!(
        (error.file.as_deref(), error.line),
        (Some("refs.bib"), Some(11))
    );
}

#[test]
fn an_empty_document_says_it_has_no_pages() {
    // The Blank template and every new .tex file start like this.
    let no_pages = error(
        "main.tex",
        None,
        "No pages of output. Add visible content to the document body.",
    );
    for engine in ENGINES {
        if !have(engine.name()) {
            continue;
        }
        let (dir, first) = build(&fixture_project("empty-body"), engine, None);
        assert!(!first.ok, "{engine:?}");
        assert_eq!(first.errors, [no_pages.clone()], "{engine:?}");
        assert_eq!(first.warnings, [], "{engine:?}");
        assert_eq!(first.pdf_path, None, "{engine:?}");

        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let second = compile(dir.path(), &options).unwrap();
        assert_eq!(second.errors, [no_pages.clone()], "{engine:?}: again");
    }
}

/// The broken-bib project: refs.bib lacks a comma, so biber stops at a
/// syntax error and every citation is left undefined.
fn broken_bib() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/biblatex-projects/broken-bib")
}

#[test]
fn a_bibliography_biber_cannot_read_gives_its_error_without_a_flood_of_citations() {
    if !have("latexmk") || !have("biber") {
        return;
    }
    let dir = copy_project(&broken_bib(), "broken-bib");
    let first = compile(dir.path(), &CompileOptions::default()).expect("compile attempted");
    assert_eq!(first.engine, Engine::Latexmk);
    assert!(!first.ok);
    assert_eq!(first.errors.len(), 1, "{:#?}", first.errors);
    assert_eq!(
        (first.errors[0].file.as_deref(), first.errors[0].line),
        (Some("refs.bib"), Some(11))
    );
    assert!(
        first.errors[0].message.starts_with("biber: syntax error"),
        "{:#?}",
        first.errors
    );
    let warnings: Vec<&str> = first.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(warnings, ["Empty bibliography"]);
    // pdflatex ran before biber, so there is a PDF to show beside the error.
    assert!(first.pdf_path.is_some(), "{first:#?}");

    // latexmk repeats its verdict without running anything: the same report.
    let second = compile(dir.path(), &CompileOptions::default()).expect("compile attempted");
    assert_eq!(second.pdf_path, first.pdf_path);
    assert_eq!(second.errors, first.errors);
    assert_eq!(second.warnings, first.warnings);
    assert_eq!(second.ok, first.ok);
}

#[test]
fn a_fatal_error_compiled_again_still_does_not_show_the_old_pdf() {
    // pdflatex leaves the previous PDF on disk when it stops in the
    // preamble, before writing a page.
    if !have("latexmk") {
        return;
    }
    let (dir, first) = build(&example_thesis(), Engine::Latexmk, None);
    assert!(first.ok, "{first:#?}");
    let main = dir.path().join("main.tex");
    let text = std::fs::read_to_string(&main).unwrap();
    std::fs::write(
        &main,
        text.replacen(
            "\\begin{document}",
            "\\usepackage{nonexistentpackage}\n\\begin{document}",
            1,
        ),
    )
    .unwrap();

    let options = CompileOptions {
        main: None,
        engine: Some(Engine::Latexmk),
    };
    for run in ["first", "second"] {
        let result = compile(dir.path(), &options).unwrap();
        assert!(!result.ok, "{run}");
        assert_eq!(result.pdf_path, None, "{run}: {result:#?}");
        assert!(dir.path().join("build/main.pdf").is_file(), "{run}");
    }
}
