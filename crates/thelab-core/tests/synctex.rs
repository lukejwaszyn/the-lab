//! SyncTeX lookups, pinned against real output from both engines for the
//! example thesis (tests/fixtures/synctex, made by regenerate.sh), and
//! checked end to end against where the words really are in a fresh PDF.

mod common;

use std::path::Path;
use std::process::Command;

use common::{copy_project, example_thesis, have};
use thelab_core::compile::{CompileOptions, Engine, compile};
use thelab_core::scan::scan;
use thelab_core::synctex::{self, SyncTex};

/// Where regenerate.sh compiled the thesis; SyncTeX records absolute paths.
/// The folder still carries the project's first name, Change: the recorded
/// fixtures hold this path, so renaming it means regenerating them.
const ROOT: &str = "/private/tmp/change-fixtures-thesis";
const ENGINES: [&str; 2] = ["tectonic", "pdflatex"];

fn fixture(engine: &str) -> SyncTex {
    let path = format!(
        "{}/tests/fixtures/synctex/thesis.{engine}.synctex",
        env!("CARGO_MANIFEST_DIR")
    );
    SyncTex::parse(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}")),
    )
}

#[test]
fn a_source_line_maps_to_its_typeset_line() {
    for engine in ENGINES {
        let sync = fixture(engine);
        // chapters/design.tex line 17: "Snapshots cost memory, so the system ...".
        // In the PDF that line of text sits on page 7 between y=425 and y=437.
        let found = sync
            .forward(Path::new(ROOT), "chapters/design.tex", 17)
            .expect(engine);
        assert_eq!((found.page, found.line), (7, 17), "{engine}");
        let b = found.line_box;
        assert!(
            (b.x - 72.0).abs() < 0.1,
            "{engine}: starts at the left margin, {b:?}"
        );
        assert!(
            (b.width - 468.0).abs() < 0.1,
            "{engine}: as wide as the text block, {b:?}"
        );
        assert!(
            (425.0..427.0).contains(&b.y) && (10.0..12.0).contains(&b.height),
            "{engine}: {b:?}"
        );
        // The exact place is on that line's baseline, after the first word.
        assert!(
            (b.y..=b.y + b.height).contains(&found.y),
            "{engine}: {found:?}"
        );
        assert!((127.0..129.0).contains(&found.x), "{engine}: {found:?}");

        // Line 36, further down the same page.
        let later = sync
            .forward(Path::new(ROOT), "chapters/design.tex", 36)
            .expect(engine);
        assert_eq!(later.page, 7, "{engine}");
        assert!(
            (635.0..637.0).contains(&later.line_box.y),
            "{engine}: {later:?}"
        );

        // Another file, another page: the abstract in main.tex is on page 2.
        assert_eq!(
            sync.forward(Path::new(ROOT), "main.tex", 23)
                .map(|f| f.page),
            Some(2),
            "{engine}"
        );
    }
}

#[test]
fn a_line_with_no_output_maps_to_the_nearest_line_that_has_some() {
    for engine in ENGINES {
        let sync = fixture(engine);
        // Line 16 of design.tex is blank; the paragraph starts on line 17.
        let found = sync
            .forward(Path::new(ROOT), "chapters/design.tex", 16)
            .expect(engine);
        assert!(
            found.line == 17 || found.line == 15,
            "{engine}: line {}",
            found.line
        );
        assert_eq!(found.page, 7, "{engine}");
    }
}

#[test]
fn a_file_outside_the_document_has_no_place_in_the_pdf() {
    for engine in ENGINES {
        let sync = fixture(engine);
        assert_eq!(
            sync.forward(Path::new(ROOT), "slides/slides.tex", 3),
            None,
            "{engine}"
        );
        assert_eq!(
            sync.forward(Path::new(ROOT), "chapters/absent.tex", 3),
            None,
            "{engine}"
        );
    }
}

