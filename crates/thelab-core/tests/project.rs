//! Scanning, outlining and checking whole projects.

mod common;

use std::fmt::Write as _;

use common::{TempDir, example_thesis, fixture_project};
use thelab_core::check::{Location, check};
use thelab_core::outline::{Outline, read_outline};
use thelab_core::scan::scan;

const ARTICLE: &str = "\\documentclass{article}\n\\begin{document}\nHello.\n\\end{document}\n";

// -------------------------------------------------------------------- scan

#[test]
fn scan_lists_sources_and_picks_main_tex() {
    let project = scan(&example_thesis(), None).unwrap();
    assert_eq!(project.main.as_deref(), Some("main.tex"));
    assert_eq!(
        project.tex_files,
        [
            "chapters/background.tex",
            "chapters/conclusion.tex",
            "chapters/design.tex",
            "chapters/evaluation.tex",
            "chapters/introduction.tex",
            "figures/pipeline.tex",
            "main.tex",
            "slides/slides.tex",
        ]
    );
    assert_eq!(project.bib_files, ["refs.bib"]);
}

#[test]
fn main_detection_prefers_thesis_tex_then_the_file_nearest_the_root() {
    let dir = TempDir::new("detect");
    dir.write("notes/draft.tex", ARTICLE);
    dir.write("zeta.tex", ARTICLE);
    dir.write("alpha.tex", "\\section{No document class here}\n");
    // Commented out, so not a candidate.
    dir.write("aardvark.tex", "% \\documentclass{article}\n");
    assert_eq!(
        scan(dir.path(), None).unwrap().main.as_deref(),
        Some("zeta.tex")
    );

    dir.write("thesis.tex", ARTICLE);
    assert_eq!(
        scan(dir.path(), None).unwrap().main.as_deref(),
        Some("thesis.tex")
    );
}

#[test]
fn build_output_and_hidden_directories_are_not_scanned() {
    let dir = TempDir::new("skip");
    dir.write("main.tex", ARTICLE);
    dir.write("build/main.tex", ARTICLE);
    dir.write(".git/hooks/sample.tex", ARTICLE);
    assert_eq!(scan(dir.path(), None).unwrap().tex_files, ["main.tex"]);
}

#[test]
fn change_toml_names_the_main_file_and_an_explicit_main_overrides_it() {
    let dir = TempDir::new("config");
    dir.write("main.tex", ARTICLE);
    dir.write("talk/slides.tex", ARTICLE);
    dir.write(
        "thelab.toml",
        "main = \"talk/slides.tex\"\nengine = \"latexmk\"\n",
    );

    let project = scan(dir.path(), None).unwrap();
    assert_eq!(project.main.as_deref(), Some("talk/slides.tex"));
    assert_eq!(project.config.engine.as_deref(), Some("latexmk"));

    let project = scan(dir.path(), Some("main.tex")).unwrap();
    assert_eq!(project.main.as_deref(), Some("main.tex"));
}

#[test]
fn scan_errors_say_what_is_wrong() {
    let dir = TempDir::new("errors");
    let missing = scan(&dir.path().join("nowhere"), None)
        .unwrap_err()
        .to_string();
    assert!(missing.contains("Cannot open project folder"), "{missing}");

    dir.write("main.tex", ARTICLE);
    let bad_main = scan(dir.path(), Some("other.tex")).unwrap_err().to_string();
    assert!(bad_main.contains("other.tex does not exist"), "{bad_main}");

    dir.write("thelab.toml", "mian = \"main.tex\"\n");
    let bad_config = scan(dir.path(), None).unwrap_err().to_string();
    assert!(
        bad_config.contains("Invalid") && bad_config.contains("mian"),
        "{bad_config}"
    );
}

#[test]
fn a_project_without_a_main_file_scans_but_cannot_be_outlined() {
    let dir = TempDir::new("nomain");
    dir.write("fragment.tex", "\\section{Just a fragment}\n");
    let project = scan(dir.path(), None).unwrap();
    assert_eq!(project.main, None);
    let error = read_outline(&project).unwrap_err().to_string();
    assert!(error.contains("No main .tex file found"), "{error}");
}

// ----------------------------------------------------------------- outline

