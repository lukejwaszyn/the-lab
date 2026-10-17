use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use thelab_core::compile::{CompileOptions, CompileResult, Engine, BUILD_DIRECTORY, NO_PAGES};
use thelab_core::logparse::{Diagnostic, Severity};
use tokio::sync::{Mutex, Semaphore};

const MAX_CONCURRENT: usize = 3;

/// Windows CREATE_NO_WINDOW flag to prevent console windows from flashing
/// when spawning TeXLive child processes from the GUI app.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Where the last compile of a project left its SyncTeX data.
pub(crate) struct BuildInfo {
    /// The copy of the project the engine ran on (see `persistent_build_dir`).
    work_dir: PathBuf,
    /// The folder the engine ran in: relative SyncTeX inputs start here.
    engine_dir: PathBuf,
    main_file_name: String,
}

#[derive(Clone)]
pub struct LatexCompilerState {
    last_builds: Arc<Mutex<HashMap<String, BuildInfo>>>,
    /// The last compile's report per project, for the problems panel.
    last_reports: Arc<Mutex<HashMap<String, CompileReport>>>,
    /// Per-project locks to prevent concurrent compilations on the same build directory.
    project_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    semaphore: Arc<Semaphore>,
}

impl Default for LatexCompilerState {
    fn default() -> Self {
        Self {
            last_builds: Arc::new(Mutex::new(HashMap::new())),
            last_reports: Arc::new(Mutex::new(HashMap::new())),
            project_locks: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }
}

#[derive(serde::Serialize)]
pub struct SynctexResult {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

// --- Engine choice ---

/// The engine menu of the PDF preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineChoice {
    /// thelab chooses per document: latexmk for biblatex with biber and for
    /// LuaLaTeX, Tectonic otherwise, whichever is installed.
    Auto,
    /// The installed `tectonic` program.
    Tectonic,
    /// TeX Live through `latexmk`.
    TexLive,
}

impl EngineChoice {
    pub(crate) fn parse(name: Option<&str>) -> Result<Self, String> {
        match name.map(|n| n.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("auto") => Ok(Self::Auto),
            Some("tectonic") => Ok(Self::Tectonic),
            Some("texlive") | Some("latexmk") => Ok(Self::TexLive),
            Some(other) => Err(format!(
                "Unknown engine \"{other}\": expected auto, tectonic or texlive"
            )),
        }
    }

    /// The engine to ask thelab for; `None` lets it choose (and lets a
    /// project's `thelab.toml` choose first).
    fn engine(self) -> Option<Engine> {
        match self {
            Self::Auto => None,
            Self::Tectonic => Some(Engine::Tectonic),
            Self::TexLive => Some(Engine::Latexmk),
        }
    }
}

// --- Compile report ---

/// One error or warning of a compile, for the problems panel.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileProblem {
    /// `error` or `warning`.
    pub severity: &'static str,
    /// Relative to the project for its own files (a `.bib` file included),
    /// absolute for others; absent when the engine did not say.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

impl CompileProblem {
    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: "error",
            file: None,
            line: None,
            message: message.into(),
        }
    }

    fn from_diagnostic(diagnostic: Diagnostic) -> Self {
        Self {
            severity: match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            },
            file: diagnostic.file,
            line: diagnostic.line,
            message: diagnostic.message,
        }
    }

    /// `file:line: message`, or as much of it as is known.
    fn describe(&self) -> String {
        // Messages may span lines (biber's do); the summary has one per line.
        let message = self
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        match (&self.file, self.line) {
            (Some(file), Some(line)) => format!("{file}:{line}: {message}"),
            (Some(file), None) => format!("{file}: {message}"),
            _ => message,
        }
    }
}

/// What a compile found: the frontend reads it with `latex_compile_report`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileReport {
    pub project_dir: String,
    pub main_file: String,
    /// `tectonic` or `latexmk`; absent when no engine could be started.
    pub engine: Option<&'static str>,
    /// The engine succeeded with no errors and wrote a current PDF.
    pub ok: bool,
    /// A PDF of this run exists, even if the run also reported errors.
    pub has_pdf: bool,
    pub duration_ms: u64,
    pub errors: Vec<CompileProblem>,
    pub warnings: Vec<CompileProblem>,
}

impl CompileReport {
    fn failed(project_dir: &Path, main_file: &str, message: impl Into<String>) -> Self {
        Self {
            project_dir: project_dir.to_string_lossy().to_string(),
            main_file: main_file.to_string(),
            engine: None,
            ok: false,
            has_pdf: false,
            duration_ms: 0,
            errors: vec![CompileProblem::error(message)],
            warnings: Vec::new(),
        }
    }

    fn from_result(project_dir: &Path, main_file: &str, result: CompileResult) -> Self {
        Self {
            project_dir: project_dir.to_string_lossy().to_string(),
            main_file: main_file.to_string(),
            engine: Some(result.engine.name()),
            ok: result.ok,
            has_pdf: result.pdf_path.is_some(),
            duration_ms: result.duration_ms,
            errors: result
                .errors
                .into_iter()
                .map(CompileProblem::from_diagnostic)
                .collect(),
            warnings: result
                .warnings
                .into_iter()
                .map(CompileProblem::from_diagnostic)
                .collect(),
        }
    }

    /// The message of a compile that left no PDF: one error per line, which
    /// the PDF preview lists when it has no report to read.
    fn failure_message(&self) -> String {
        let backend = self.engine.unwrap_or("no engine");
        let errors: Vec<String> = self.errors.iter().map(CompileProblem::describe).collect();
        let details = if errors.is_empty() {
            "No PDF was generated.".to_string()
        } else {
            errors.join("\n")
        };
        format!("Compilation failed ({backend})\n\n{details}")
    }
}

/// What `compile_project` produced.
pub(crate) struct CompileOutcome {
    pub report: CompileReport,
    /// The PDF of this run, if it wrote one.
    pub pdf: Option<Vec<u8>>,
    /// Where the SyncTeX data is, once an engine has run.
    pub build: Option<BuildInfo>,
}

// --- Helpers ---

#[derive(Debug, PartialEq)]
enum TexEngine {
    Latex,
    XeLaTeX,
    LuaLaTeX,
}

/// Detect TeX engine from `% !TEX program = <engine>` magic comment in the first 20 lines.
fn detect_tex_engine(content: &str) -> Option<TexEngine> {
    for line in content.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('%') {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix("!TEX") {
                let rest = rest.trim();
                if let Some(rest) = rest.strip_prefix("program") {
                    let rest = rest.trim();
                    if let Some(rest) = rest.strip_prefix('=') {
                        let engine = rest.trim().to_lowercase();
                        return match engine.as_str() {
                            "xelatex" => Some(TexEngine::XeLaTeX),
                            "lualatex" => Some(TexEngine::LuaLaTeX),
                            "pdflatex" | "latex" => Some(TexEngine::Latex),
                            _ => None,
                        };
                    }
                }
            }
        }
    }
    None
}

