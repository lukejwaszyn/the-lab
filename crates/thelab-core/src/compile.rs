//! Runs a TeX engine on a project and turns its output into diagnostics.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

use crate::logparse::{
    BoxSummary, Diagnostic, OutputFile, Severity, parse_blg, parse_tectonic_console, parse_tex_log,
};
use crate::scan::{Project, read_text, scan};
use crate::tex::Source;
use crate::tools::{Output, find_program, run};
use crate::{Error, Result};

/// Directory inside the project that receives the PDF, logs and SyncTeX data.
/// The app watches the same directory.
pub const BUILD_DIRECTORY: &str = "build";

/// Seconds a compile may run, unless this variable says otherwise.
pub const TIMEOUT_VARIABLE: &str = "THELAB_COMPILE_TIMEOUT";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Tectonic,
    Latexmk,
}

impl Engine {
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "tectonic" => Ok(Self::Tectonic),
            "latexmk" => Ok(Self::Latexmk),
            other => Err(Error::new(format!(
                "Unknown engine \"{other}\": expected \"tectonic\" or \"latexmk\""
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tectonic => "tectonic",
            Self::Latexmk => "latexmk",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompileOptions {
    /// Main file relative to the project root. Detected when absent.
    pub main: Option<String>,
    /// Engine to use. Chosen automatically when absent.
    pub engine: Option<Engine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    /// True when the engine succeeded, there are no errors, and a PDF exists.
    pub ok: bool,
    pub engine: Engine,
    pub root: String,
    /// Main file, relative to `root`.
    pub main: String,
    /// The PDF the engine wrote the last time it typeset the sources: in
    /// this run, or, when latexmk found nothing to redo, in the run whose
    /// verdict it repeated. Absent if that run wrote none, even if an older
    /// PDF is still on disk. A bibliography that biber could not read
    /// leaves the PDF of the last typesetting in place.
    pub pdf_path: Option<String>,
    pub log_path: Option<String>,
    pub duration_ms: u64,
    /// Files are relative to `root` where they belong to the project.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub undefined_references: Vec<String>,
    pub undefined_citations: Vec<String>,
    pub duplicate_labels: Vec<String>,
    pub boxes: BoxSummary,
}

/// Compiles the project at `root`.
///
/// `Err` means the compile could not be attempted (no main file, no engine
/// installed). A document that fails to build is an `Ok` result with
/// `ok: false` and the errors to fix.
pub fn compile(root: &Path, options: &CompileOptions) -> Result<CompileResult> {
    let project = scan(root, options.main.as_deref())?;
    let main = project.require_main()?.to_string();
    let main_text = read_text(&project.root.join(&main)).unwrap_or_default();
    let program = main_program(&main_text);
    let biber = uses_biber(&main_text);
    let (engine, executable) = choose_engine(&project, options.engine, program, biber)?;

    let build = project.root.join(BUILD_DIRECTORY);
    fs::create_dir_all(&build)
        .map_err(|e| Error::new(format!("Cannot create {}: {e}", build.display())))?;
    let main_dir = main.rsplit_once('/').map_or("", |(directory, _)| directory);
    let stem = Path::new(&main)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main")
        .to_string();
    let pdf = build.join(format!("{stem}.pdf"));
    let log = build.join(format!("{stem}.log"));
    let blg = build.join(format!("{stem}.blg"));
    // biblatex writes this control file for biber only.
    let bcf = build.join(format!("{stem}.bcf"));

    let clock = Instant::now();
    // An earlier run's control file also counts, for a preamble kept in a
    // separate file. Everything below sees the build folder as it is once
    // the turn has come.
    let turn = if biber || bcf.is_file() {
        biber_turn()
    } else {
        None
    };

    // A file name that starts with a dash must not be taken for an option.
    let as_argument = |name: &str| {
        if name.starts_with('-') {
            format!("./{name}")
        } else {
            name.to_string()
        }
    };

    let mut command = Command::new(&executable);
    match engine {
        Engine::Tectonic => {
            command.current_dir(&project.root);
            command
                .args(["-X", "compile"])
                .arg(as_argument(&main))
                .args(["--synctex", "--keep-logs", "--outdir"])
                .arg(&build);
        }
        Engine::Latexmk => {
            // pdflatex writes each \include'd file's .aux next to where the
            // source would be, and does not create directories for them.
            mirror_directories(&project, main_dir, &build);
            let file_name = main.rsplit('/').next().unwrap_or(&main);
            command.current_dir(project.root.join(main_dir));
            command.arg(match program {
                Some(Program::XeLaTeX) => "-xelatex",
                Some(Program::LuaLaTeX) => "-lualatex",
                _ => "-pdf",
            });
            command.args(["-interaction=nonstopmode", "-file-line-error", "-synctex=1"]);
            // A control file without biber's log is what a biber that never
            // started leaves behind (see `biber_turn`). latexmk would repeat
            // that failure until a source changed, so it reruns everything.
            if bcf.is_file() && !blg.is_file() {
                command.arg("-g");
            }
            command
                .arg(format!("-outdir={}", build.display()))
                .arg(as_argument(file_name));
        }
    }

    // Taken before the run, to tell afterwards which files the run wrote.
    let before = [&pdf, &log, &blg].map(|path| stamp(path));

    let limit = timeout();
    let output = run(&mut command, limit)
        .map_err(|e| Error::new(format!("Cannot run {}: {e}", executable.display())))?;
    drop(turn);
    let duration_ms = clock.elapsed().as_millis() as u64;
    let succeeded = output.status.is_some_and(|status| status.success());

    let rewritten = |path: &Path, before: Option<Stamp>| path.is_file() && stamp(path) != before;
    let [pdf_rewritten, log_rewritten, blg_rewritten] = [
        rewritten(&pdf, before[0]),
        rewritten(&log, before[1]),
        rewritten(&blg, before[2]),
    ];
    // Tectonic starts from scratch and rewrites its output on every run, so
    // a file it left alone belongs to an earlier run. latexmk runs nothing
    // when no source has changed and repeats its last verdict instead, so
    // the log on disk is the one that verdict is about.
    let (log_current, blg_current) = match engine {
        Engine::Tectonic => (log_rewritten, blg_rewritten),
        Engine::Latexmk => (log.is_file(), blg.is_file()),
    };

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut report = if log_current {
        read_text(&log).map(|text| parse_tex_log(&text)).ok()
    } else {
        None
    };
    let has_log = report.is_some();
    let mut report = report.take().unwrap_or_default();

    let pdf_current = match engine {
        Engine::Tectonic => pdf_rewritten,
        // The same sources must give the same answer whether latexmk ran
        // the engine or only repeated its verdict, so it comes from what is
        // on disk, not from what this run rewrote. On success the PDF is
        // current. After a failure it is if the last run wrote it, which
        // that run's log says: pdflatex keeps a PDF after an error it could
        // continue past, but an earlier run's PDF stays on disk after a
        // fatal error in the preamble.
        Engine::Latexmk => pdf.is_file() && (succeeded || wrote(report.output.as_ref(), &pdf)),
    };

    errors.append(&mut report.errors);
    warnings.append(&mut report.warnings);

    if engine == Engine::Tectonic {
        let console =
            parse_tectonic_console(&format!("{}\n{}", output.stderr, output.stdout_text()));
        errors.extend(console.errors);
        if !has_log {
            warnings.extend(console.engine_warnings);
        }
        warnings.extend(console.warnings);
    }

    // latexmk reruns BibTeX only when needed, so its last .blg still
    // describes the bibliography in use.
    if blg_current && let Ok(text) = read_text(&blg) {
        let bibtex = parse_blg(&text);
        errors.extend(bibtex.errors);
        warnings.extend(bibtex.warnings);
        for key in bibtex.missing_entries {
            if !report.undefined_citations.contains(&key) {
                report.undefined_citations.push(key);
            }
        }
    }

    for diagnostic in errors.iter_mut().chain(warnings.iter_mut()) {
        diagnostic.file = match diagnostic.file.take() {
            Some(file) => Some(resolve(&file, &project.root, main_dir)),
            // An error with no file surfaced at the end of the main file.
            None if diagnostic.severity == Severity::Error => Some(main.clone()),
            None => None,
        };
    }
    dedup(&mut errors);
    dedup(&mut warnings);

    // Turns keep The Lab's own runs apart, but not a biber started elsewhere.
    if biber_collided(&output.stderr) {
        errors.push(Diagnostic {
            severity: Severity::Error,
            file: None,
            line: None,
            message: BIBER_COLLIDED.to_string(),
        });
    }

    // The engine searched the same folders for biber (see `tools::run`).
    // Whatever else went wrong follows from this, so it comes first.
    if !succeeded && (biber || bcf.is_file()) && find_program("biber").is_none() {
        errors.insert(
            0,
            Diagnostic {
                severity: Severity::Error,
                file: None,
                line: None,
                message: BIBER_MISSING.to_string(),
            },
        );
    }

    // A run that stopped at an error did not get as far as resolving
    // cross-references: Tectonic gives up in its first pass, where every
    // reference and citation is still undefined. Reporting those would send
    // the reader after problems that are not there.
    if !errors.is_empty() {
        warnings.retain(|warning| !is_unfinished_run_artifact(&warning.message));
        report.undefined_references.clear();
        report.undefined_citations.clear();
    }

    if !succeeded && errors.is_empty() {
        let diagnostic = if report.no_pages {
            // Tectonic's own warning says the same thing less plainly.
            warnings.retain(|warning| !warning.message.starts_with("did not produce \""));
            Diagnostic {
                severity: Severity::Error,
                file: Some(main.clone()),
                line: None,
                message: NO_PAGES.to_string(),
            }
        } else {
            Diagnostic {
                severity: Severity::Error,
                file: None,
                line: None,
                message: unexplained_failure(engine, &output, limit),
            }
        };
        errors.push(diagnostic);
    }

    Ok(CompileResult {
        ok: succeeded && errors.is_empty() && pdf_current,
        engine,
        root: project.root.display().to_string(),
        main,
        pdf_path: pdf_current.then(|| pdf.display().to_string()),
        log_path: has_log.then(|| log.display().to_string()),
        duration_ms,
        errors,
        warnings,
        undefined_references: report.undefined_references,
        undefined_citations: report.undefined_citations,
        duplicate_labels: report.duplicate_labels,
        boxes: report.boxes,
    })
}

/// Where a compile of this project puts its PDF, whether or not it exists.
pub fn pdf_path(project: &Project) -> Result<PathBuf> {
    let main = project.require_main()?;
    let stem = Path::new(main)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");
    Ok(project
        .root
        .join(BUILD_DIRECTORY)
        .join(format!("{stem}.pdf")))
}

/// Whether a run whose log names `output` wrote `pdf`: the same file name,
/// and the same size where the log gives one.
fn wrote(output: Option<&OutputFile>, pdf: &Path) -> bool {
    let Some(output) = output else {
        return false;
    };
    let named = Path::new(&output.file).file_name() == pdf.file_name();
    let size = output
        .bytes
        .is_none_or(|bytes| pdf.metadata().is_ok_and(|m| m.len() == bytes));
    named && size
}

/// The error for a document that typeset nothing, which is what the Blank
/// template and a new .tex file start as.
pub const NO_PAGES: &str = "No pages of output. Add visible content to the document body.";

/// A file's modification time and size: enough to notice a rewrite.
type Stamp = (SystemTime, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = path.metadata().ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn timeout() -> Duration {
    std::env::var(TIMEOUT_VARIABLE)
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
}

/// The TeX program a document asks for in a `% !TEX program = ...` comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Program {
    PdfLaTeX,
    XeLaTeX,
    LuaLaTeX,
}

fn main_program(main_text: &str) -> Option<Program> {
    main_text.lines().take(20).find_map(|line| {
        let lower = line.trim().to_ascii_lowercase();
        if !lower.starts_with('%') || !lower.contains("!tex") || !lower.contains("program") {
            return None;
        }
        match lower.split_once('=')?.1.trim() {
            "pdflatex" => Some(Program::PdfLaTeX),
            "xelatex" => Some(Program::XeLaTeX),
            "lualatex" => Some(Program::LuaLaTeX),
            _ => None,
        }
    })
}

/// Whether the main file loads biblatex with biber, its default backend. A
/// preamble kept in a separate file is not looked at.
fn uses_biber(main_text: &str) -> bool {
    let is_bibtex_backend = |option: &str| {
        option.split_once('=').is_some_and(|(key, value)| {
            key.trim() == "backend" && value.trim().starts_with("bibtex")
        })
    };
    Source::new(main_text).commands().iter().any(|command| {
        matches!(command.name, "usepackage" | "RequirePackage")
            && command
                .argument
                .is_some_and(|names| names.split(',').any(|name| name.trim() == "biblatex"))
            && !command
                .options
                .iter()
                .flat_map(|options| options.split(','))
                .any(is_bibtex_backend)
    })
}

/// The file, in the temporary folder, that runs which may start biber take
/// turns on.
const BIBER_LOCK: &str = "thelab-biber.lock";

/// Waits until no other run that may start biber is going, in this process
/// or another, and returns the turn, which ends when it is dropped.
///
/// TeX Live 2026's biber 2.22 for macOS is packed with PAR. Each time it
/// starts it unpacks itself into `$TMPDIR/par-<user>/cache-<hash>/`, a
/// folder every biber of that user shares, and of two that start at the
/// same moment one fails with "cannot execute thin binary" and writes no
/// log. The app's live preview and Claude's compiles are separate `thelab`
/// processes, so the lock is a file in that same temporary folder. A turn
/// that cannot be had is not waited for.
fn biber_turn() -> Option<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join(BIBER_LOCK))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

const BIBER_COLLIDED: &str = "biber could not start because another biber was \
starting at the same moment (TeX Live's biber for macOS unpacks itself into a \
folder that every biber shares). This is not a fault in the document; compile again.";

const BIBER_MISSING: &str = "biber is not installed (or not on PATH), and this \
document needs it: biblatex uses biber unless it is loaded with backend=bibtex. \
biber comes with MacTeX and full TeX Live (https://tug.org/mactex/). With BasicTeX, \
run `sudo tlmgr install biber`; on Linux, install your distribution's biber package.";

/// Whether biber failed to start because another one was unpacking itself
/// at the same moment (see `biber_turn`). latexmk passes biber's stderr on.
fn biber_collided(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| line.contains("/biber: cannot execute thin binary"))
}

fn choose_engine(
    project: &Project,
    requested: Option<Engine>,
    program: Option<Program>,
    biber: bool,
) -> Result<(Engine, PathBuf)> {
    let requested = match (requested, &project.config.engine) {
        (Some(engine), _) => Some(engine),
        (None, Some(name)) => Some(Engine::parse(name)?),
        (None, None) => None,
    };
    if let Some(engine) = requested {
        return match find_program(engine.name()) {
            Some(path) => Ok((engine, path)),
            None => Err(Error::new(format!(
                "{} is not installed (or not on PATH). {}",
                engine.name(),
                install_hint(engine)
            ))),
        };
    }

    // Tectonic is built on XeTeX and cannot run a document that needs LuaLaTeX.
    // Its bundle also carries its own biblatex, which only the biber release
    // made for it can read (Tectonic 0.17's 3.17 against TeX Live 2026's
    // biber 2.22 fails), while latexmk uses a biblatex and biber that come
    // from the same TeX Live.
    let order = if program == Some(Program::LuaLaTeX) || biber {
        [Engine::Latexmk, Engine::Tectonic]
    } else {
        [Engine::Tectonic, Engine::Latexmk]
    };
    order
        .into_iter()
        .find_map(|engine| find_program(engine.name()).map(|path| (engine, path)))
        .ok_or_else(|| {
            Error::new(format!(
                "No TeX engine found. {} Alternatively: {}",
                install_hint(Engine::Tectonic),
                install_hint(Engine::Latexmk)
            ))
        })
}

fn install_hint(engine: Engine) -> &'static str {
    match engine {
        Engine::Tectonic => "Install Tectonic with `brew install tectonic`.",
        Engine::Latexmk => "latexmk comes with MacTeX or TeX Live (https://tug.org/mactex/).",
    }
}

/// Creates, inside the build directory, every directory that holds sources.
fn mirror_directories(project: &Project, main_dir: &str, build: &Path) {
    let prefix = if main_dir.is_empty() {
        String::new()
    } else {
        format!("{main_dir}/")
    };
    for file in &project.tex_files {
        let Some(relative) = file.strip_prefix(&prefix) else {
            continue;
        };
        if let Some((directory, _)) = relative.rsplit_once('/') {
            let _ = fs::create_dir_all(build.join(directory));
        }
    }
}

/// Maps a file name from a log onto a path relative to the project root.
/// Logs name files relative to the main file's directory, and Tectonic
/// leaves out the extension when `\input` did.
fn resolve(file: &str, root: &Path, main_dir: &str) -> String {
    if Path::new(file).is_absolute() {
        return match Path::new(file).strip_prefix(root) {
            Ok(inside) => inside.display().to_string(),
            Err(_) => file.to_string(),
        };
    }
    let in_main_dir = if main_dir.is_empty() {
        file.to_string()
    } else {
        format!("{main_dir}/{file}")
    };
    for candidate in [
        in_main_dir.clone(),
        format!("{in_main_dir}.tex"),
        file.to_string(),
        format!("{file}.tex"),
    ] {
        if root.join(&candidate).is_file() {
            return candidate;
        }
    }
    file.to_string()
}

/// Warnings that only say the run was not the last one it needed.
fn is_unfinished_run_artifact(message: &str) -> bool {
    // The kernel and natbib quote a key `like this', biblatex 'like this'.
    let undefined = message.ends_with("undefined")
        && ["Reference `", "Reference '", "Citation `", "Citation '"]
            .iter()
            .any(|start| message.contains(start));
    undefined
        || message.contains("Rerun to get")
        || message.contains("Label(s) may have changed")
        // biblatex asking for another round of biber, or of LaTeX after it.
        || message.contains("Please (re)run Biber")
        || message.contains("Please rerun LaTeX")
}

fn dedup(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen: Vec<Diagnostic> = Vec::new();
    diagnostics.retain(|d| {
        let fresh = !seen.contains(d);
        if fresh {
            seen.push(d.clone());
        }
        fresh
    });
}

/// A message for a failed run in which no error could be identified.
fn unexplained_failure(engine: Engine, output: &Output, limit: Duration) -> String {
    let Some(status) = output.status else {
        return format!(
            "{} was stopped after running for {} seconds (set {TIMEOUT_VARIABLE} to allow more)",
            engine.name(),
            limit.as_secs()
        );
    };
    let stdout = output.stdout_text();
    let tail: Vec<&str> = output
        .stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = tail[tail.len().saturating_sub(4)..].join(" | ");
    format!(
        "{} failed ({status}) without a recognizable error. Last output: {tail}",
        engine.name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_biber_fails_only_warnings_about_the_document_are_kept() {
        // latexmk's log after biber stopped at a syntax error in refs.bib.
        let log = parse_tex_log(include_str!(
            "../tests/fixtures/logs/latexmk/broken-bib.log"
        ));
        let kept: Vec<&str> = log
            .warnings
            .iter()
            .map(|w| w.message.as_str())
            .filter(|message| !is_unfinished_run_artifact(message))
            .collect();
        assert_eq!(kept, ["Empty bibliography"]);
    }

    #[test]
    fn undefined_keys_in_either_quote_style_are_artifacts_of_an_unfinished_run() {
        for message in [
            "Reference `sec:missing' on page 1 undefined",
            "Package natbib Warning: Citation `nokey2020' on page 1 undefined",
            "Citation 'nasa2025moon' on page 1 undefined",
            "Label(s) may have changed. Rerun to get cross-references right",
        ] {
            assert!(is_unfinished_run_artifact(message), "{message}");
        }
        assert!(!is_unfinished_run_artifact("Empty bibliography"));
    }

    #[test]
    fn a_log_that_names_the_pdf_and_its_size_wrote_it() {
        let pdf = std::env::temp_dir().join(format!("thelab-wrote-{}.pdf", std::process::id()));
        fs::write(&pdf, b"%PDF-1.5 twelve").unwrap();
        let output = |file: &str, bytes| OutputFile {
            file: file.to_string(),
            pages: 1,
            bytes,
        };
        let name = pdf.file_name().unwrap().to_str().unwrap().to_string();
        assert!(wrote(
            Some(&output(&format!("build/{name}"), Some(15))),
            &pdf
        ));
        assert!(wrote(Some(&output(&name, None)), &pdf));
        // Another size: the PDF on disk is not the one that run wrote.
        assert!(!wrote(Some(&output(&name, Some(16))), &pdf));
        assert!(!wrote(Some(&output("build/main.xdv", Some(15))), &pdf));
        assert!(!wrote(None, &pdf));
        fs::remove_file(&pdf).unwrap();
    }

    #[test]
    fn biber_is_biblatex_s_default_backend() {
        assert!(uses_biber("\\usepackage{biblatex}"));
        assert!(uses_biber(
            "\\usepackage[style=ieee,backend=biber]{biblatex}"
        ));
        assert!(uses_biber("\\usepackage{amsmath, biblatex}"));
        assert!(uses_biber("\\RequirePackage[sorting=none]{biblatex}"));
    }

    #[test]
    fn biblatex_with_a_bibtex_backend_or_no_biblatex_is_not_biber() {
        assert!(!uses_biber(
            "\\usepackage[style=ieee, backend = bibtex]{biblatex}"
        ));
        assert!(!uses_biber("\\usepackage[backend=bibtex8]{biblatex}"));
        assert!(!uses_biber(
            "\\usepackage[numbers]{natbib}\n\\bibliography{refs}"
        ));
        assert!(!uses_biber(
            "% \\usepackage{biblatex}\n\\usepackage{natbib}"
        ));
    }

    #[test]
    fn a_biber_turn_keeps_others_waiting_until_it_ends() {
        // The tests in one process rely on this as much as separate
        // processes do: a second opening of the file is a second claimant.
        let turn = biber_turn().expect("a turn");
        let other = fs::OpenOptions::new()
            .append(true)
            .open(std::env::temp_dir().join(BIBER_LOCK))
            .unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
        drop(turn);
        other.try_lock().expect("free once the turn has ended");
    }

    #[test]
    fn a_biber_that_could_not_start_is_recognized() {
        // latexmk's stderr when it happened (TeX Live 2026, biber 2.22 for
        // macOS), with the user's folders shortened.
        let collided = "Latexmk: Using biber to make bibliography file(s).
/var/folders/xx/T/par-75736572/cache-bcf1e1e0/thin/biber: cannot execute thin binary \
/var/folders/xx/T/par-75736572/cache-bcf1e1e0/thin/biber (errno=8)
Latexmk: Could not open biber log file 'build/main.blg'
";
        assert!(biber_collided(collided));
        assert!(!biber_collided(
            "Latexmk: Using biber to make bibliography file(s).\n\
             Latexmk: Could not open biber log file 'build/main.blg'\n"
        ));
    }
}
