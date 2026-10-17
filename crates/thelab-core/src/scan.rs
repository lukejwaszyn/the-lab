//! Finds the files of a LaTeX project and decides which one is the main file.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tex::Source;
use crate::{Error, Result};

/// Name of the optional per-project settings file.
pub const CONFIG_FILE: &str = "thelab.toml";

/// Directories that never hold project sources.
const SKIPPED_DIRECTORIES: [&str; 3] = ["build", "target", "node_modules"];

/// Settings from `thelab.toml` in the project root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The main `.tex` file, relative to the project root.
    pub main: Option<String>,
    /// `tectonic` or `latexmk`.
    pub engine: Option<String>,
    /// `[python]`: which interpreter runs which folder.
    pub python: Option<crate::python::PythonConfig>,
}

/// The source files of a project. Paths are relative to `root` and use `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The file to compile, if one could be identified.
    pub main: Option<String>,
    pub tex_files: Vec<String>,
    pub bib_files: Vec<String>,
    pub config: Config,
}

impl Project {
    /// The main file, or an error that says how to name one.
    pub fn require_main(&self) -> Result<&str> {
        self.main.as_deref().ok_or_else(|| {
            Error::new(format!(
                "No main .tex file found in {}. Expected a .tex file containing \\documentclass; \
                 name one explicitly or set `main` in {CONFIG_FILE}.",
                self.root.display()
            ))
        })
    }
}

/// Scans the project at `root`. `main_override` (relative to `root`) takes
/// precedence over `thelab.toml` and detection.
pub fn scan(root: &Path, main_override: Option<&str>) -> Result<Project> {
    let root = fs::canonicalize(root).map_err(|e| {
        Error::new(format!(
            "Cannot open project folder {}: {e}",
            root.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(Error::new(format!("{} is not a folder", root.display())));
    }

    let config = read_config(&root)?;
    let mut tex_files = Vec::new();
    let mut bib_files = Vec::new();
    walk(&root, "", 0, &mut tex_files, &mut bib_files);

    let main = match main_override.or(config.main.as_deref()) {
        Some(named) => {
            let named = named.trim_start_matches("./").to_string();
            if !root.join(&named).is_file() {
                return Err(Error::new(format!(
                    "Main file {named} does not exist in {}",
                    root.display()
                )));
            }
            Some(named)
        }
        None => detect_main(&root, &tex_files),
    };

    Ok(Project {
        root,
        main,
        tex_files,
        bib_files,
        config,
    })
}

fn read_config(root: &Path) -> Result<Config> {
    let path = root.join(CONFIG_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| Error::new(format!("Invalid {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(Error::new(format!("Cannot read {}: {e}", path.display()))),
    }
}

fn walk(root: &Path, relative: &str, depth: usize, tex: &mut Vec<String>, bib: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        // `file_type` does not follow symlinks, so a link back up the tree
        // cannot send the walk in circles.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < 16 && !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                walk(root, &path, depth + 1, tex, bib);
            }
        } else if name.ends_with(".tex") {
            tex.push(path);
        } else if name.ends_with(".bib") {
            bib.push(path);
        }
    }
}

/// Picks the main file among those that contain `\documentclass`:
/// `main.tex` or `thesis.tex` in the root if present, otherwise the one
/// nearest the root.
fn detect_main(root: &Path, tex_files: &[String]) -> Option<String> {
    let mut candidates: Vec<&String> = tex_files
        .iter()
        .filter(|file| has_documentclass(&root.join(file)))
        .collect();
    for preferred in ["main.tex", "thesis.tex"] {
        if let Some(found) = candidates.iter().find(|file| file.as_str() == preferred) {
            return Some(found.to_string());
        }
    }
    candidates.sort_by_key(|file| (file.matches('/').count(), file.as_str()));
    candidates.first().map(|file| file.to_string())
}

fn has_documentclass(path: &Path) -> bool {
    match read_text(path) {
        Ok(text) => Source::new(&text)
            .commands()
            .iter()
            .any(|c| c.name == "documentclass"),
        Err(_) => false,
    }
}

/// The language mode of a file, judged by its extension. These are the names
/// the app uses for its editor modes.
pub fn language_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "tex" | "sty" | "cls" | "ltx" => "latex",
        "bib" => "bibtex",
        "py" => "python",
        "ipynb" => "notebook",
        "md" | "markdown" => "markdown",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "sh" | "bash" | "zsh" => "shell",
        "sysml" | "kerml" => "sysml",
        "swift" => "swift",
        "rs" => "rust",
        _ => return None,
    })
}

/// Reads a source file, tolerating bytes that are not UTF-8 (old projects in
/// Latin-1 still turn up).
pub fn read_text(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).map_err(|e| Error::new(format!("Cannot read {}: {e}", path.display())))?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}