/// Resolve a TeXLive engine binary to its full path.
/// GUI apps on macOS lack the user's shell PATH, so we check standard
/// TeXLive installation locations and fall back to a login-shell query.
fn find_texlive_binary(name: &str) -> Result<PathBuf, String> {
    // 1. Try PATH (works when launched from terminal)
    if let Ok(path) = which::which(name) {
        return Ok(path);
    }

    // 2. Check standard TeXLive locations
    #[cfg(not(target_os = "windows"))]
    {
        let standard_paths = [
            format!("/Library/TeX/texbin/{}", name),
            format!("/usr/local/texlive/2025/bin/universal-darwin/{}", name),
            format!("/usr/local/texlive/2024/bin/universal-darwin/{}", name),
            format!("/usr/local/texlive/2025/bin/x86_64-linux/{}", name),
            format!("/usr/local/texlive/2024/bin/x86_64-linux/{}", name),
            format!("/opt/homebrew/bin/{}", name),
            format!("/usr/bin/{}", name),
        ];
        for path_str in &standard_paths {
            let p = PathBuf::from(path_str);
            if p.exists() {
                return Ok(p);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let standard_paths = [
            format!("C:\\texlive\\2025\\bin\\windows\\{}.exe", name),
            format!("C:\\texlive\\2024\\bin\\windows\\{}.exe", name),
        ];
        for path_str in &standard_paths {
            let p = PathBuf::from(path_str);
            if p.exists() {
                return Ok(p);
            }
        }
    }

    // 3. macOS: ask login shell for PATH
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("/bin/zsh")
            .args(["-l", "-c", &format!("which {}", name)])
            .output()
        {
            if output.status.success() {
                let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let p = PathBuf::from(&resolved);
                if p.exists() {
                    return Ok(p);
                }
            }
        }
    }

    Err(format!(
        "{} not found. Install TeXLive or add it to your PATH.",
        name
    ))
}

/// Whether a directory of the project is never copied: hidden ones (.git,
/// .claudeprism, .prism itself) and, at the top, the project's own `build/`,
/// which thelab's CLI and MCP server write to. The copy's `build/` is the
/// app's, and a PDF or latexmk database from there must not land in it.
fn skip_directory(name: &str, at_top: bool) -> bool {
    name.starts_with('.') || (at_top && name == BUILD_DIRECTORY)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_tree(src, dst, true)
}

fn copy_tree(src: &Path, dst: &Path, at_top: bool) -> std::io::Result<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            if skip_directory(&entry.file_name().to_string_lossy(), at_top) {
                continue;
            }
            copy_tree(&src_path, &dst_path, false)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Sync only source files (.tex, .bib, .sty, .cls, .bst, images, .pdf figures) from project to build dir.
/// Skips build artifacts (.aux, .log, .toc, .synctex.gz, etc.) to preserve them.
/// Note: .pdf is NOT skipped — figure PDFs must be synced. The engine writes its PDF to the copy's `build/`.
fn sync_source_files(src: &Path, dst: &Path) -> std::io::Result<()> {
    sync_tree(src, dst, true)
}

fn sync_tree(src: &Path, dst: &Path, at_top: bool) -> std::io::Result<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);
        if src_path.is_dir() {
            let name = file_name.to_string_lossy();
            if skip_directory(&name, at_top)
                || matches!(name.as_ref(), "node_modules" | "target" | "dist")
            {
                continue;
            }
            sync_tree(&src_path, &dst_path, false)?;
        } else {
            let ext = src_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_artifact = matches!(
                ext,
                "aux"
                    | "log"
                    | "toc"
                    | "lof"
                    | "lot"
                    | "out"
                    | "nav"
                    | "snm"
                    | "vrb"
                    | "bbl"
                    | "blg"
                    | "fls"
                    | "fdb_latexmk"
                    | "synctex"
                    | "idx"
                    | "ind"
                    | "ilg"
                    | "glo"
                    | "gls"
                    | "glg"
                    | "fmt"
                    | "xdv"
            );
            let is_synctex = src_path.to_string_lossy().ends_with(".synctex.gz");
            if !is_artifact && !is_synctex {
                // Cloud storage (Dropbox/iCloud) may keep files as online-only
                // placeholders with 0 bytes. Reading the file forces a download.
                let metadata = std::fs::metadata(&src_path)?;

                if metadata.len() > 0 {
                    if let Ok(dst_meta) = std::fs::metadata(&dst_path) {
                        if metadata.len() == dst_meta.len() {
                            if let (Ok(src_m), Ok(dst_m)) =
                                (metadata.modified(), dst_meta.modified())
                            {
                                if src_m == dst_m {
                                    continue;
                                }
                            }
                        }
                    }
                }

                if metadata.len() == 0 {
                    // Attempt to materialize the file by reading it
                    let data = std::fs::read(&src_path)?;
                    if !data.is_empty() {
                        std::fs::write(&dst_path, &data)?;
                    } else {
                        std::fs::copy(&src_path, &dst_path)?;
                    }
                } else {
                    std::fs::copy(&src_path, &dst_path)?;
                }
            }
        }
    }
    Ok(())
}

/// Persistent build directory inside the project.
/// Stored in `<project>/.prism/build/` — hidden from file tree (dot-prefix is filtered).
/// It holds a copy of the project's sources; thelab writes the PDF, logs and
/// SyncTeX data to the copy's own `build/`.
fn persistent_build_dir(project_dir: &str) -> PathBuf {
    PathBuf::from(project_dir).join(".prism").join("build")
}

/// Where a compile leaves the project's PDF, log and SyncTeX data: the
/// `build/` thelab writes inside the copy. The project picker looks there
/// too, through `src/lib/compiled-pdf-dir.json`.
pub(crate) fn compiled_pdf_dir(project_dir: &Path) -> PathBuf {
    persistent_build_dir(&project_dir.to_string_lossy()).join(BUILD_DIRECTORY)
}

/// What ClaudePrism put before `\begin{document}` to compile a document
/// with nothing in its body: a blank page instead of an error.
const BLANK_PAGE: &str = "\\AtEndDocument{\\null}";

/// Whether a compile failed only because the document has no pages: the
/// Blank template and every new .tex file, until something is written.
fn only_no_pages(result: &CompileResult) -> bool {
    result.pdf_path.is_none()
        && !result.errors.is_empty()
        && result.errors.iter().all(|error| error.message == NO_PAGES)
}

/// Adds `BLANK_PAGE` to the copy's main file, on the line of
/// `\begin{document}` so that line numbers stay right. The project's own
/// file is untouched, and the next sync puts it back over the copy, whose
/// size no longer matches. False when there is no `\begin{document}`
/// outside comments to put it before.
fn add_blank_page(main_path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(main_path) else {
        return false;
    };
    let source = thelab_core::tex::Source::new(&text);
    let Some(begin) = source
        .commands()
        .into_iter()
        .find(|command| command.name == "begin" && command.argument == Some("document"))
    else {
        return false;
    };
    let patched = format!(
        "{}{BLANK_PAGE}{}",
        &text[..begin.start],
        &text[begin.start..]
    );
    std::fs::write(main_path, patched).is_ok()
}

