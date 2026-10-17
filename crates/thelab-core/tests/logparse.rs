//! Log parser tests, pinned against real logs under tests/fixtures/logs.
//! Regenerate those with tests/fixtures/regenerate.sh.

use thelab_core::logparse::{
    Diagnostic, OutputFile, Severity, parse_blg, parse_tectonic_console, parse_tex_log,
};

fn fixture(path: &str) -> String {
    let full = format!("{}/tests/fixtures/logs/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {full}: {e}"))
}

fn error(file: Option<&str>, line: Option<u32>, message: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        file: file.map(str::to_string),
        line,
        message: message.to_string(),
    }
}

const LONG: &str = "chapters/a-chapter-with-a-very-long-file-name-that-pushes-the-log-line-past-column-seventy-nine";

// ---------------------------------------------------------------- errors

#[test]
fn undefined_control_sequence_in_an_input_file() {
    // Tectonic logs the name as written in \input, without the extension.
    let log = parse_tex_log(&fixture("tectonic/undefined-cs.log"));
    assert_eq!(
        log.errors,
        [error(
            Some("chapters/one"),
            Some(5),
            "Undefined control sequence"
        )]
    );

    for variant in ["pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/undefined-cs.log")));
        assert_eq!(
            log.errors,
            [error(
                Some("chapters/one.tex"),
                Some(5),
                "Undefined control sequence"
            )],
            "{variant}"
        );
    }
}

#[test]
fn file_names_wrapped_at_column_79_are_rejoined() {
    let log = parse_tex_log(&fixture("tectonic/long-path.log"));
    assert_eq!(
        log.errors,
        [error(Some(LONG), Some(5), "Undefined control sequence")]
    );

    let with_ext = format!("{LONG}.tex");
    for variant in ["pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/long-path.log")));
        assert_eq!(
            log.errors,
            [error(
                Some(&with_ext),
                Some(5),
                "Undefined control sequence"
            )],
            "{variant}"
        );
    }
}

#[test]
fn missing_dollar() {
    // Tectonic halts at the first error.
    let log = parse_tex_log(&fixture("tectonic/missing-dollar.log"));
    assert_eq!(
        log.errors,
        [error(Some("main.tex"), Some(4), "Missing $ inserted")]
    );

    // pdflatex carries on and reports the closing `$` it inserts as well.
    for variant in ["pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/missing-dollar.log")));
        assert_eq!(
            log.errors,
            [
                error(Some("main.tex"), Some(4), "Missing $ inserted"),
                error(Some("main.tex"), Some(5), "Missing $ inserted"),
            ],
            "{variant}"
        );
    }
}

#[test]
fn missing_input_file_reports_the_latex_error_not_the_emergency_stop() {
    let expected = [error(
        Some("main.tex"),
        Some(4),
        "LaTeX Error: File `chapters/nope.tex' not found",
    )];
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/missing-file.log")));
        assert_eq!(log.errors, expected, "{variant}");
    }
}

#[test]
fn unclosed_environment() {
    let expected = [error(
        Some("main.tex"),
        Some(8),
        r"LaTeX Error: \begin{itemize} on input line 3 ended by \end{document}",
    )];
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/env-mismatch.log")));
        assert_eq!(log.errors, expected, "{variant}");
    }
}

#[test]
fn runaway_argument_has_no_location() {
    // TeX reads past the end of main.tex, so the log has neither a file on
    // the stack nor an `l.N` line. The compile driver falls back to the main
    // file for errors like this.
    let expected = [error(
        None,
        None,
        r"File ended while scanning use of \@xdblarg",
    )];
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/runaway.log")));
        assert_eq!(log.errors, expected, "{variant}");
    }
}

// -------------------------------------------------------------- warnings

