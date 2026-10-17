//! The command-line interface, as the app and scripts use it.

mod common;

use std::process::{Command, Stdio};

use common::{
    BINARY, TempDir, copy_project, example_thesis, fixture_project, have, have_engine, run_cli,
};
use serde_json::{Value, json};

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 stdout")
}

fn json_of(output: &std::process::Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", stdout(output)))
}

#[test]
fn compile_json_reports_ok_and_a_pdf_for_the_example_thesis() {
    if !have_engine() {
        return;
    }
    let project = copy_project(&example_thesis(), "cli-ok");
    let output = run_cli(&["compile", ".", "--json"], project.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = json_of(&output);
    assert_eq!(result["ok"], true);
    assert_eq!(result["main"], "main.tex");
    assert!(std::path::Path::new(result["pdfPath"].as_str().unwrap()).is_file());
    assert_eq!(result["errors"], json!([]));
}

#[test]
fn compile_json_reports_file_line_and_message_for_a_broken_project() {
    if !have_engine() {
        return;
    }
    let project = copy_project(&fixture_project("undefined-cs"), "cli-broken");
    // The project can also be named by path instead of being the working directory.
    let output = run_cli(
        &["compile", project.path().to_str().unwrap(), "--json"],
        &std::env::temp_dir(),
    );
    assert_eq!(output.status.code(), Some(1), "a failed build exits with 1");
    let result = json_of(&output);
    assert_eq!(result["ok"], false);
    assert_eq!(
        result["errors"],
        json!([{
            "severity": "error",
            "file": "chapters/one.tex",
            "line": 5,
            "message": "Undefined control sequence"
        }])
    );

    let text = run_cli(&["compile"], project.path());
    assert_eq!(text.status.code(), Some(1));
    assert!(
        stdout(&text).contains("error: chapters/one.tex:5: Undefined control sequence"),
        "{}",
        stdout(&text)
    );
}

/// A document that latexmk builds with biber.
fn biber_project(tag: &str) -> TempDir {
    let project = TempDir::new(tag);
    project.write(
        "main.tex",
        "\\documentclass{article}
\\usepackage{biblatex}
\\addbibresource{refs.bib}
\\begin{document}
As shown by \\cite{knuth1984}.
\\printbibliography
\\end{document}
",
    );
    project.write(
        "refs.bib",
        "@book{knuth1984,
  author = {Donald E. Knuth},
  title = {The {\\TeX}book},
  publisher = {Addison-Wesley},
  year = {1984},
}
",
    );
    project
}

/// `thelab compile --json` in `directory`, started but not waited for.
fn start_compile(
    directory: &std::path::Path,
    environment: &[(&str, &std::ffi::OsStr)],
) -> std::process::Child {
    let mut command = Command::new(BINARY);
    command
        .args(["compile", ".", "--json"])
        .current_dir(directory)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in environment {
        command.env(name, value);
    }
    command.spawn().expect("start thelab")
}

#[test]
fn two_compiles_that_run_biber_at_the_same_moment_both_succeed() {
    // The app's live preview and Claude's compile are separate processes.
    // biber 2.22's macOS build unpacks itself into a cache all bibers share,
    // and of two that start at once, one used to fail.
    if !have("latexmk") || !have("biber") {
        return;
    }
    let projects = [biber_project("biber-one"), biber_project("biber-two")];
    let compiles: Vec<_> = projects
        .iter()
        .map(|project| start_compile(project.path(), &[]))
        .collect();
    for compile in compiles {
        let output = compile.wait_with_output().expect("thelab finishes");
        let result = json_of(&output);
        assert_eq!(result["engine"], "latexmk");
        assert_eq!(result["ok"], true, "{result:#}");
    }
}