/// Removes `.` and `..` from a path without touching the disk.
fn clean_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other),
        }
    }
    cleaned
}

/// A SyncTeX input as the frontend's file list names it: relative to the
/// project for files of the copy, otherwise as written. Engines write
/// absolute paths (pdfTeX: `/copy/./main.tex`) or paths relative to the
/// folder they ran in (`engine_dir`).
fn project_relative(input: &str, work_dir: &Path, engine_dir: &Path) -> String {
    let path = Path::new(input);
    let absolute = if path.is_absolute() {
        clean_path(path)
    } else {
        clean_path(&engine_dir.join(path))
    };
    match absolute.strip_prefix(clean_path(work_dir)) {
        Ok(relative) => relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => input.to_string(),
    }
}

// --- Thread priority ---

/// Lower the current thread's scheduling priority so CPU-heavy compilation
/// does not starve the WebView's main thread (and thus the UI / typing).
fn lower_thread_priority() {
    #[cfg(target_os = "macos")]
    {
        // QOS_CLASS_UTILITY (0x11) — lower than default, appropriate for long-running work.
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        unsafe { pthread_set_qos_class_self_np(0x11, 0) };
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        extern "C" {
            fn nice(inc: i32) -> i32;
        }
        unsafe { nice(10) };
    }
}

// --- Compilation through thelab-core ---

/// Compiles a project the way the app does: the sources are copied into
/// `<project>/.prism/build/`, and `thelab_core::compile::compile` runs the
/// engine on that copy, which writes to the copy's `build/`. Blocking.
///
/// This is everything `compile_latex` does apart from the app's state, so
/// tests (and a driver) call it directly. A compile that could not run is a
/// report with one error, never an `Err`.
pub(crate) fn compile_project(
    project_dir: &Path,
    main_file: &str,
    choice: EngineChoice,
) -> CompileOutcome {
    let failed = |message: String| CompileOutcome {
        report: CompileReport::failed(project_dir, main_file, message),
        pdf: None,
        build: None,
    };

    let work_dir = persistent_build_dir(&project_dir.to_string_lossy());
    let synced = if work_dir.exists() {
        sync_source_files(project_dir, &work_dir)
            .map_err(|e| format!("Failed to sync project: {e}"))
    } else {
        std::fs::create_dir_all(&work_dir)
            .and_then(|()| copy_dir_recursive(project_dir, &work_dir))
            .map_err(|e| format!("Failed to copy project: {e}"))
    };
    if let Err(message) = synced {
        return failed(message);
    }

    let main_path = work_dir.join(main_file);
    if !main_path.is_file() {
        return failed(format!(
            "No .tex file found: \"{main_file}\". Create a document.tex or main.tex file to compile."
        ));
    }
    let main_text = std::fs::read_to_string(&main_path).unwrap_or_default();
    if choice == EngineChoice::Tectonic
        && detect_tex_engine(&main_text) == Some(TexEngine::LuaLaTeX)
    {
        return failed(
            "This document asks for LuaLaTeX (% !TEX program = lualatex), which Tectonic \
             cannot run. Choose Auto or TeX Live in the engine menu."
                .to_string(),
        );
    }

    let options = CompileOptions {
        main: Some(main_file.replace('\\', "/")),
        engine: choice.engine(),
    };
    let mut result = match thelab_core::compile::compile(&work_dir, &options) {
        Ok(result) => result,
        Err(error) => return failed(error.to_string()),
    };
    // An empty body shows as a blank page, as it did in ClaudePrism. thelab
    // itself (Claude's compile tool) keeps saying that there are no pages.
    if only_no_pages(&result) && add_blank_page(&main_path) {
        if let Ok(retried) = thelab_core::compile::compile(&work_dir, &options) {
            result = retried;
        }
    }

    // thelab resolves the copy to its real path; SyncTeX inputs use that.
    let root = PathBuf::from(&result.root);
    let main_dir = Path::new(&result.main)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let build = BuildInfo {
        engine_dir: match result.engine {
            // latexmk runs in the main file's folder, Tectonic in the root.
            Engine::Latexmk => root.join(&main_dir),
            Engine::Tectonic => root.clone(),
        },
        work_dir: root,
        main_file_name: Path::new(&result.main)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string(),
    };

    let pdf_path = result.pdf_path.clone();
    let mut report = CompileReport::from_result(project_dir, main_file, result);
    let pdf = match pdf_path.map(std::fs::read) {
        Some(Ok(bytes)) => Some(bytes),
        Some(Err(e)) => {
            report.has_pdf = false;
            report.ok = false;
            report
                .errors
                .push(CompileProblem::error(format!("Failed to read PDF: {e}")));
            None
        }
        None => None,
    };
    CompileOutcome {
        report,
        pdf,
        build: Some(build),
    }
}

// --- SyncTeX Native Parser ---

struct SynctexNode {
    tag: u32,
    line: u32,
    h: f64, // PDF points
    v: f64, // PDF points
}

/// Parse synctex data and find the source location closest to (target_x, target_y) on target_page.
fn parse_synctex_data(
    data: &str,
    target_page: u32,
    target_x: f64,
    target_y: f64,
) -> Option<(String, u32, u32)> {
    let mut inputs: HashMap<u32, String> = HashMap::new();
    let mut magnification: f64 = 1000.0;
    let mut unit: f64 = 1.0;
    let mut x_offset: f64 = 0.0;
    let mut y_offset: f64 = 0.0;

    let mut in_content = false;
    let mut on_target_page = false;
    let mut nodes: Vec<SynctexNode> = Vec::new();

    for raw_line in data.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if !in_content {
            if let Some(rest) = line.strip_prefix("Input:") {
                if let Some(colon_pos) = rest.find(':') {
                    if let Ok(tag) = rest[..colon_pos].parse::<u32>() {
                        inputs.insert(tag, rest[colon_pos + 1..].to_string());
                    }
                }
            } else if let Some(rest) = line.strip_prefix("Magnification:") {
                magnification = rest.trim().parse().unwrap_or(1000.0);
            } else if let Some(rest) = line.strip_prefix("Unit:") {
                unit = rest.trim().parse().unwrap_or(1.0);
            } else if let Some(rest) = line.strip_prefix("X Offset:") {
                x_offset = rest.trim().parse().unwrap_or(0.0);
            } else if let Some(rest) = line.strip_prefix("Y Offset:") {
                y_offset = rest.trim().parse().unwrap_or(0.0);
            } else if line == "Content:" {
                in_content = true;
            }
            continue;
        }

        // Content section
        if line.starts_with("Postamble:") {
            break;
        }

        let first_byte = match line.as_bytes().first() {
            Some(b) => *b,
            None => continue,
        };
        match first_byte {
            b'{' => {
                let page: u32 = line.get(1..).and_then(|s| s.parse().ok()).unwrap_or(0);
                on_target_page = page == target_page;
            }
            b'}' => {
                on_target_page = false;
            }
            // Box/node records: [, (, h, v, k, x, g, $
            b'[' | b'(' | b'h' | b'v' | b'k' | b'x' | b'g' | b'$' if on_target_page => {
                // Convert synctex internal units to PDF points (bp)
                // 1 TeX pt = 65536 sp; 1 inch = 72.27 TeX pt = 72 PDF bp
                let factor = unit * magnification / (1000.0 * 65536.0) * 72.0 / 72.27;
                if let Some(node) = line
                    .get(1..)
                    .and_then(|s| parse_synctex_node(s, factor, x_offset, y_offset))
                {
                    nodes.push(node);
                }
            }
            _ => {}
        }
    }

    if nodes.is_empty() {
        return None;
    }

    // Find closest node to (target_x, target_y)
    let mut best_idx = 0;
    let mut best_dist = f64::MAX;
    for (i, node) in nodes.iter().enumerate() {
        let dx = node.h - target_x;
        let dy = node.v - target_y;
        let dist = dx * dx + dy * dy;
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }

    let best = nodes.get(best_idx)?;
    let filename = inputs.get(&best.tag)?.clone();
    Some((filename, best.line, 0))
}