fn render(outline: &Outline) -> String {
    let mut out = String::new();
    writeln!(out, "main: {}", outline.main).unwrap();
    writeln!(out, "files: {}", outline.files.join(" ")).unwrap();
    writeln!(out, "bibliography: {}", outline.bib_resources.join(" ")).unwrap();
    writeln!(out, "words before first heading: {}", outline.front_words).unwrap();
    writeln!(out, "total words: {}", outline.total_words()).unwrap();
    writeln!(out, "\nheadings:").unwrap();
    for h in &outline.headings {
        let indent = "  ".repeat(h.level as usize);
        writeln!(
            out,
            "{indent}{} \"{}\" {}:{} ({} words)",
            h.level.name(),
            h.title,
            h.file,
            h.line,
            h.words
        )
        .unwrap();
    }
    writeln!(out, "\nunits:").unwrap();
    for u in outline.units() {
        writeln!(
            out,
            "  {} \"{}\" {} words",
            u.level.name(),
            u.title,
            u.words
        )
        .unwrap();
    }
    writeln!(out, "\nlabels:").unwrap();
    for l in &outline.labels {
        writeln!(out, "  {} {}:{}", l.name, l.file, l.line).unwrap();
    }
    writeln!(out, "\ncitations:").unwrap();
    for c in &outline.citations {
        writeln!(out, "  {} {}:{}", c.name, c.file, c.line).unwrap();
    }
    out
}

