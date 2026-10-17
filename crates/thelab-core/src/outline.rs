//! Reads a document from its main file through `\input` and `\include`, in
//! the order TeX would, and records its structure: headings, labels,
//! references, citations and how much prose sits under each heading.

use std::path::Path;

use serde::Serialize;

use crate::Result;
use crate::scan::{Project, read_text};
use crate::tex::{Command, Source, count_words, matching, plain_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Part,
    Chapter,
    Section,
    Subsection,
    Subsubsection,
}

impl Level {
    fn of_command(name: &str) -> Option<Self> {
        Some(match name {
            "part" => Self::Part,
            "chapter" => Self::Chapter,
            "section" => Self::Section,
            "subsection" => Self::Subsection,
            "subsubsection" => Self::Subsubsection,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Part => "part",
            Self::Chapter => "chapter",
            Self::Section => "section",
            Self::Subsection => "subsection",
            Self::Subsubsection => "subsubsection",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Heading {
    pub level: Level,
    pub title: String,
    pub file: String,
    pub line: u32,
    /// Words of prose between this heading and the next one of any level.
    pub words: u32,
}

/// A name (label, citation key, file) and where it appears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Occurrence {
    pub name: String,
    pub file: String,
    pub line: u32,
}

/// A chapter, or a section in a document without chapters, with the word
/// count of everything inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub level: Level,
    pub title: String,
    pub file: String,
    pub line: u32,
    pub words: u32,
}

/// The structure of one document. File paths are relative to the project root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    pub main: String,
    /// The main file and everything it pulls in, in reading order.
    pub files: Vec<String>,
    /// `\input` and `\include` targets that do not exist.
    pub missing_inputs: Vec<Occurrence>,
    pub headings: Vec<Heading>,
    pub labels: Vec<Occurrence>,
    /// Every use of `\ref` and its relatives, one entry per label named.
    pub references: Vec<Occurrence>,
    /// Every use of a citation command, one entry per key named.
    pub citations: Vec<Occurrence>,
    /// Whether `\nocite{*}` pulls in the whole bibliography.
    pub cites_everything: bool,
    /// Keys defined by `\bibitem` in a hand-written bibliography.
    pub bib_items: Vec<Occurrence>,
    /// `.bib` files named by `\bibliography` or `\addbibresource`.
    pub bib_resources: Vec<String>,
    /// Words of prose before the first heading (the abstract, usually).
    pub front_words: u32,
}

impl Outline {
    pub fn total_words(&self) -> u32 {
        self.front_words + self.headings.iter().map(|h| h.words).sum::<u32>()
    }

    /// Word counts per chapter, or per section if there are no chapters.
    pub fn units(&self) -> Vec<Unit> {
        let has = |level| self.headings.iter().any(|h| h.level == level);
        let top = if has(Level::Chapter) {
            Level::Chapter
        } else if has(Level::Section) {
            Level::Section
        } else {
            return Vec::new();
        };

        let mut units: Vec<Unit> = Vec::new();
        let mut open = false;
        for heading in &self.headings {
            if heading.level == top {
                units.push(Unit {
                    level: heading.level,
                    title: heading.title.clone(),
                    file: heading.file.clone(),
                    line: heading.line,
                    words: heading.words,
                });
                open = true;
            } else if heading.level > top {
                if let Some(unit) = units.last_mut().filter(|_| open) {
                    unit.words += heading.words;
                }
            } else {
                // A \part ends the chapter before it.
                open = false;
            }
        }
        units
    }
}

/// Reads the outline of the project's main document.
pub fn read_outline(project: &Project) -> Result<Outline> {
    let main = project.require_main()?.to_string();
    let source = Source::new(&read_text(&project.root.join(&main))?);
    let has_body_marker = source
        .commands()
        .iter()
        .any(|c| is_environment(c, "begin", "document"));

    let mut walker = Walker {
        root: &project.root,
        main_dir: parent(&main).to_string(),
        outline: Outline {
            main: main.clone(),
            ..Outline::default()
        },
        visiting: Vec::new(),
        // A file with no \begin{document} is treated as all body.
        in_body: !has_body_marker,
        finished: false,
    };
    walker.walk(&main);
    Ok(walker.outline)
}

struct Walker<'a> {
    root: &'a Path,
    /// Directory of the main file, which is where TeX resolves `\input` from.
    main_dir: String,
    outline: Outline,
    /// Files currently being read, to stop a file that includes itself.
    visiting: Vec<String>,
    in_body: bool,
    /// Set at `\end{document}`: TeX reads nothing after it.
    finished: bool,
}