/// Parse a synctex node record (after stripping the type character).
/// Format: `<tag>,<line>,<column>:<h>,<v>[:<W>,<H>,<D>]`
fn parse_synctex_node(s: &str, factor: f64, x_offset: f64, y_offset: f64) -> Option<SynctexNode> {
    let colon_parts: Vec<&str> = s.splitn(4, ':').collect();
    if colon_parts.len() < 2 {
        return None;
    }

    // Parse tag and line (ignore column)
    let first_part = colon_parts.first()?;
    let tlc: Vec<&str> = first_part.splitn(3, ',').collect();
    if tlc.len() < 2 {
        return None;
    }
    let tag: u32 = tlc.first()?.parse().ok()?;
    let line: u32 = tlc.get(1)?.parse().ok()?;

    // Parse h, v coordinates
    let second_part = colon_parts.get(1)?;
    let hv: Vec<&str> = second_part.splitn(2, ',').collect();
    if hv.len() < 2 {
        return None;
    }
    let h_raw: i64 = hv.first()?.parse().ok()?;
    let v_raw: i64 = hv.get(1)?.parse().ok()?;

    let h = h_raw as f64 * factor + x_offset;
    let v = v_raw as f64 * factor + y_offset;

    Some(SynctexNode { tag, line, h, v })
}

// --- Tauri Commands ---

#[derive(serde::Serialize)]
pub struct TexliveStatus {
    pub available: bool,
    pub engines: Vec<String>,
    pub version: Option<String>,
}

#[tauri::command]
pub fn detect_texlive() -> TexliveStatus {
    let engines_to_check = ["pdflatex", "xelatex", "lualatex"];
    let mut found_engines = Vec::new();

    for name in &engines_to_check {
        if find_texlive_binary(name).is_ok() {
            found_engines.push(name.to_string());
        }
    }

    let version = find_texlive_binary("pdflatex").ok().and_then(|path| {
        let mut cmd = std::process::Command::new(&path);
        cmd.arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.output().ok().and_then(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().next().map(|l| l.to_string())
        })
    });

    TexliveStatus {
        available: !found_engines.is_empty(),
        engines: found_engines,
        version,
    }
}