/// Compares against tests/fixtures/golden/<name>. Run with UPDATE_GOLDEN=1
/// to rewrite the file, then review the diff.
fn assert_golden(name: &str, actual: &str) {
    let path = format!(
        "{}/tests/fixtures/golden/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(actual, expected, "golden file {path} is out of date");
}

#[test]
fn outline_of_the_example_thesis() {
    let project = scan(&example_thesis(), None).unwrap();
    let outline = read_outline(&project).unwrap();
    assert_golden("thesis-outline.txt", &render(&outline));
}

#[test]
fn outline_of_the_slides_has_no_headings_but_knows_its_labels() {
    let project = scan(&example_thesis(), Some("slides/slides.tex")).unwrap();
    let outline = read_outline(&project).unwrap();
    assert_eq!(outline.files, ["slides/slides.tex"]);
    assert!(outline.headings.is_empty());
    let labels: Vec<&str> = outline.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(labels, ["frame:problem", "frame:idea", "frame:results"]);
}

#[test]
fn outline_follows_inputs_relative_to_the_main_file_and_survives_cycles() {
    let dir = TempDir::new("inputs");
    dir.write(
        "doc/main.tex",
        "\\documentclass{article}\n\\input{preamble}\n\\begin{document}\n\\section{One}\n\
         \\input{parts/two}\n\\input{parts/gone}\n\\end{document}\n\\section{After the end}\n",
    );
    dir.write(
        "doc/preamble.tex",
        "\\newcommand{\\thing}[1]{\\label{#1}\\section{Macro}}\n",
    );
    dir.write(
        "doc/parts/two.tex",
        "\\section{Two}\nSome words here.\n\\input{parts/two}\n\\input{three}\n",
    );
    // Found next to the including file, since doc/three.tex does not exist.
    dir.write("doc/parts/three.tex", "\\subsection{Three}\n");

    let project = scan(dir.path(), Some("doc/main.tex")).unwrap();
    let outline = read_outline(&project).unwrap();
    assert_eq!(
        outline.files,
        [
            "doc/main.tex",
            "doc/preamble.tex",
            "doc/parts/two.tex",
            "doc/parts/three.tex"
        ]
    );
    let titles: Vec<&str> = outline.headings.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(titles, ["One", "Two", "Three"]);
    assert_eq!(outline.headings[1].words, 3);
    assert!(
        outline.labels.is_empty(),
        "labels in macro definitions are not labels"
    );

    let missing: Vec<(&str, u32)> = outline
        .missing_inputs
        .iter()
        .map(|m| (m.name.as_str(), m.line))
        .collect();
    assert_eq!(missing, [("parts/gone", 6)]);
}

#[test]
fn headings_keep_readable_titles() {
    let dir = TempDir::new("titles");
    dir.write(
        "main.tex",
        "\\documentclass{book}\n\\begin{document}\n\\part{Foundations}\n\
         \\chapter[Short]{The \\TeX{} Engine\\label{chap:engine}}\n\\section*{An \\emph{unnumbered} one}\n\
         % \\section{Commented out}\n\\end{document}\n",
    );
    let outline = read_outline(&scan(dir.path(), None).unwrap()).unwrap();
    let got: Vec<(&str, &str, u32)> = outline
        .headings
        .iter()
        .map(|h| (h.level.name(), h.title.as_str(), h.line))
        .collect();
    assert_eq!(
        got,
        [
            ("part", "Foundations", 3),
            ("chapter", "The TeX Engine", 4),
            ("section", "An unnumbered one", 5),
        ]
    );
    let labels: Vec<&str> = outline.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(labels, ["chap:engine"]);
}

// ------------------------------------------------------------------- check

#[test]
fn the_example_thesis_has_no_reference_problems() {
    let project = scan(&example_thesis(), None).unwrap();
    let outline = read_outline(&project).unwrap();
    let report = check(&project, &outline).unwrap();

    assert!(!report.has_problems(), "{report:#?}");
    assert!(
        report.uncited_entries.is_empty(),
        "{:?}",
        report.uncited_entries
    );
    assert_eq!(report.counts.bib_entries, 7);
    assert_eq!(report.counts.citations, 7);
    assert_eq!(report.counts.labels, outline.labels.len());

    // Plenty of section labels are defined for later and not used yet.
    let unused: Vec<&str> = report
        .unused_labels
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert!(unused.contains(&"sec:motivation"), "{unused:?}");
    assert!(!unused.contains(&"chap:design"), "{unused:?}");
}

#[test]
fn check_finds_what_the_compiler_would_warn_about() {
    let project = scan(&fixture_project("warnings"), None).unwrap();
    let outline = read_outline(&project).unwrap();
    let report = check(&project, &outline).unwrap();
    assert!(report.has_problems());

    let at = |list: &[thelab_core::outline::Occurrence]| -> Vec<(String, String, u32)> {
        list.iter()
            .map(|o| (o.name.clone(), o.file.clone(), o.line))
            .collect()
    };
    assert_eq!(
        at(&report.undefined_references),
        [("sec:missing".into(), "chapters/body.tex".into(), 4)]
    );
    assert_eq!(
        at(&report.undefined_citations),
        [("nokey2020".into(), "chapters/body.tex".into(), 4)]
    );

    assert_eq!(report.duplicate_labels.len(), 1);
    assert_eq!(report.duplicate_labels[0].name, "chap:dup");
    assert_eq!(
        report.duplicate_labels[0].locations,
        [
            Location {
                file: "main.tex".into(),
                line: 5
            },
            Location {
                file: "chapters/body.tex".into(),
                line: 2
            },
        ]
    );
    assert!(report.uncited_entries.is_empty());
}

#[test]
fn check_knows_bibitems_nocite_star_listing_labels_and_missing_bib_files() {
    let dir = TempDir::new("check");
    dir.write(
        "main.tex",
        "\\documentclass{article}\n\\begin{document}\n\
         \\begin{lstlisting}[caption={Code}, label={lst:code}]\nx = 1 % \\ref{not:real}\n\\end{lstlisting}\n\
         See Listing~\\ref{lst:code}, \\cref{eq:a,eq:b} and \\cite{manual1999, absent}.\n\
         \\begin{equation}\\label{eq:a} a \\end{equation}\n\
         \\nocite{*}\n\\bibliography{refs,lost}\n\
         \\begin{thebibliography}{9}\n\\bibitem{manual1999} A manual.\n\\end{thebibliography}\n\
         \\end{document}\n",
    );
    dir.write("refs.bib", "@misc{unused2001, title = {Never cited}}\n");

    let project = scan(dir.path(), None).unwrap();
    let outline = read_outline(&project).unwrap();
    let report = check(&project, &outline).unwrap();

    let names = |list: &[thelab_core::outline::Occurrence]| -> Vec<String> {
        list.iter().map(|o| o.name.clone()).collect()
    };
    assert_eq!(names(&report.undefined_references), ["eq:b"]);
    assert_eq!(names(&report.undefined_citations), ["absent"]);
    assert_eq!(report.missing_bib_files, ["lost.bib"]);
    assert!(
        report.uncited_entries.is_empty(),
        "\\nocite{{*}} cites everything"
    );
    assert!(
        report.unused_labels.is_empty(),
        "{:?}",
        report.unused_labels
    );
}