#[test]
fn a_point_in_the_pdf_maps_back_to_its_source_line() {
    for engine in ENGINES {
        let sync = fixture(engine);
        let at = |x: f64, y: f64| {
            sync.inverse(Path::new(ROOT), 7, x, y)
                .map(|found| (found.file, found.line))
        };
        let design = |line: u32| Some(("chapters/design.tex".to_string(), line));
        // The word "deterministic", the word "cost", and a section heading.
        assert_eq!(at(282.0, 641.0), design(36), "{engine}");
        assert_eq!(at(138.0, 431.0), design(17), "{engine}");
        assert_eq!(at(160.0, 518.0), design(25), "{engine}");
        // The top margin has nothing near it.
        assert_eq!(at(300.0, 20.0), None, "{engine}");
        assert_eq!(
            sync.inverse(Path::new(ROOT), 99, 282.0, 641.0),
            None,
            "{engine}"
        );
    }
}

#[test]
fn a_project_that_moved_since_it_was_compiled_still_resolves() {
    // The recorded paths start with ROOT; the project is now somewhere else.
    for engine in ENGINES {
        let sync = fixture(engine);
        let elsewhere = Path::new("/somewhere/else/thesis");
        assert_eq!(
            sync.forward(elsewhere, "chapters/design.tex", 17)
                .map(|f| f.page),
            Some(7),
            "{engine}"
        );
    }
}

/// The boxes of every word on a page, from Poppler: (text, xMin, yMin, xMax, yMax).
fn words_on_page(pdf: &Path, page: u32) -> Vec<(String, f64, f64, f64, f64)> {
    let pdftotext = thelab_core::tools::find_program("pdftotext").expect("pdftotext");
    let page = page.to_string();
    let output = Command::new(pdftotext)
        .args(["-bbox", "-f", &page, "-l", &page])
        .arg(pdf)
        .arg("-")
        .output()
        .expect("run pdftotext");
    let number = |line: &str, name: &str| -> f64 {
        let start = line.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
        line[start..].split('"').next().unwrap().parse().unwrap()
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.trim_start().starts_with("<word "))
        .map(|line| {
            let text = line
                .split('>')
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap()
                .to_string();
            (
                text,
                number(line, "xMin"),
                number(line, "yMin"),
                number(line, "xMax"),
                number(line, "yMax"),
            )
        })
        .collect()
}

#[test]
fn lookups_agree_with_where_the_words_are_in_a_fresh_pdf() {
    if !have("pdftotext") {
        return;
    }
    for engine in [Engine::Tectonic, Engine::Latexmk] {
        if !have(engine.name()) {
            continue;
        }
        let dir = copy_project(&example_thesis(), "synctex");
        let options = CompileOptions {
            main: None,
            engine: Some(engine),
        };
        let result = compile(dir.path(), &options).unwrap();
        assert!(result.ok, "{engine:?}");
        let project = scan(dir.path(), None).unwrap();
        // This reads build/main.synctex.gz, as the app does.
        let sync = synctex::load(&project).expect("SyncTeX data from the compile");

        // Forward: the line that begins "Snapshots cost memory".
        let found = sync
            .forward(&project.root, "chapters/design.tex", 17)
            .expect("forward");
        let words = words_on_page(Path::new(result.pdf_path.as_deref().unwrap()), found.page);
        let cost = words
            .iter()
            .find(|w| {
                w.0 == "cost"
                    && w.2 < found.line_box.y + found.line_box.height
                    && w.4 > found.line_box.y
            })
            .unwrap_or_else(|| {
                panic!(
                    "{engine:?}: no word \"cost\" on the line at {:?}",
                    found.line_box
                )
            });
        // SyncTeX's place on that line is the gap just before "cost".
        assert!(
            (found.x - cost.1).abs() < 3.0,
            "{engine:?}: x {} against {}",
            found.x,
            cost.1
        );

        // Inverse: the middle of the word "deterministic".
        let word = words
            .iter()
            .find(|w| w.0 == "deterministic")
            .expect("the word deterministic");
        let back = sync.inverse(
            &project.root,
            found.page,
            (word.1 + word.3) / 2.0,
            (word.2 + word.4) / 2.0,
        );
        assert_eq!(
            back.map(|b| (b.file, b.line)),
            Some(("chapters/design.tex".to_string(), 36)),
            "{engine:?}"
        );
    }
}

#[test]
fn a_project_that_was_never_compiled_says_so() {
    let dir = copy_project(&example_thesis(), "synctex-none");
    let project = scan(dir.path(), None).unwrap();
    let error = synctex::load(&project).unwrap_err().to_string();
    assert!(
        error.contains("No SyncTeX data") && error.contains("Compile the document first"),
        "{error}"
    );
}