#[tauri::command]
pub async fn compile_latex(
    state: tauri::State<'_, LatexCompilerState>,
    project_dir: String,
    main_file: String,
    engine: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    // Acquire semaphore permit (non-blocking)
    let _permit = state
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| "Server busy, too many concurrent compilations".to_string())?;

    // Acquire per-project lock to prevent concurrent compilations on the same build dir.
    let project_lock = {
        let mut locks = state.project_locks.lock().await;
        locks
            .entry(project_dir.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _project_guard = project_lock.lock().await;

    let choice = EngineChoice::parse(engine.as_deref())?;
    let t0 = std::time::Instant::now();

    // thelab runs the engine as a child process and waits for it: blocking.
    let outcome = {
        let project_dir = project_dir.clone();
        let main_file = main_file.clone();
        tokio::task::spawn_blocking(move || {
            lower_thread_priority();
            compile_project(Path::new(&project_dir), &main_file, choice)
        })
        .await
        .map_err(|e| format!("Compilation task panicked: {}", e))?
    };

    let CompileOutcome { report, pdf, build } = outcome;
    eprintln!(
        "[latex] +{:.0}ms {} (choice={:?}, engine={}, ok={}, errors={}, warnings={}, pdf={})",
        t0.elapsed().as_millis(),
        main_file,
        choice,
        report.engine.unwrap_or("none"),
        report.ok,
        report.errors.len(),
        report.warnings.len(),
        pdf.as_ref().map_or(0, Vec::len) / 1024,
    );

    if let Some(build) = build {
        state
            .last_builds
            .lock()
            .await
            .insert(project_dir.clone(), build);
    }
    let failure = pdf.is_none().then(|| report.failure_message());
    state.last_reports.lock().await.insert(project_dir, report);

    // A PDF is shown even when the run also reported errors (as before);
    // the errors reach the problems panel through the report.
    match (pdf, failure) {
        (Some(bytes), _) => Ok(tauri::ipc::Response::new(bytes)),
        (None, failure) => Err(failure.unwrap_or_else(|| "Compilation failed".to_string())),
    }
}

/// The report of the last compile of a project in this session: its
/// errors and warnings by file and line, and the engine that ran.
#[tauri::command]
pub async fn latex_compile_report(
    state: tauri::State<'_, LatexCompilerState>,
    project_dir: String,
) -> Result<Option<CompileReport>, String> {
    Ok(state.last_reports.lock().await.get(&project_dir).cloned())
}

#[tauri::command]
pub async fn synctex_edit(
    state: tauri::State<'_, LatexCompilerState>,
    project_dir: String,
    page: u32,
    x: f64,
    y: f64,
) -> Result<SynctexResult, String> {
    let builds = state.last_builds.lock().await;
    let build = builds
        .get(&project_dir)
        .ok_or("No build found for this project")?;

    // thelab writes the SyncTeX data next to the PDF, in the copy's build/.
    let output_dir = build.work_dir.join(BUILD_DIRECTORY);
    let synctex_gz = output_dir.join(format!("{}.synctex.gz", build.main_file_name));
    let synctex_plain = output_dir.join(format!("{}.synctex", build.main_file_name));

    let work_dir = build.work_dir.clone();
    let engine_dir = build.engine_dir.clone();
    drop(builds); // Release lock before I/O

    // Read, decompress, and parse synctex data (blocking I/O + CPU work → offload)
    let (file, line, column) = tokio::task::spawn_blocking(move || {
        let synctex_data = if synctex_gz.exists() {
            let compressed = std::fs::read(&synctex_gz)
                .map_err(|e| format!("Failed to read synctex.gz: {}", e))?;
            let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
            let mut data = String::new();
            decoder
                .read_to_string(&mut data)
                .map_err(|e| format!("Failed to decompress synctex: {}", e))?;
            Ok::<_, String>(data)
        } else if synctex_plain.exists() {
            std::fs::read_to_string(&synctex_plain)
                .map_err(|e| format!("Failed to read synctex: {}", e))
        } else {
            Err("No synctex data found. Recompile with synctex enabled.".to_string())
        }?;

        parse_synctex_data(&synctex_data, page, x, y)
            .ok_or_else(|| "Could not resolve source location".to_string())
    })
    .await
    .map_err(|e| format!("Synctex task panicked: {}", e))??;

    let file = project_relative(&file, &work_dir, &engine_dir);
    Ok(SynctexResult { file, line, column })
}

/// Clear in-memory build state on app exit.
/// Persistent build directories are intentionally kept for fast restart.
pub async fn cleanup_all_builds(state: &LatexCompilerState) {
    let mut builds = state.last_builds.lock().await;
    builds.clear();
    state.last_reports.lock().await.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- persistent_build_dir ---

    #[test]
    fn test_persistent_build_dir() {
        let dir = persistent_build_dir("/Users/dev/my-project");
        assert_eq!(dir, PathBuf::from("/Users/dev/my-project/.prism/build"));
    }

    // --- parse_synctex_node ---

    #[test]
    fn test_parse_synctex_node_basic() {
        // Format: tag,line,column:h,v
        let node = parse_synctex_node("1,42,0:1000,2000", 1.0, 0.0, 0.0);
        assert!(node.is_some());
        let node = node.unwrap();
        assert_eq!(node.tag, 1);
        assert_eq!(node.line, 42);
        assert_eq!(node.h, 1000.0);
        assert_eq!(node.v, 2000.0);
    }

    #[test]
    fn test_parse_synctex_node_with_dimensions() {
        // Format: tag,line,column:h,v:W,H,D
        let node = parse_synctex_node("3,10,0:500,600:100,20,5", 1.0, 0.0, 0.0);
        assert!(node.is_some());
        let node = node.unwrap();
        assert_eq!(node.tag, 3);
        assert_eq!(node.line, 10);
    }

    #[test]
    fn test_parse_synctex_node_with_offset() {
        let node = parse_synctex_node("1,1,0:0,0", 1.0, 10.0, 20.0);
        let node = node.unwrap();
        assert_eq!(node.h, 10.0); // 0 * 1.0 + 10.0
        assert_eq!(node.v, 20.0); // 0 * 1.0 + 20.0
    }

    #[test]
    fn test_parse_synctex_node_invalid_missing_colon() {
        assert!(parse_synctex_node("1,1,0", 1.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn test_parse_synctex_node_invalid_missing_comma() {
        assert!(parse_synctex_node("1:100,200", 1.0, 0.0, 0.0).is_none());
    }

    // --- parse_synctex_data ---

    #[test]
    fn test_parse_synctex_data_basic() {
        let data = "\
SyncTeX Version:1
Input:1:./main.tex
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h1,5,0:1000,2000:500,100,0
}1
Postamble:
";
        let result = parse_synctex_data(data, 1, 50.0, 50.0);
        assert!(result.is_some());
        let (file, line, _col) = result.unwrap();
        assert_eq!(file, "./main.tex");
        assert_eq!(line, 5);
    }

    #[test]
    fn test_parse_synctex_data_wrong_page() {
        let data = "\
Input:1:./main.tex
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h1,5,0:1000,2000
}1
Postamble:
";
        // Looking for page 2 but data only has page 1
        let result = parse_synctex_data(data, 2, 50.0, 50.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_synctex_data_closest_node() {
        let data = "\
Input:1:./main.tex
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h1,10,0:0,0
h1,20,0:100000000,100000000
}1
Postamble:
";
        // (0, 0) is closer to the first node
        let result = parse_synctex_data(data, 1, 0.0, 0.0);
        assert!(result.is_some());
        let (_, line, _) = result.unwrap();
        assert_eq!(line, 10);
    }

    #[test]
    fn test_parse_synctex_data_empty() {
        let result = parse_synctex_data("", 1, 0.0, 0.0);
        assert!(result.is_none());
    }

    // --- parse_synctex_node additional edge cases ---

    #[test]
    fn test_parse_synctex_node_negative_coordinates() {
        let node = parse_synctex_node("1,1,0:-500,300", 1.0, 0.0, 0.0);
        assert!(node.is_some());
        let n = node.unwrap();
        assert_eq!(n.h, -500.0);
        assert_eq!(n.v, 300.0);
    }

    #[test]
    fn test_parse_synctex_node_factor_scaling() {
        // factor=2.0 should double the coordinates
        let node = parse_synctex_node("1,1,0:100,200", 2.0, 0.0, 0.0);
        let n = node.unwrap();
        assert_eq!(n.h, 200.0);
        assert_eq!(n.v, 400.0);
    }

    #[test]
    fn test_parse_synctex_node_zero_tag_and_line() {
        let node = parse_synctex_node("0,0,0:0,0", 1.0, 0.0, 0.0);
        let n = node.unwrap();
        assert_eq!(n.tag, 0);
        assert_eq!(n.line, 0);
    }

    // --- parse_synctex_data additional edge cases ---

    #[test]
    fn test_parse_synctex_data_multiple_inputs() {
        let data = "\
Input:1:./main.tex
Input:2:./chapter1.tex
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h2,15,0:500,500
}1
Postamble:
";
        let result = parse_synctex_data(data, 1, 0.0, 0.0);
        assert!(result.is_some());
        let (file, line, _) = result.unwrap();
        assert_eq!(file, "./chapter1.tex");
        assert_eq!(line, 15);
    }

    #[test]
    fn test_parse_synctex_data_multiple_pages() {
        let data = "\
Input:1:./main.tex
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
h1,5,0:100,100
}1
{2
h1,25,0:200,200
}2
Postamble:
";
        let result = parse_synctex_data(data, 2, 200.0, 200.0);
        assert!(result.is_some());
        let (_, line, _) = result.unwrap();
        assert_eq!(line, 25);
    }

    // --- detect_tex_engine ---

    #[test]
    fn test_detect_tex_engine_xelatex() {
        let content = "% !TEX program = xelatex\n\\documentclass{article}\n";
        assert_eq!(detect_tex_engine(content), Some(TexEngine::XeLaTeX));
    }

    #[test]
    fn test_detect_tex_engine_pdflatex() {
        let content = "% !TEX program = pdflatex\n\\documentclass{article}\n";
        assert_eq!(detect_tex_engine(content), Some(TexEngine::Latex));
    }

    #[test]
    fn test_detect_tex_engine_lualatex() {
        let content = "% !TEX program = lualatex\n\\documentclass{article}\n";
        assert_eq!(detect_tex_engine(content), Some(TexEngine::LuaLaTeX));
    }

    #[test]
    fn test_detect_tex_engine_none() {
        let content = "\\documentclass{article}\n\\begin{document}\nHello\n\\end{document}\n";
        assert_eq!(detect_tex_engine(content), None);
    }

    #[test]
    fn test_detect_tex_engine_case_insensitive() {
        let content = "% !TEX program = XeLaTeX\n";
        assert_eq!(detect_tex_engine(content), Some(TexEngine::XeLaTeX));
    }

    #[test]
    fn test_detect_tex_engine_no_spaces() {
        let content = "%!TEX program=xelatex\n";
        assert_eq!(detect_tex_engine(content), Some(TexEngine::XeLaTeX));
    }

    // --- persistent_build_dir edge case ---

    #[test]
    fn test_persistent_build_dir_trailing_slash() {
        let dir = persistent_build_dir("/project/");
        assert_eq!(dir, PathBuf::from("/project/.prism/build"));
    }

    // --- copy_dir_recursive integration tests ---

    #[test]
    fn test_copy_dir_recursive_nested() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Create nested structure
        std::fs::create_dir_all(src.path().join("sub").join("deep")).unwrap();
        std::fs::write(src.path().join("top.tex"), "top").unwrap();
        std::fs::write(src.path().join("sub").join("mid.tex"), "mid").unwrap();
        std::fs::write(
            src.path().join("sub").join("deep").join("bottom.tex"),
            "bottom",
        )
        .unwrap();

        copy_dir_recursive(src.path(), dst.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("top.tex")).unwrap(),
            "top"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("sub").join("mid.tex")).unwrap(),
            "mid"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("sub").join("deep").join("bottom.tex"))
                .unwrap(),
            "bottom"
        );
    }

    #[test]
    fn test_copy_dir_recursive_skips_hidden_dirs() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join(".git")).unwrap();
        std::fs::write(src.path().join(".git").join("config"), "secret").unwrap();
        std::fs::write(src.path().join("main.tex"), "doc").unwrap();

        copy_dir_recursive(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("main.tex").exists());
        assert!(!dst.path().join(".git").exists(), ".git should be skipped");
    }

    #[test]
    fn test_copy_dir_recursive_empty_subdir() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("empty_sub")).unwrap();
        std::fs::write(src.path().join("a.tex"), "a").unwrap();

        copy_dir_recursive(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("empty_sub").exists());
        assert!(dst.path().join("empty_sub").is_dir());
    }

    // --- sync_source_files integration tests ---

    #[test]
    fn test_sync_source_files_copies_sources() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::write(src.path().join("main.tex"), "doc").unwrap();
        std::fs::write(src.path().join("refs.bib"), "bib").unwrap();
        std::fs::write(src.path().join("style.sty"), "sty").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("main.tex")).unwrap(),
            "doc"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("refs.bib")).unwrap(),
            "bib"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("style.sty")).unwrap(),
            "sty"
        );
    }

    #[test]
    fn test_sync_source_files_skips_artifacts() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::write(src.path().join("main.tex"), "doc").unwrap();
        std::fs::write(src.path().join("main.aux"), "aux").unwrap();
        std::fs::write(src.path().join("main.log"), "log").unwrap();
        std::fs::write(src.path().join("main.synctex.gz"), "sync").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("main.tex").exists());
        assert!(!dst.path().join("main.aux").exists());
        assert!(!dst.path().join("main.log").exists());
        assert!(!dst.path().join("main.synctex.gz").exists());
    }

    #[test]
    fn test_sync_source_files_recursive_and_skips_hidden() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("chapters")).unwrap();
        std::fs::create_dir_all(src.path().join(".claudeprism")).unwrap();
        std::fs::write(src.path().join("chapters").join("ch1.tex"), "ch1").unwrap();
        std::fs::write(src.path().join("chapters").join("ch1.aux"), "aux").unwrap();
        std::fs::write(src.path().join(".claudeprism").join("data"), "data").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("chapters").join("ch1.tex")).unwrap(),
            "ch1"
        );
        assert!(!dst.path().join("chapters").join("ch1.aux").exists());
        assert!(!dst.path().join(".claudeprism").exists());
    }

    // --- sync_source_files copies figure PDFs ---

    #[test]
    fn test_sync_source_files_copies_figure_pdfs() {
        // .pdf files (e.g. figures) must be synced — they are NOT artifacts.
        // The output PDF is managed by compile_latex (explicit remove_file).
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("figures")).unwrap();
        std::fs::write(src.path().join("main.tex"), "doc").unwrap();
        std::fs::write(src.path().join("figures").join("chart.pdf"), "pdf figure").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("main.tex").exists());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("figures").join("chart.pdf")).unwrap(),
            "pdf figure"
        );
    }

    #[test]
    fn test_sync_source_files_overwrites_changed_tex_content() {
        // Regression: when a user empties a file, sync must overwrite
        // the old content in the build dir with the empty content.
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Old content in build dir
        std::fs::write(dst.path().join("main.tex"), "old content").unwrap();
        // User emptied the file
        std::fs::write(src.path().join("main.tex"), "").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("main.tex")).unwrap(),
            ""
        );
    }

    // --- the project's own build/ stays out of the copy ---

    #[test]
    fn test_copy_dir_recursive_skips_the_projects_build_folder() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("build")).unwrap();
        std::fs::write(src.path().join("build").join("main.pdf"), "cli pdf").unwrap();
        std::fs::create_dir_all(src.path().join("figures").join("build")).unwrap();
        std::fs::write(src.path().join("figures").join("build").join("a.tex"), "a").unwrap();
        std::fs::write(src.path().join("main.tex"), "doc").unwrap();

        copy_dir_recursive(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("main.tex").exists());
        assert!(!dst.path().join("build").exists());
        // Only the top-level build/ is thelab's; a deeper one is the user's.
        assert!(dst
            .path()
            .join("figures")
            .join("build")
            .join("a.tex")
            .exists());
    }

    #[test]
    fn test_sync_source_files_skips_the_projects_build_folder() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("build")).unwrap();
        std::fs::write(src.path().join("build").join("main.pdf"), "cli pdf").unwrap();
        std::fs::write(src.path().join("main.tex"), "doc").unwrap();

        sync_source_files(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("main.tex").exists());
        assert!(!dst.path().join("build").exists());
    }

    // --- engine menu ---

    #[test]
    fn test_engine_choice_maps_the_menu_onto_thelab() {
        assert_eq!(EngineChoice::parse(None), Ok(EngineChoice::Auto));
        assert_eq!(EngineChoice::parse(Some("auto")), Ok(EngineChoice::Auto));
        assert_eq!(
            EngineChoice::parse(Some("tectonic")),
            Ok(EngineChoice::Tectonic)
        );
        assert_eq!(
            EngineChoice::parse(Some("texlive")),
            Ok(EngineChoice::TexLive)
        );
        assert!(EngineChoice::parse(Some("lualatex")).is_err());

        assert_eq!(EngineChoice::Auto.engine(), None);
        assert_eq!(EngineChoice::Tectonic.engine(), Some(Engine::Tectonic));
        assert_eq!(EngineChoice::TexLive.engine(), Some(Engine::Latexmk));
    }

    // --- SyncTeX inputs ---

    #[test]
    fn test_project_relative_strips_the_copy() {
        let work = Path::new("/p/.prism/build");
        // pdfTeX writes absolute paths with a `./` in them.
        assert_eq!(
            project_relative("/p/.prism/build/./chapters/one.tex", work, work),
            "chapters/one.tex"
        );
        // latexmk runs in the main file's folder; relative inputs start there.
        assert_eq!(
            project_relative("./intro.tex", work, &work.join("paper")),
            "paper/intro.tex"
        );
        assert_eq!(
            project_relative("../shared/defs.tex", work, &work.join("paper")),
            "shared/defs.tex"
        );
        // A file of the TeX installation stays as it was written.
        assert_eq!(
            project_relative("/usr/local/texlive/article.cls", work, work),
            "/usr/local/texlive/article.cls"
        );
    }

    // --- reports ---

    #[test]
    fn test_failure_message_lists_one_error_per_line() {
        let report = CompileReport {
            project_dir: "/p".to_string(),
            main_file: "main.tex".to_string(),
            engine: Some("latexmk"),
            ok: false,
            has_pdf: false,
            duration_ms: 1,
            errors: vec![
                CompileProblem {
                    severity: "error",
                    file: Some("refs.bib".to_string()),
                    line: Some(4),
                    message: "syntax error:\n  found end of entry".to_string(),
                },
                CompileProblem::error("biber failed"),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            report.failure_message(),
            "Compilation failed (latexmk)\n\nrefs.bib:4: syntax error: found end of entry\nbiber failed"
        );
    }

    #[test]
    fn test_compile_project_without_the_main_file_reports_it() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("notes.txt"), "no tex here").unwrap();

        let outcome = compile_project(project.path(), "main.tex", EngineChoice::Auto);

        assert!(outcome.pdf.is_none());
        assert!(!outcome.report.ok);
        assert_eq!(outcome.report.engine, None);
        assert!(outcome.report.errors[0]
            .message
            .contains("No .tex file found"));
    }

    // --- compiling through thelab-core (needs latexmk and biber) ---

    fn have(program: &str) -> bool {
        thelab_core::tools::find_program(program).is_some()
    }

    const BIBLATEX_MAIN: &str = "\\documentclass{article}