#[cfg(unix)]
#[test]
fn a_biber_that_could_not_start_is_explained_and_compiling_again_recovers() {
    // A biber started outside The Lab does not wait its turn. This stand-in
    // fails the way a collision does.
    use std::os::unix::fs::PermissionsExt;
    if !have("latexmk") || !have("biber") {
        return;
    }
    let project = biber_project("biber-collided");
    let stand_in = TempDir::new("biber-stand-in");
    let thin = "/var/folders/xx/T/par-75736572/cache-bcf1e1e0/thin/biber";
    stand_in.write(
        "biber",
        &format!(
            "#!/bin/sh\necho '{thin}: cannot execute thin binary {thin} (errno=8)' >&2\nexit 255\n"
        ),
    );
    std::fs::set_permissions(
        stand_in.path().join("biber"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut directories = vec![stand_in.path().to_path_buf()];
    directories.extend(thelab_core::tools::search_directories());
    let tools_path = std::env::join_paths(directories).unwrap();

    let failed = start_compile(project.path(), &[("THELAB_TOOLS_PATH", &tools_path)])
        .wait_with_output()
        .expect("thelab finishes");
    let result = json_of(&failed);
    assert_eq!(result["ok"], false);
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{result:#}");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("biber could not start because another biber was starting"),
        "{result:#}"
    );
    assert_eq!(result["undefinedCitations"], json!([]), "{result:#}");

    // latexmk would repeat the failure until a source changed.
    let again = run_cli(&["compile", ".", "--json"], project.path());
    let result = json_of(&again);
    assert_eq!(result["ok"], true, "{result:#}");
    assert_eq!(result["undefinedCitations"], json!([]), "{result:#}");
}

#[cfg(unix)]
#[test]
fn a_biber_document_without_biber_says_how_to_get_it() {
    // biber is installed here, so it is hidden: each folder that holds it is
    // replaced with links to everything else in it. (Without biber, biblatex
    // is usually missing too, as on CI's Linux job.)
    if !have("latexmk") || !have("biber") {
        return;
    }
    let hidden = TempDir::new("biber-hidden");
    let mut directories = Vec::new();
    for directory in thelab_core::tools::search_directories() {
        if !directory.join("biber").exists() {
            directories.push(directory);
            continue;
        }
        let copy = hidden.path().join(directories.len().to_string());
        std::fs::create_dir_all(&copy).unwrap();
        for entry in std::fs::read_dir(&directory).unwrap().flatten() {
            if entry.file_name() != "biber" {
                std::os::unix::fs::symlink(entry.path(), copy.join(entry.file_name())).unwrap();
            }
        }
        directories.push(copy);
    }
    let tools_path = std::env::join_paths(directories).unwrap();

    let project = biber_project("biber-missing");
    let output = start_compile(project.path(), &[("THELAB_TOOLS_PATH", &tools_path)])
        .wait_with_output()
        .expect("thelab finishes");
    let result = json_of(&output);
    assert_eq!(result["engine"], "latexmk");
    assert_eq!(result["ok"], false);
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{result:#}");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("biber is not installed (or not on PATH), and this document needs it"),
        "{result:#}"
    );
    // The citations are unresolved only because biber never ran.
    assert_eq!(result["undefinedCitations"], json!([]), "{result:#}");
    // pdflatex still made a PDF, for the preview to show.
    assert!(result["pdfPath"].is_string(), "{result:#}");
}

#[test]
fn outline_check_and_bib_read_sources_only() {
    let thesis = example_thesis();

    let outline = run_cli(&["outline", "--json"], &thesis);
    assert_eq!(outline.status.code(), Some(0));
    let overview = json_of(&outline);
    assert_eq!(overview["main"], "main.tex");
    assert_eq!(overview["units"][1]["title"], "Background");
    assert!(overview["totalWords"].as_u64().unwrap() > 500);

    let slides = run_cli(
        &["outline", "--main", "slides/slides.tex", "--json"],
        &thesis,
    );
    assert_eq!(
        json_of(&slides)["documentFiles"],
        json!(["slides/slides.tex"])
    );

    let check = run_cli(&["check"], &thesis);
    assert_eq!(check.status.code(), Some(0));
    assert!(
        stdout(&check).starts_with("no problems"),
        "{}",
        stdout(&check)
    );

    let problems = run_cli(&["check", "--json"], &fixture_project("warnings"));
    assert_eq!(problems.status.code(), Some(1), "problems exit with 1");
    assert_eq!(json_of(&problems)["duplicateLabels"][0]["name"], "chap:dup");

    let listing = run_cli(&["bib"], &thesis);
    assert_eq!(stdout(&listing).lines().count(), 7);
    let search = run_cli(&["bib", "--query", "shannon", "--json"], &thesis);
    assert_eq!(
        json_of(&search)["entries"][0]["key"],
        "shannon1948mathematical"
    );
    let nothing = run_cli(&["bib", "--query", "feynman"], &thesis);
    assert_eq!(nothing.status.code(), Some(1));
}

#[test]
fn usage_errors_exit_with_2_and_explain_themselves() {
    let thesis = example_thesis();
    for (arguments, expected) in [
        (vec!["frobnicate"], "unknown command frobnicate"),
        (vec!["compile", "--engine"], "--engine needs a value"),
        (
            vec!["compile", "--engine", "pdftex"],
            "Unknown engine \"pdftex\"",
        ),
        (vec!["compile", "--fast"], "unknown option --fast"),
        (
            vec!["outline", "--main", "absent.tex"],
            "absent.tex does not exist",
        ),
        (
            vec!["check", "/no/such/folder"],
            "Cannot open project folder",
        ),
    ] {
        let output = run_cli(&arguments, &thesis);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {stderr}");
        assert!(stderr.contains(expected), "{arguments:?}: {stderr}");
        assert_eq!(stdout(&output), "", "{arguments:?}: errors go to stderr");
    }

    let version = run_cli(&["--version"], &thesis);
    assert_eq!(
        stdout(&version).trim(),
        format!("thelab {}", env!("CARGO_PKG_VERSION"))
    );
    let help = run_cli(&["--help"], &thesis);
    assert!(stdout(&help).contains("Usage: thelab <command>"));
}

#[test]
fn synctex_maps_lines_to_the_page_and_back() {
    if !have_engine() {
        return;
    }
    let project = copy_project(&example_thesis(), "cli-synctex");
    let before = run_cli(
        &["synctex", "--file", "main.tex", "--line", "23"],
        project.path(),
    );
    assert_eq!(before.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&before.stderr).contains("Compile the document first"));

    assert_eq!(run_cli(&["compile"], project.path()).status.code(), Some(0));

    let forward = run_cli(
        &[
            "synctex",
            "--file",
            "chapters/design.tex",
            "--line",
            "17",
            "--json",
        ],
        project.path(),
    );
    assert_eq!(forward.status.code(), Some(0));
    let place = json_of(&forward);
    assert_eq!(place["found"], true);
    assert_eq!(place["page"], 7);
    assert_eq!(place["line"], 17);
    let (x, y) = (
        place["box"]["x"].as_f64().unwrap(),
        place["box"]["y"].as_f64().unwrap(),
    );

    // And back again from inside that line.
    let (x, y) = ((x + 60.0).to_string(), (y + 5.0).to_string());
    let inverse = run_cli(
        &["synctex", "--page", "7", "--x", &x, "--y", &y, "--json"],
        project.path(),
    );
    assert_eq!(inverse.status.code(), Some(0));
    assert_eq!(
        json_of(&inverse),
        json!({ "found": true, "file": "chapters/design.tex", "line": 17 })
    );

    let text = run_cli(
        &["synctex", "--page", "7", "--x", &x, "--y", &y],
        project.path(),
    );
    assert_eq!(stdout(&text).trim(), "chapters/design.tex:17");

    let nowhere = run_cli(
        &[
            "synctex", "--page", "7", "--x", "300", "--y", "20", "--json",
        ],
        project.path(),
    );
    assert_eq!(nowhere.status.code(), Some(1), "nothing found exits with 1");
    assert_eq!(json_of(&nowhere), json!({ "found": false }));
    let other = run_cli(
        &[
            "synctex",
            "--file",
            "slides/slides.tex",
            "--line",
            "3",
            "--json",
        ],
        project.path(),
    );
    assert_eq!(other.status.code(), Some(1));

    for (arguments, expected) in [
        (
            vec!["synctex", "--file", "main.tex"],
            "synctex needs --file and --line",
        ),
        (
            vec!["synctex", "--page", "1", "--x", "ten", "--y", "1"],
            "--x needs a number",
        ),
    ] {
        let output = run_cli(&arguments, project.path());
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{arguments:?}"
        );
    }
}