#[test]
fn undefined_references_citations_and_duplicate_labels() {
    for (variant, body) in [
        ("tectonic", "chapters/body"),
        ("pdflatex", "chapters/body.tex"),
        ("pdflatex-file-line-error", "chapters/body.tex"),
    ] {
        let log = parse_tex_log(&fixture(&format!("{variant}/warnings.log")));
        assert!(log.errors.is_empty(), "{variant}: {:?}", log.errors);
        assert_eq!(log.undefined_references, ["sec:missing"], "{variant}");
        assert_eq!(log.undefined_citations, ["nokey2020"], "{variant}");
        assert_eq!(log.duplicate_labels, ["chap:dup"], "{variant}");

        let messages: Vec<&str> = log.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Label `chap:dup' multiply defined",
                "Reference `sec:missing' on page 1 undefined",
                "Package natbib Warning: Citation `nokey2020' on page 1 undefined",
            ],
            "{variant}"
        );

        // The duplicate is noticed while reading main.aux, which is no use
        // as a location.
        assert_eq!(log.warnings[0].file, None, "{variant}");
        assert_eq!(log.warnings[0].line, None, "{variant}");
        for w in &log.warnings[1..] {
            assert_eq!(w.severity, Severity::Warning);
            assert_eq!(w.file.as_deref(), Some(body), "{variant}");
            assert_eq!(w.line, Some(4), "{variant}");
        }
    }
}

#[test]
fn boxes_are_summarized_not_listed() {
    for (variant, body, points) in [
        ("tectonic", "chapters/body", 323.5),
        ("pdflatex", "chapters/body.tex", 323.89082),
        ("pdflatex-file-line-error", "chapters/body.tex", 323.89082),
    ] {
        let log = parse_tex_log(&fixture(&format!("{variant}/warnings.log")));
        assert_eq!(log.boxes.overfull_hbox, 1, "{variant}");
        assert_eq!(log.boxes.underfull_hbox, 0, "{variant}");
        assert_eq!(log.boxes.overfull_vbox, 0, "{variant}");
        assert_eq!(log.boxes.underfull_vbox, 1, "{variant}");

        let worst = log
            .boxes
            .worst_overfull
            .as_ref()
            .expect("worst overfull box");
        assert_eq!(worst.points, points, "{variant}");
        assert_eq!(worst.file.as_deref(), Some(body), "{variant}");
        assert_eq!(worst.line, Some(7), "{variant}");

        assert!(
            log.warnings
                .iter()
                .all(|w| !w.message.contains("hbox") && !w.message.contains("vbox")),
            "{variant}: boxes leaked into warnings"
        );
    }
}

#[test]
fn pdftex_warnings_take_their_line_from_the_context_that_follows() {
    let log = parse_tex_log(&fixture("pdflatex/duplicate-anchor.log"));
    assert!(log.errors.is_empty(), "{:?}", log.errors);
    let got: Vec<(Option<&str>, Option<u32>, &str)> = log
        .warnings
        .iter()
        .map(|w| (w.file.as_deref(), w.line, w.message.as_str()))
        .collect();
    let message = "pdfTeX warning (ext4): destination with the same identifier (name{page.1}) \
                   has been already used, duplicate ignored";
    assert_eq!(
        got,
        [
            (Some("main.tex"), Some(9), message),
            (Some("main.tex"), Some(11), message)
        ]
    );
}

#[test]
fn the_example_thesis_is_clean() {
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/thesis.log")));
        assert!(log.errors.is_empty(), "{variant}: {:?}", log.errors);
        assert!(log.warnings.is_empty(), "{variant}: {:?}", log.warnings);
        assert!(log.undefined_references.is_empty(), "{variant}");
        assert!(log.undefined_citations.is_empty(), "{variant}");
        assert!(log.duplicate_labels.is_empty(), "{variant}");
        assert_eq!(log.boxes.total(), 0, "{variant}");
    }
}