\\usepackage{biblatex}
\\addbibresource{refs.bib}
\\begin{document}
As \\textcite{knuth1984} shows.
\\printbibliography
\\end{document}
";

    const KNUTH: &str = "@book{knuth1984,
  author = {Knuth, Donald E.},
  title = {The {\\TeX}book},
  publisher = {Addison-Wesley},
  year = {1984}
}
";

    #[test]
    fn test_a_biber_document_compiles_with_latexmk_and_a_broken_bib_is_located() {
        if !have("latexmk") || !have("biber") {
            eprintln!("skipped: latexmk or biber is not installed");
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.tex"), BIBLATEX_MAIN).unwrap();
        std::fs::write(project.path().join("refs.bib"), KNUTH).unwrap();

        let outcome = compile_project(project.path(), "main.tex", EngineChoice::Auto);
        let report = &outcome.report;
        assert_eq!(report.engine, Some("latexmk"), "{report:#?}");
        assert!(report.ok, "{report:#?}");
        assert!(outcome
            .pdf
            .as_ref()
            .is_some_and(|pdf| pdf.starts_with(b"%PDF")));
        // The engine ran on the copy, which the project does not see, and
        // left the PDF where the project picker looks for it.
        assert!(compiled_pdf_dir(project.path()).join("main.pdf").is_file());
        assert!(!project.path().join("build").exists());

        // An entry left open: biber's own message, at its file and line.
        let broken =
            format!("{KNUTH}\n@article{{broken2026,\n  title = {{Unclosed,\n  year = 2026\n");
        std::fs::write(project.path().join("refs.bib"), broken).unwrap();
        let outcome = compile_project(project.path(), "main.tex", EngineChoice::Auto);
        let report = &outcome.report;
        assert!(!report.ok, "{report:#?}");
        let located = report
            .errors
            .iter()
            .find(|e| e.file.as_deref() == Some("refs.bib"))
            .unwrap_or_else(|| panic!("no error in refs.bib: {report:#?}"));
        assert!(located.line.is_some(), "{located:#?}");
        // biber stopped, so every citation is undefined: not worth listing.
        assert!(
            report
                .warnings
                .iter()
                .all(|w| !w.message.contains("Citation")),
            "{report:#?}"
        );

        // Compiling again unchanged (or after a restart: .prism/build stays)
        // gives the same answer, though latexmk only repeats its verdict.
        let again = compile_project(project.path(), "main.tex", EngineChoice::Auto);
        assert_eq!(again.report.has_pdf, report.has_pdf);
        assert_eq!(again.pdf.is_some(), outcome.pdf.is_some());
        assert_eq!(again.report.errors, report.errors);
        assert_eq!(again.report.warnings, report.warnings);
    }

    // --- the same sources give the same preview ---

    #[test]
    fn test_unchanged_sources_with_an_error_show_their_pdf_every_time() {
        if !have("latexmk") {
            eprintln!("skipped: latexmk is not installed");
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\nThis calls \\undefinedmacro here.\n\\end{document}\n",
        )
        .unwrap();

        let first = compile_project(project.path(), "main.tex", EngineChoice::TexLive);
        // pdflatex goes on past the error and writes a PDF, which the
        // preview shows while the error is in the problems panel.
        assert!(first.report.has_pdf, "{:#?}", first.report);
        assert!(first.pdf.is_some());
        assert_eq!(first.report.errors.len(), 1, "{:#?}", first.report);
        for run in ["second", "third"] {
            let again = compile_project(project.path(), "main.tex", EngineChoice::TexLive);
            assert!(again.report.has_pdf, "{run}: {:#?}", again.report);
            assert_eq!(again.pdf, first.pdf, "{run}");
            assert_eq!(again.report.errors, first.report.errors, "{run}");
        }
    }

    // --- an empty document ---

    /// The Blank template (template-registry.ts) and a new .tex file
    /// (document-store.ts createNewFile).
    const EMPTY_DOCUMENTS: [&str; 2] = [
        "\\documentclass[12pt]{article}\n\\usepackage[utf8]{inputenc}\n\\usepackage[T1]{fontenc}\n\n\\begin{document}\n\n% Start writing here.\n\n\\end{document}\n",
        "\\documentclass{article}\n\n\\begin{document}\n\n% Your content here\n\n\\end{document}\n",
    ];

    #[test]
    fn test_an_empty_document_shows_a_blank_page_and_its_file_is_untouched() {
        for (choice, program) in [
            (EngineChoice::Tectonic, "tectonic"),
            (EngineChoice::TexLive, "latexmk"),
        ] {
            if !have(program) {
                eprintln!("skipped {choice:?}: {program} is not installed");
                continue;
            }
            for document in EMPTY_DOCUMENTS {
                let project = tempfile::tempdir().unwrap();
                let main = project.path().join("main.tex");
                std::fs::write(&main, document).unwrap();
                for run in ["first", "second"] {
                    let outcome = compile_project(project.path(), "main.tex", choice);
                    let report = &outcome.report;
                    assert!(report.ok, "{choice:?} {run}: {report:#?}");
                    assert!(report.has_pdf, "{choice:?} {run}");
                    assert!(outcome.pdf.is_some(), "{choice:?} {run}");
                    assert_eq!(report.errors, [], "{choice:?} {run}");
                    assert_eq!(std::fs::read_to_string(&main).unwrap(), document);
                }
            }
        }
    }

    #[test]
    fn test_an_empty_document_that_cannot_be_given_a_page_says_why() {
        // \begin{document} is in another file, so no blank page is added.
        if !have("tectonic") && !have("latexmk") {
            eprintln!("skipped: no engine is installed");
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("main.tex"),
            "\\documentclass{article}\n\\input{body}\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("body.tex"),
            "\\begin{document}\n\\end{document}\n",
        )
        .unwrap();

        let outcome = compile_project(project.path(), "main.tex", EngineChoice::Auto);
        let report = &outcome.report;
        assert!(outcome.pdf.is_none());
        assert_eq!(
            report.errors,
            [CompileProblem {
                severity: "error",
                file: Some("main.tex".to_string()),
                line: None,
                message: NO_PAGES.to_string(),
            }],
            "{report:#?}"
        );
        assert!(
            report
                .failure_message()
                .ends_with(&format!("main.tex: {NO_PAGES}")),
            "{}",
            report.failure_message()
        );
    }

    #[test]
    fn test_a_blank_page_goes_on_the_line_of_the_real_begin_document() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        std::fs::write(
            &main,
            "\\documentclass{article}\n% \\begin{document} comes below\n\\begin{document}\n\\end{document}\n",
        )
        .unwrap();
        assert!(add_blank_page(&main));
        assert_eq!(
            std::fs::read_to_string(&main).unwrap(),
            "\\documentclass{article}\n% \\begin{document} comes below\n\\AtEndDocument{\\null}\\begin{document}\n\\end{document}\n"
        );

        std::fs::write(&main, "\\documentclass{article}\n\\input{body}\n").unwrap();
        assert!(!add_blank_page(&main));
    }

    // --- the program for fontspec ---

    /// The engine's banner, the first line of the log a compile left.
    fn log_banner(project: &Path) -> String {
        let log = compiled_pdf_dir(project).join("main.log");
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        text.lines().next().unwrap_or_default().to_string()
    }

    #[test]
    fn test_a_fontspec_document_is_compiled_with_xelatex_by_latexmk() {
        if !have("latexmk") {
            eprintln!("skipped: latexmk is not installed");
            return;
        }
        let fontspec = "\\documentclass{article}\n\\usepackage{fontspec}\n\\begin{document}\nHello.\n\\end{document}\n";

        // The TeX Live choice.
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.tex"), fontspec).unwrap();
        let outcome = compile_project(project.path(), "main.tex", EngineChoice::TexLive);
        assert_eq!(outcome.report.engine, Some("latexmk"));
        assert!(outcome.report.ok, "{:#?}", outcome.report);
        assert!(log_banner(project.path()).starts_with("This is XeTeX,"));

        // A magic comment still wins.
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("main.tex"),
            format!("% !TEX program = lualatex\n{fontspec}"),
        )
        .unwrap();
        let outcome = compile_project(project.path(), "main.tex", EngineChoice::TexLive);
        assert!(outcome.report.ok, "{:#?}", outcome.report);
        assert!(log_banner(project.path()).starts_with("This is LuaHBTeX,"));

        // Auto, with biber: latexmk runs it, with XeLaTeX.
        if !have("biber") {
            eprintln!("skipped Auto: biber is not installed");
            return;
        }
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("main.tex"),
            BIBLATEX_MAIN.replace(
                "\\usepackage{biblatex}",
                "\\usepackage{fontspec}\n\\usepackage{biblatex}",
            ),
        )
        .unwrap();
        std::fs::write(project.path().join("refs.bib"), KNUTH).unwrap();
        let outcome = compile_project(project.path(), "main.tex", EngineChoice::Auto);
        assert_eq!(outcome.report.engine, Some("latexmk"));
        assert!(outcome.report.ok, "{:#?}", outcome.report);
        assert!(log_banner(project.path()).starts_with("This is XeTeX,"));
    }

    // --- where the PDF is ---

    #[test]
    fn test_the_frontend_looks_for_the_pdf_where_compiles_leave_it() {
        // The project picker's first place to look (src/lib/compiled-pdf.ts).
        let segments: Vec<String> =
            serde_json::from_str(include_str!("../../src/lib/compiled-pdf-dir.json")).unwrap();
        let project = Path::new("/p");
        let frontend = segments
            .iter()
            .fold(project.to_path_buf(), |path, segment| path.join(segment));
        assert_eq!(frontend, compiled_pdf_dir(project));

        // compiled_pdf_dir is where thelab puts the PDF when it compiles
        // the copy (an actual compile checks it in the biber test).
        let project = tempfile::tempdir().unwrap();
        let work = persistent_build_dir(&project.path().to_string_lossy());
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("main.tex"), "\\documentclass{article}").unwrap();
        let copy = thelab_core::scan::scan(&work, Some("main.tex")).unwrap();
        assert_eq!(
            thelab_core::compile::pdf_path(&copy).unwrap(),
            compiled_pdf_dir(&project.path().canonicalize().unwrap()).join("main.pdf")
        );
    }

    /// Compiles the project folder named by `THELAB_COMPILE_PROJECT` through
    /// `compile_project`, exactly as the app does, and prints the report:
    ///
    /// THELAB_COMPILE_PROJECT=<a copy of a project> cargo test --manifest-path
    /// apps/desktop/src-tauri/Cargo.toml compile_a_named_project -- --ignored --nocapture
    ///
    /// `THELAB_COMPILE_MAIN` names the main file (default main.tex) and
    /// `THELAB_COMPILE_ENGINE` the menu choice (default auto). With
    /// `THELAB_EXPECT_BIB_ERROR` set it expects an error located in a .bib
    /// file; otherwise a clean compile with a PDF.
    #[test]
    #[ignore]
    fn compile_a_named_project() {
        let Some(project) = std::env::var_os("THELAB_COMPILE_PROJECT") else {
            panic!("set THELAB_COMPILE_PROJECT to a project folder (a copy: .prism/ is written)");
        };
        let main = std::env::var("THELAB_COMPILE_MAIN").unwrap_or_else(|_| "main.tex".into());
        let choice =
            EngineChoice::parse(std::env::var("THELAB_COMPILE_ENGINE").ok().as_deref()).unwrap();

        let outcome = compile_project(Path::new(&project), &main, choice);
        let report = &outcome.report;
        println!("{}", serde_json::to_string_pretty(report).unwrap());

        if std::env::var_os("THELAB_EXPECT_BIB_ERROR").is_some() {
            let located = report.errors.iter().find(|e| {
                e.file.as_deref().is_some_and(|f| f.ends_with(".bib")) && e.line.is_some()
            });
            assert!(located.is_some(), "no error located in a .bib file");
            println!("located: {}", located.unwrap().describe());
        } else {
            assert!(report.ok, "the compile reported errors");
            assert!(outcome.pdf.is_some(), "no PDF");
        }
    }
}