impl Walker<'_> {
    fn walk(&mut self, file: &str) {
        if self.visiting.iter().any(|open| open == file) {
            return;
        }
        let Ok(text) = read_text(&self.root.join(file)) else {
            return;
        };
        if !self.outline.files.iter().any(|known| known == file) {
            self.outline.files.push(file.to_string());
        }
        self.visiting.push(file.to_string());

        let source = Source::new(&text);
        let text = source.text();
        // Prose before `cursor` has been counted already.
        let mut cursor = 0;

        for command in source.commands() {
            if self.finished {
                break;
            }
            let here = |name: &str| Occurrence {
                name: name.trim().to_string(),
                file: file.to_string(),
                line: command.line,
            };
            // Commands inside a heading's title start before the cursor.
            let nested = command.start < cursor;

            // The bibliography can be declared in the preamble (biblatex).
            if let ("bibliography" | "addbibresource", Some(names)) =
                (command.name, command.argument)
            {
                for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    let name = if name.ends_with(".bib") {
                        name.to_string()
                    } else {
                        format!("{name}.bib")
                    };
                    let path = normalize(&join(&self.main_dir, &name));
                    if !self.outline.bib_resources.contains(&path) {
                        self.outline.bib_resources.push(path);
                    }
                }
            }

            if is_environment(&command, "begin", "document") {
                self.in_body = true;
                cursor = command.end;
                continue;
            }
            if !self.in_body {
                if let ("input" | "include", Some(name)) = (command.name, command.argument)
                    && let Some(path) = self.resolve_input(name, file)
                    && !self.outline.files.contains(&path)
                {
                    // Preamble files are listed but not read: they define
                    // macros, and a \label inside a definition is not a label.
                    self.outline.files.push(path);
                }
                continue;
            }
            if is_environment(&command, "end", "document") {
                self.add_words(&text[cursor.min(command.start)..command.start]);
                self.finished = true;
                break;
            }

            if let (Some(level), Some(title)) = (Level::of_command(command.name), command.argument)
            {
                if !nested {
                    self.add_words(&text[cursor..command.start]);
                    self.outline.headings.push(Heading {
                        level,
                        title: plain_text(&without_labels(title)),
                        file: file.to_string(),
                        line: command.line,
                        words: 0,
                    });
                    cursor = command.end;
                }
                continue;
            }

            match (command.name, command.argument) {
                ("input" | "include" | "subfile", Some(name)) if !nested => {
                    self.add_words(&text[cursor..command.start]);
                    cursor = command.end;
                    match self.resolve_input(name, file) {
                        Some(path) => self.walk(&path),
                        None => self.outline.missing_inputs.push(here(name)),
                    }
                }
                ("label", Some(name)) if !name.contains('#') => {
                    self.outline.labels.push(here(name))
                }
                ("bibitem", Some(key)) => self.outline.bib_items.push(here(key)),
                ("hyperref", _) if !command.options.is_empty() => {
                    self.outline.references.push(here(command.options[0]));
                }
                (name, Some(argument)) if is_reference(name) => {
                    for label in argument.split(',').filter(|l| !l.trim().is_empty()) {
                        self.outline.references.push(here(label));
                    }
                }
                (name, Some(argument)) if is_citation(name) => {
                    for key in argument.split(',').map(str::trim).filter(|k| !k.is_empty()) {
                        if key == "*" {
                            self.outline.cites_everything = true;
                        } else {
                            self.outline.citations.push(here(key));
                        }
                    }
                }
                _ => {}
            }

            // Listings, frames and boxes set labels through `label=name`.
            let trailing = if command.name == "begin" {
                trailing_options(text, command.end)
            } else {
                None
            };
            for options in command.options.iter().copied().chain(trailing) {
                if let Some(name) = label_option(options) {
                    self.outline.labels.push(here(name));
                }
            }
        }

        if !self.finished && self.in_body {
            self.add_words(&text[cursor.min(text.len())..]);
        }
        self.visiting.pop();
    }

    fn add_words(&mut self, prose: &str) {
        let words = count_words(prose);
        match self.outline.headings.last_mut() {
            Some(heading) => heading.words += words,
            None => self.outline.front_words += words,
        }
    }

    /// Finds the file an `\input` names: relative to the main file's
    /// directory as TeX does, or failing that next to the including file.
    fn resolve_input(&self, name: &str, from: &str) -> Option<String> {
        let name = name.trim();
        for base in [self.main_dir.as_str(), parent(from)] {
            let plain = normalize(&join(base, name));
            for candidate in [format!("{plain}.tex"), plain] {
                if self.root.join(&candidate).is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

fn is_environment(command: &Command, which: &str, environment: &str) -> bool {
    command.name == which && command.argument == Some(environment)
}

fn is_reference(name: &str) -> bool {
    matches!(
        name,
        "ref"
            | "eqref"
            | "pageref"
            | "autoref"
            | "nameref"
            | "vref"
            | "Vref"
            | "cref"
            | "Cref"
            | "labelcref"
            | "subref"
    )
}

fn is_citation(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cite")
        && !matches!(
            lower.as_str(),
            "bibcite" | "citestyle" | "setcitestyle" | "defcitealias"
        )
}

/// The `[...]` that follows `\begin{name}`, if any.
fn trailing_options(text: &str, after: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(after) != Some(&b'[') {
        return None;
    }
    let close = matching(bytes, after, b'[', b']')?;
    Some(&text[after + 1..close])
}

/// The value of `label=` in a key-value list.
fn label_option(options: &str) -> Option<&str> {
    let mut rest = options;
    loop {
        let at = rest.find("label")?;
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = rest[at + "label".len()..].trim_start();
        if before_ok && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start().trim_start_matches('{');
            let end = value.find([',', '}', ']']).unwrap_or(value.len());
            let name = value[..end].trim();
            return (!name.is_empty()).then_some(name);
        }
        rest = &rest[at + "label".len()..];
    }
}

/// A heading's title with any `\label{...}` cut out.
fn without_labels(title: &str) -> String {
    let source = Source::new(title);
    let mut out = String::new();
    let mut cursor = 0;
    for command in source.commands() {
        if command.name == "label" && command.start >= cursor {
            out.push_str(&title[cursor..command.start]);
            cursor = command.end;
        }
    }
    out.push_str(&title[cursor..]);
    out
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_string()
    } else {
        format!("{directory}/{name}")
    }
}

/// Resolves `.` and `..` in a relative path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}