#[test]
fn text_parentheses_do_not_confuse_file_tracking() {
    // Ordinary log text is full of parentheses. After all of it, an error in
    // the included file must still be attributed to that file.
    let log = "\
This is pdfTeX, Version 3.14 (TeX Live 2026) (preloaded format=pdflatex)
(./main.tex
LaTeX2e <2025-11-01>
(/usr/local/texlive/texmf-dist/tex/latex/base/article.cls
Document Class: article 2025/01/22 v1.4n Standard LaTeX document class
File: size10.clo 2025/01/22 v1.4n Standard LaTeX file (size option)
)
LaTeX Font Info:    External font `cmex10' loaded for size
(Font)              <7> on input line 4.
(./sections/intro.tex

Underfull \\hbox (badness 10000) in paragraph at lines 3--4
[]\\OT1/cmr/m/n/10 text with an unbalanced paren ( in the box dump
 []

! Undefined control sequence.
l.9 some (unbalanced source \\oops
                                 and more) text) here
The control sequence at the end of the top line
of your error message was never \\def'ed.

) [1] )
";
    let log = parse_tex_log(log);
    assert_eq!(
        log.errors,
        [error(
            Some("sections/intro.tex"),
            Some(9),
            "Undefined control sequence"
        )]
    );
    assert_eq!(log.boxes.underfull_hbox, 1);
}

#[test]
fn an_error_after_an_included_file_closes_belongs_to_the_parent() {
    let log = "\
(./main.tex
(./a.tex
) (./b.tex)
! Missing $ inserted.
<inserted text>
                $
l.12 x_
       1

)
";
    let log = parse_tex_log(log);
    assert_eq!(
        log.errors,
        [error(Some("main.tex"), Some(12), "Missing $ inserted")]
    );
}

#[test]
fn package_warning_continuation_lines_are_joined() {
    let log = "\
(./main.tex

Package hyperref Warning: Token not allowed in a PDF string (Unicode):
(hyperref)                removing `math shift' on input line 14.


LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.

)
";
    let log = parse_tex_log(log);
    let got: Vec<(Option<u32>, &str)> = log
        .warnings
        .iter()
        .map(|w| (w.line, w.message.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Some(14),
                "Package hyperref Warning: Token not allowed in a PDF string (Unicode): \
                 removing `math shift'"
            ),
            (
                None,
                "Label(s) may have changed. Rerun to get cross-references right"
            ),
        ]
    );
}

#[test]
fn biblatex_citation_warnings_quote_the_key_with_straight_quotes() {
    // latexmk with biber, after biber stopped at a syntax error in refs.bib:
    // the .bbl is empty and every citation is undefined. biblatex quotes
    // the key 'like this', not `like this' as the kernel and natbib do.
    let log = parse_tex_log(&fixture("latexmk/broken-bib.log"));
    assert!(log.errors.is_empty(), "{:?}", log.errors);
    assert_eq!(
        log.undefined_citations,
        ["knuth1984texbook", "lamport1994latex"]
    );
    let got: Vec<(Option<&str>, Option<u32>, &str)> = log
        .warnings
        .iter()
        .map(|w| (w.file.as_deref(), w.line, w.message.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Some("main.tex"),
                Some(5),
                "Citation 'knuth1984texbook' on page 1 undefined"
            ),
            (
                Some("main.tex"),
                Some(5),
                "Citation 'lamport1994latex' on page 1 undefined"
            ),
            (
                Some("main.tex"),
                Some(8),
                "Citation 'knuth1984texbook' on page 2 undefined"
            ),
            (Some("main.tex"), Some(9), "Empty bibliography"),
            (
                Some("main.tex"),
                None,
                "Package biblatex Warning: Please (re)run Biber on the file: main and rerun \
                 LaTeX afterwards"
            ),
        ]
    );
}

// ---------------------------------------------------------------- output

#[test]
fn the_log_names_the_file_the_run_wrote_and_its_size() {
    let thesis = parse_tex_log(&fixture("pdflatex/thesis.log"));
    assert_eq!(
        thesis.output,
        Some(OutputFile {
            file: "main.pdf".to_string(),
            pages: 10,
            bytes: Some(114560),
        })
    );
    assert!(!thesis.no_pages);

    // Tectonic's XeTeX writes an .xdv, which xdvipdfmx turns into the PDF.
    let tectonic = parse_tex_log(&fixture("tectonic/thesis.log"));
    assert_eq!(
        tectonic.output,
        Some(OutputFile {
            file: "main.xdv".to_string(),
            pages: 10,
            bytes: Some(89556),
        })
    );

    // latexmk's -outdir: the log names the folder too.
    let latexmk = parse_tex_log(&fixture("latexmk/broken-bib.log"));
    assert_eq!(
        latexmk.output,
        Some(OutputFile {
            file: "build/main.pdf".to_string(),
            pages: 2,
            bytes: Some(30077),
        })
    );
}