#[test]
fn python_reports_each_interpreter_and_exits_1_on_a_problem() {
    use std::os::unix::fs::PermissionsExt;
    let dir = common::TempDir::new("python");
    // A stand-in interpreter, so the test does not depend on what is installed.
    dir.write(".venv/bin/python", "#!/bin/sh\necho 'Python 3.12.0'\n");
    let python = dir.path().join(".venv/bin/python");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir.write(".venv/pyvenv.cfg", "home = /usr/bin\n");
    dir.write("sim/run.py", "");
    dir.write(
        "thelab.toml",
        "[python.folders]\n\"sim\" = \"missing/bin/python\"\n",
    );
    let project = dir.path().to_str().unwrap();

    let output = run_cli(&["python", "--json", project], dir.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "a missing interpreter is a problem"
    );
    let json = json_of(&output);
    assert_eq!(json["default"]["source"], "venv");
    assert_eq!(json["default"]["version"], "3.12.0");
    assert_eq!(json["folders"][0]["folder"], "sim");
    let problem = json["folders"][0]["problem"].as_str().unwrap();
    assert!(
        problem.contains("missing/bin/python does not exist"),
        "{problem}"
    );

    let text = stdout(&run_cli(&["python", project], dir.path()));
    assert!(text.contains("default: Python 3.12.0, "), "{text}");
    assert!(text.contains("(.venv in the project"), "{text}");
    assert!(text.contains("sim: problem: "), "{text}");
}