#[test]
fn xetex_writing_a_pdf_itself_does_not_give_its_size() {
    let log = parse_tex_log("(./main.tex)\nOutput written on build/main.pdf (1 page).\n");
    assert_eq!(
        log.output,
        Some(OutputFile {
            file: "build/main.pdf".to_string(),
            pages: 1,
            bytes: None,
        })
    );
}

#[test]
fn a_long_output_path_wrapped_at_column_79_is_rejoined() {
    let path = "/tmp/a-project-folder-with-a-rather-long-name/.prism/build/build/main.pdf";
    let line = format!("Output written on {path} (12 pages, 345678 bytes).");
    let (head, tail) = line.split_at(79);
    let log = parse_tex_log(&format!("(./main.tex)\n{head}\n{tail}\n"));
    assert_eq!(
        log.output.map(|o| (o.file, o.bytes)),
        Some((path.to_string(), Some(345678)))
    );
}

#[test]
fn pdftex_quotes_an_output_path_with_a_space_and_the_quotes_are_not_the_name() {
    // A main file named "my thesis.tex" with an error pdflatex continues
    // past, run as thelab runs latexmk: `Output written on "build/my
    // thesis.pdf" (1 page, 23668 bytes).`
    let log = parse_tex_log(&fixture("latexmk/spaced-name.log"));
    assert_eq!(
        log.output,
        Some(OutputFile {
            file: "build/my thesis.pdf".to_string(),
            pages: 1,
            bytes: Some(23668),
        })
    );
}

#[test]
fn a_quoted_absolute_output_path_wrapped_at_column_79_is_rejoined() {
    // The same file in a subfolder of "a folder": the build folder is
    // outside the folder latexmk ran in, so the log names it absolutely.
    let log = parse_tex_log(&fixture("latexmk/spaced-folder.log"));
    assert_eq!(
        log.output,
        Some(OutputFile {
            file: "/private/tmp/thelab-fixtures/a folder/build/my thesis.pdf".to_string(),
            pages: 1,
            bytes: Some(23668),
        })
    );
}

#[test]
fn a_file_name_with_a_space_is_followed_whole() {
    // TeX prints the name as it is when it opens the file, `(./my
    // thesis.tex` (Tectonic: `(my thesis.tex`), and LuaTeX quotes it,
    // `("./my thesis.tex"`. An error or warning found by following those
    // belongs to "my thesis.tex", not to "my".
    let undefined = error(Some("my thesis.tex"), Some(3), "Undefined control sequence");
    for variant in ["tectonic", "latexmk"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/spaced-name.log")));
        assert_eq!(log.errors, [undefined.clone()], "{variant}");
    }
    for variant in ["latexmk/spaced-name", "latexmk/spaced-name-lualatex"] {
        let log = parse_tex_log(&fixture(&format!("{variant}.log")));
        assert_eq!(log.errors, [undefined.clone()], "{variant}");
        let reference = log
            .warnings
            .iter()
            .find(|w| w.message == "Reference `nowhere' on page 1 undefined")
            .unwrap_or_else(|| panic!("{variant}: {:#?}", log.warnings));
        assert_eq!(
            (reference.file.as_deref(), reference.line),
            (Some("my thesis.tex"), Some(4)),
            "{variant}"
        );
    }
}

#[test]
fn names_with_spaces_opened_on_one_line_are_followed_whole() {
    // As pdfTeX prints files opened in turn, with text in parentheses that
    // is no file name in between.
    let log = "\
(./main.tex (./chapters/first part.tex) (15.0pt too wide) (see notes 2.5)
(./chapters/second part.tex
LaTeX Warning: Reference `x' on page 1 undefined on input line 7.

) )
";
    let log = parse_tex_log(log);
    let located: Vec<(Option<&str>, Option<u32>)> = log
        .warnings
        .iter()
        .map(|w| (w.file.as_deref(), w.line))
        .collect();
    assert_eq!(located, [(Some("chapters/second part.tex"), Some(7))]);
}

#[test]
fn a_document_with_an_empty_body_has_no_pages_and_no_errors() {
    // What the Blank template and every new .tex file start as.
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/empty-body.log")));
        assert!(log.no_pages, "{variant}");
        assert_eq!(log.output, None, "{variant}");
        assert!(log.errors.is_empty(), "{variant}: {:?}", log.errors);
        assert!(log.warnings.is_empty(), "{variant}: {:?}", log.warnings);
    }
}

#[test]
fn a_fatal_error_leaves_no_output() {
    // pdflatex's fatal error says "no output PDF file produced"; the
    // Tectonic log says "No pages of output" as well.
    for variant in ["tectonic", "pdflatex", "pdflatex-file-line-error"] {
        let log = parse_tex_log(&fixture(&format!("{variant}/missing-file.log")));
        assert_eq!(log.output, None, "{variant}");
        assert!(!log.errors.is_empty(), "{variant}");
    }
}

// ------------------------------------------------------- tectonic console

#[test]
fn tectonic_console_errors() {
    let cases: [(&str, Diagnostic); 5] = [
        (
            "undefined-cs",
            error(Some("chapters/one"), Some(5), "Undefined control sequence"),
        ),
        (
            "missing-dollar",
            error(Some("main.tex"), Some(4), "Missing $ inserted"),
        ),
        (
            "missing-file",
            error(
                Some("main.tex"),
                Some(4),
                "LaTeX Error: File `chapters/nope.tex' not found",
            ),
        ),
        (
            "env-mismatch",
            error(
                Some("main.tex"),
                Some(8),
                r"LaTeX Error: \begin{itemize} on input line 3 ended by \end{document}",
            ),
        ),
        (
            "runaway",
            error(None, None, r"File ended while scanning use of \@xdblarg"),
        ),
    ];
    for (name, expected) in cases {
        let console = parse_tectonic_console(&fixture(&format!("tectonic/{name}.stderr.txt")));
        // Only the real error survives: "something bad happened inside XeTeX"
        // and friends are boilerplate.
        assert_eq!(console.errors, [expected], "{name}");
    }
}

#[test]
fn tectonic_console_relays_engine_warnings_separately() {
    // Tectonic prints box warnings once per TeX pass. They also end up in the
    // kept log, so they are reported apart from Tectonic's own warnings.
    let console = parse_tectonic_console(&fixture("tectonic/warnings.stderr.txt"));
    assert!(console.errors.is_empty());
    assert!(console.warnings.is_empty(), "{:?}", console.warnings);
    let relayed: Vec<(Option<&str>, Option<u32>)> = console
        .engine_warnings
        .iter()
        .map(|w| (w.file.as_deref(), w.line))
        .collect();
    assert_eq!(
        relayed,
        [
            (Some("chapters/body"), Some(8)),
            (Some("chapters/body"), Some(9))
        ]
    );
}

#[test]
fn tectonic_console_keeps_backend_warnings_once() {
    let console = parse_tectonic_console(&fixture("tectonic/duplicate-anchor.stderr.txt"));
    let messages: Vec<&str> = console
        .warnings
        .iter()
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(messages, ["Object @page.1 already defined"]);
}

#[test]
fn tectonic_console_drops_bibtex_boilerplate() {
    // With \include, Tectonic runs BibTeX on every chapter's .aux and warns
    // each time. The details, if any are real, come from the .blg instead.
    let console = parse_tectonic_console(&fixture("tectonic/thesis.stderr.txt"));
    assert!(console.errors.is_empty());
    assert!(console.warnings.is_empty(), "{:?}", console.warnings);
}

#[test]
fn tectonic_console_keeps_errors_that_are_not_about_tex() {
    let console = parse_tectonic_console(
        "note: \"version 2\" Tectonic command-line interface activated\n\
         error: output directory \"build\" does not exist\n",
    );
    assert_eq!(
        console.errors,
        [error(
            None,
            None,
            "output directory \"build\" does not exist"
        )]
    );
}

#[test]
fn tectonic_console_keeps_what_a_failed_external_tool_printed() {
    // Tectonic quotes the tool's output between rules, as it does the TeX
    // engine's, but here the rules hold the only explanation there is. The
    // tool's name is in a note on stdout, which the compile driver appends
    // after stderr.
    let console = parse_tectonic_console(&format!(
        "{}\nnote: Running external tool biber ...\n",
        fixture("tectonic/biber-mismatch.stderr.txt")
    ));
    assert_eq!(
        console.errors,
        [error(
            None,
            None,
            "biber exited with error code 2: Error: Found biblatex control file version 3.8, \
             expected version 3.11. This means that your biber (2.22) and biblatex (3.17) \
             versions are incompatible. See compat matrix in biblatex or biber PDF documentation."
        )]
    );
}

#[test]
fn tectonic_console_reports_a_silent_external_tool_failure_once() {
    let rule = "=".repeat(79);
    let console = parse_tectonic_console(&format!(
        "error: the external tool exited with an error code; its stdout was:\n\n{rule}\n{rule}\n\
         error: its stderr was:\n\n{rule}\n{rule}\n\
         error: the external tool exited with error code 3\n"
    ));
    assert_eq!(
        console.errors,
        [error(
            None,
            None,
            "the external tool exited with error code 3 without printing an error"
        )]
    );
}

// ------------------------------------------------------------------ bibtex

#[test]
fn blg_missing_database_entry() {
    for variant in ["tectonic", "pdflatex"] {
        let blg = parse_blg(&fixture(&format!("{variant}/warnings.blg")));
        assert!(blg.errors.is_empty(), "{variant}");
        assert_eq!(blg.missing_entries, ["nokey2020"], "{variant}");
        let messages: Vec<&str> = blg.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(
            messages,
            ["BibTeX: I didn't find a database entry for \"nokey2020\""]
        );
    }
}

#[test]
fn blg_clean_run() {
    for variant in ["tectonic", "pdflatex"] {
        let blg = parse_blg(&fixture(&format!("{variant}/thesis.blg")));
        assert!(blg.errors.is_empty(), "{variant}");
        assert!(blg.warnings.is_empty(), "{variant}");
        assert!(blg.missing_entries.is_empty(), "{variant}");
    }
}

#[test]
fn blg_errors_carry_the_bib_file_and_line() {
    let blg = parse_blg(
        "This is BibTeX, Version 0.99d\n\
         The top-level auxiliary file: main.aux\n\
         Database file #1: refs.bib\n\
         Repeated entry---line 12 of file refs.bib\n \
         : @book{knuth1984texbook\n \
         :                       ,\n\
         I'm skipping whatever remains of this entry\n\
         I couldn't open database file missing.bib\n\
         ---line 4 of file main.aux\n \
         : \\bibdata{missing\n \
         :                 }\n\
         Warning--empty year in lamport1994latex\n\
         (There were 2 error messages)\n",
    );
    assert_eq!(
        blg.errors,
        [
            error(Some("refs.bib"), Some(12), "BibTeX: Repeated entry"),
            error(
                None,
                None,
                "BibTeX: I couldn't open database file missing.bib"
            ),
        ]
    );
    let messages: Vec<&str> = blg.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(messages, ["BibTeX: empty year in lamport1994latex"]);
}

#[test]
fn biber_blg_errors_point_at_the_bib_file_not_biber_s_copy_of_it() {
    // biber parses a temporary UTF-8 copy of each .bib file. Its line
    // numbers are the original's, so the message is moved onto that file.
    let blg = parse_blg(&fixture("biber/syntax-error.blg"));
    assert_eq!(
        blg.errors,
        [error(
            Some("refs.bib"),
            Some(11),
            "biber: syntax error: at end of input, expected one of: number, name (entry type, \
             key, field, or macro name) or quoted string ({...} or \"...\")"
        )]
    );
    assert_eq!(
        blg.warnings,
        [Diagnostic {
            severity: Severity::Warning,
            file: Some("refs.bib".to_string()),
            line: Some(10),
            message: "biber: warning: possible runaway string started at line 9".to_string(),
        }]
    );
}

#[test]
fn biber_blg_messages_without_a_bib_location_are_kept_whole() {
    let blg = parse_blg(
        "[57] Biber.pm:421> INFO - Reading 'build/main.bcf'\n\
         [60] Biber.pm:398> ERROR - Error: Found biblatex control file version 3.8, expected version 3.11.\n\
         [61] Biber.pm:137> INFO - ERRORS: 1\n",
    );
    assert_eq!(
        blg.errors,
        [error(
            None,
            None,
            "biber: Error: Found biblatex control file version 3.8, expected version 3.11."
        )]
    );
    assert!(blg.warnings.is_empty());
}
