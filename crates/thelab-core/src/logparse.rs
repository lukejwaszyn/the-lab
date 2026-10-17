//! Parsers for TeX engine logs, Tectonic's console output and BibTeX logs.
//!
//! Everything here is pure text processing: file names come back exactly as
//! the log spells them (minus a leading `./`). Mapping them onto files in the
//! project is the compile driver's job.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

impl Diagnostic {
    fn new(severity: Severity, file: Option<String>, line: Option<u32>, message: String) -> Self {
        Self {
            severity,
            file,
            line,
            message,
        }
    }
}

/// The widest overfull box of a run: the one most worth looking at.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorstBox {
    pub points: f64,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Overfull and underfull boxes, counted rather than listed.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxSummary {
    pub overfull_hbox: u32,
    pub underfull_hbox: u32,
    pub overfull_vbox: u32,
    pub underfull_vbox: u32,
    pub worst_overfull: Option<WorstBox>,
}

impl BoxSummary {
    pub fn total(&self) -> u32 {
        self.overfull_hbox + self.underfull_hbox + self.overfull_vbox + self.underfull_vbox
    }
}

/// What a TeX engine log (pdfTeX, XeTeX or LuaTeX format) says about a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogReport {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// Labels that `\ref` and friends pointed at but nothing defines.
    pub undefined_references: Vec<String>,
    /// Citation keys with no bibliography entry.
    pub undefined_citations: Vec<String>,
    pub duplicate_labels: Vec<String>,
    pub boxes: BoxSummary,
}

/// TeX wraps log lines at this many characters (`max_print_line`).
const WRAP_COLUMN: usize = 79;

/// Parses a `.log` file written by a TeX engine.
///
/// Handles both error styles: `! message` followed by an `l.N` context line
/// (the file is inferred from the parentheses TeX prints when it opens and
/// closes files) and `path:line: message` (`-file-line-error`).
pub fn parse_tex_log(log: &str) -> LogReport {
    let lines = unwrap_lines(log);
    let mut parser = LogParser::default();
    let mut i = 0;
    while i < lines.len() {
        i = parser.step(&lines, i);
    }
    parser.finish()
}

/// Rejoins lines that TeX broke at column 79. A line of exactly that length
/// is assumed to continue on the next one.
fn unwrap_lines(log: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut joining = false;
    for raw in log.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if !joining {
            current.clear();
        }
        current.push_str(raw);
        joining = raw.len() == WRAP_COLUMN || raw.chars().count() == WRAP_COLUMN;
        if !joining {
            out.push(std::mem::take(&mut current));
        }
    }
    if joining {
        out.push(current);
    }
    out
}

#[derive(Default)]
struct LogParser {
    /// One entry per unclosed `(`. Most are files; the rest are ordinary
    /// parenthesized text, kept so that its `)` does not close a file.
    open: Vec<String>,
    errors: Vec<Diagnostic>,
    /// Indices into `errors` still waiting for an `l.N` line.
    pending: Vec<usize>,
    warnings: Vec<Diagnostic>,
    undefined_references: Vec<String>,
    undefined_citations: Vec<String>,
    duplicate_labels: Vec<String>,
    boxes: BoxSummary,
}

impl LogParser {
    /// Handles the line at `i` and returns the index of the next line to look at.
    fn step(&mut self, lines: &[String], i: usize) -> usize {
        let line = lines[i].as_str();

        if let Some(found) = parse_box(line) {
            self.record_box(found);
            return skip_until_blank(lines, i + 1, 8);
        }

        if let Some(rest) = line.strip_prefix('!') {
            let message = clean_message(rest);
            if !message.starts_with("==>") {
                self.pending.push(self.errors.len());
                self.errors.push(Diagnostic::new(
                    Severity::Error,
                    self.current_file(),
                    None,
                    message,
                ));
            }
            return self.read_context(lines, i + 1);
        }

        if let Some((file, number, message)) = parse_file_line_error(line, false) {
            let message = clean_message(&message);
            self.locate_pending(Some(file.clone()), number);
            if !message.starts_with("==>") {
                self.errors.push(Diagnostic::new(
                    Severity::Error,
                    Some(file),
                    Some(number),
                    message,
                ));
            }
            return self.read_context(lines, i + 1);
        }

        if let Some(start) = WarningStart::parse(line) {
            return self.read_warning(lines, i, start);
        }

        if let Some(at) = line.find("pdfTeX warning") {
            self.track(&line[..at]);
            return self.read_pdftex_warning(lines, i, &line[at..]);
        }

        // TeX echoes the runaway text on the next line; it is document text,
        // not log structure.
        if line.starts_with("Runaway ") && line.ends_with('?') {
            return i + 2;
        }

        self.track(line);
        i + 1
    }

    fn finish(mut self) -> LogReport {
        // "Emergency stop" only says that an earlier error was fatal.
        if self.errors.iter().any(|e| e.message != "Emergency stop") {
            self.errors.retain(|e| e.message != "Emergency stop");
        }
        dedup(&mut self.errors);
        dedup(&mut self.warnings);
        LogReport {
            errors: self.errors,
            warnings: self.warnings,
            undefined_references: self.undefined_references,
            undefined_citations: self.undefined_citations,
            duplicate_labels: self.duplicate_labels,
            boxes: self.boxes,
        }
    }

    /// Reads the lines TeX prints after an error: macro context, then
    /// `l.N <source up to the error>`, the rest of the source line, and (in
    /// nonstop mode) a paragraph of help text.
    fn read_context(&mut self, lines: &[String], start: usize) -> usize {
        let mut i = start;
        while i < lines.len() && i - start < 40 {
            let line = lines[i].as_str();
            if line.starts_with('!') || parse_file_line_error(line, false).is_some() {
                // Another error before any location: let `step` take it.
                // Whatever is pending shares the location that one finds.
                return i;
            }
            if let Some(number) = parse_context_line(line) {
                let file = self.current_file();
                self.locate_pending(file, number);
                return self.skip_after_context(lines, i + 1);
            }
            if line.starts_with("<*>") {
                // The error surfaced at the end of the job; there is no line.
                self.pending.clear();
                return self.skip_after_context(lines, i + 1);
            }
            i += 1;
        }
        self.pending.clear();
        i
    }

    /// Skips the second half of a two-line source context and any help text.
    fn skip_after_context(&mut self, lines: &[String], start: usize) -> usize {
        let mut i = start;
        if let Some(rest_of_source) = lines.get(i) {
            self.track_after_source(rest_of_source);
            i += 1;
        }
        let help_start = i;
        while i < lines.len() && i - help_start < 12 {
            let line = lines[i].as_str();
            if line.trim().is_empty()
                || line.starts_with('!')
                || parse_file_line_error(line, false).is_some()
            {
                break;
            }
            i += 1;
        }
        i
    }

    fn locate_pending(&mut self, file: Option<String>, line: u32) {
        for index in self.pending.drain(..) {
            let error = &mut self.errors[index];
            error.line = Some(line);
            if error.file.is_none() {
                error.file = file.clone();
            }
        }
    }

    fn read_warning(&mut self, lines: &[String], i: usize, start: WarningStart) -> usize {
        let mut text = start.text.clone();
        let mut next = i + 1;
        while next < lines.len() && next - i <= 6 {
            match start.continuation(&lines[next]) {
                Some(more) => {
                    text.push(' ');
                    text.push_str(more);
                    next += 1;
                }
                None => break,
            }
        }

        let (text, line) = take_input_line(&text);
        let body = clean_message(&text);
        let message = format!("{}{}", start.keep_prefix, body);

        if body.starts_with("There were undefined")
            || body.starts_with("There were multiply-defined")
        {
            // Summaries of warnings already recorded one by one.
            return next;
        }

        let mut file = self.current_file().filter(|f| !f.ends_with(".aux"));
        let mut line = line;
        if let Some(label) =
            quoted_after(&body, "Label ").filter(|_| body.contains("multiply defined"))
        {
            push_unique(&mut self.duplicate_labels, label);
            // Reported while reading the .aux file, so there is no source location.
            file = None;
            line = None;
        } else if body.contains("undefined") {
            if let Some(label) = quoted_after(&body, "Reference ") {
                push_unique(&mut self.undefined_references, label);
            } else if let Some(key) = quoted_after(&body, "Citation ") {
                push_unique(&mut self.undefined_citations, key);
            }
        }

        self.warnings
            .push(Diagnostic::new(Severity::Warning, file, line, message));
        next
    }

    /// pdfTeX's own warnings are followed by a source context, like errors,
    /// which is the only place their line number appears.
    fn read_pdftex_warning(&mut self, lines: &[String], i: usize, text: &str) -> usize {
        let mut line = None;
        let mut next = i + 1;
        for (offset, candidate) in lines.iter().enumerate().skip(i + 1).take(5) {
            if candidate.trim().is_empty() {
                break;
            }
            if let Some(number) = parse_context_line(candidate) {
                line = Some(number);
                next = offset + 1;
                if let Some(rest_of_source) = lines.get(next) {
                    self.track_after_source(rest_of_source);
                    next += 1;
                }
                break;
            }
        }
        let file = self.current_file();
        self.warnings.push(Diagnostic::new(
            Severity::Warning,
            file,
            line,
            clean_message(text),
        ));
        next
    }

    fn record_box(&mut self, found: BoxWarning) {
        let count = match (found.overfull, found.vertical) {
            (true, false) => &mut self.boxes.overfull_hbox,
            (false, false) => &mut self.boxes.underfull_hbox,
            (true, true) => &mut self.boxes.overfull_vbox,
            (false, true) => &mut self.boxes.underfull_vbox,
        };
        *count += 1;

        if let Some(points) = found.points.filter(|_| found.overfull) {
            let wider = self
                .boxes
                .worst_overfull
                .as_ref()
                .is_none_or(|w| points > w.points);
            if wider {
                self.boxes.worst_overfull = Some(WorstBox {
                    points,
                    file: self.current_file(),
                    line: found.line,
                });
            }
        }
    }

    /// Follows the `(` and `)` TeX prints as it opens and closes files.
    fn track(&mut self, line: &str) {
        let mut rest = line;
        while let Some(at) = rest.find(['(', ')']) {
            let (paren, after) = rest[at..].split_at(1);
            if paren == "(" {
                let end = after
                    .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
                    .unwrap_or(after.len());
                self.open.push(after[..end].to_string());
                rest = &after[end..];
            } else {
                self.open.pop();
                rest = after;
            }
        }
    }

    /// The second line of a source context starts with the rest of the
    /// source line, which can hold any parentheses at all. Log output may
    /// continue after it on the same line, so pick tracking back up at the
    /// first thing that is unmistakably a file being opened.
    fn track_after_source(&mut self, line: &str) {
        let start = ["(./", "(/", "(../"]
            .iter()
            .filter_map(|p| line.find(p))
            .min();
        if let Some(start) = start {
            self.track(&line[start..]);
        }
    }

    fn current_file(&self) -> Option<String> {
        self.open
            .iter()
            .rev()
            .find(|name| is_pathish(name))
            .map(|name| normalize_path(name))
    }
}

fn skip_until_blank(lines: &[String], start: usize, limit: usize) -> usize {
    let mut i = start;
    while i < lines.len() && i - start < limit && !lines[i].trim().is_empty() {
        i += 1;
    }
    i
}

/// `l.12 some source text` gives 12.
fn parse_context_line(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("l.")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !(rest.len() == digits || rest[digits..].starts_with(' ')) {
        return None;
    }
    rest[..digits].parse().ok()
}

/// Splits `path:12: message`. With `bare_names`, the path may lack an
/// extension (Tectonic prints `chapters/one:5: ...`).
fn parse_file_line_error(line: &str, bare_names: bool) -> Option<(String, u32, String)> {
    let mut from = 0;
    while let Some(offset) = line[from..].find(':') {
        let colon = from + offset;
        let rest = &line[colon + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(": ") {
            let path = &line[..colon];
            let plausible = !path.is_empty()
                && !path.starts_with(char::is_whitespace)
                && !path.contains(": ")
                && (has_extension(path) || (bare_names && !path.contains(char::is_whitespace)));
            if !plausible {
                return None;
            }
            let number = rest[..digits].parse().ok()?;
            return Some((normalize_path(path), number, rest[digits + 2..].to_string()));
        }
        from = colon + 1;
    }
    None
}

fn has_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=8).contains(&ext.len())
                && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn is_pathish(token: &str) -> bool {
    token.chars().any(char::is_alphabetic)
        && token
            .chars()
            .all(|c| c.is_alphanumeric() || "/._-+~@:".contains(c))
}

fn normalize_path(path: &str) -> String {
    path.strip_prefix("./").unwrap_or(path).to_string()
}

/// Strips TeX's `!` marker and the trailing full stop.
fn clean_message(raw: &str) -> String {
    raw.trim_start_matches(|c: char| c == '!' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

/// Removes a trailing `on input line N` and returns N.
fn take_input_line(text: &str) -> (String, Option<u32>) {
    const MARKER: &str = "on input line ";
    if let Some(at) = text.rfind(MARKER) {
        let tail =
            text[at + MARKER.len()..].trim_end_matches(|c: char| c == '.' || c.is_whitespace());
        if let Ok(number) = tail.parse() {
            return (text[..at].to_string(), Some(number));
        }
    }
    (text.to_string(), None)
}

/// Finds `marker` and returns the quoted name after it: `` `name' ``,
/// `'name'` or `"name"`.
fn quoted_after(text: &str, marker: &str) -> Option<String> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let mut chars = rest.chars();
    let close = match chars.next()? {
        '`' | '\'' => '\'',
        '"' => '"',
        _ => return None,
    };
    let inner = chars.as_str();
    Some(inner[..inner.find(close)?].to_string())
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
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

/// The first line of a LaTeX, package or class warning.
struct WarningStart {
    /// Kept at the front of the message, so that package warnings say where
    /// they come from. Empty for the kernel's own `LaTeX Warning:`.
    keep_prefix: String,
    text: String,
    /// `(name)`, which opens each continuation line of a package warning.
    marker: Option<String>,
}

impl WarningStart {
    fn parse(line: &str) -> Option<Self> {
        if let Some(text) = line.strip_prefix("LaTeX Warning: ") {
            return Some(Self {
                keep_prefix: String::new(),
                text: text.to_string(),
                marker: None,
            });
        }
        if let Some(text) = line.strip_prefix("LaTeX Font Warning: ") {
            return Some(Self {
                keep_prefix: "LaTeX Font Warning: ".to_string(),
                text: text.to_string(),
                marker: Some("(Font)".to_string()),
            });
        }
        for kind in ["Package ", "Class "] {
            let Some(rest) = line.strip_prefix(kind) else {
                continue;
            };
            let Some((name, text)) = rest.split_once(" Warning: ") else {
                continue;
            };
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            return Some(Self {
                keep_prefix: format!("{kind}{name} Warning: "),
                text: text.to_string(),
                marker: Some(format!("({name})")),
            });
        }
        None
    }

    /// The text of `line` if it continues this warning.
    fn continuation<'a>(&self, line: &'a str) -> Option<&'a str> {
        let rest = match &self.marker {
            Some(marker) => line.strip_prefix(marker.as_str())?,
            // Kernel warnings indent their continuation lines.
            None => line.strip_prefix(' ')?,
        };
        let rest = rest.trim();
        (!rest.is_empty()).then_some(rest)
    }
}

struct BoxWarning {
    overfull: bool,
    vertical: bool,
    points: Option<f64>,
    line: Option<u32>,
}

/// `Overfull \hbox (12.3pt too wide) in paragraph at lines 7--8` and its
/// underfull, vertical and `detected at line N` variants.
fn parse_box(line: &str) -> Option<BoxWarning> {
    let (overfull, rest) = if let Some(rest) = line.strip_prefix("Overfull \\") {
        (true, rest)
    } else {
        (false, line.strip_prefix("Underfull \\")?)
    };
    let vertical = match rest.get(..4)? {
        "hbox" => false,
        "vbox" => true,
        _ => return None,
    };
    let points = rest
        .split_once('(')
        .and_then(|(_, after)| after.split_once("pt too "))
        .and_then(|(number, _)| number.parse().ok());
    let number_after = |marker: &str| {
        let tail = &rest[rest.find(marker)? + marker.len()..];
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        tail[..digits].parse().ok()
    };
    let line = number_after("at lines ").or_else(|| number_after("at line "));
    Some(BoxWarning {
        overfull,
        vertical,
        points,
        line,
    })
}

/// What Tectonic printed while compiling.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConsoleReport {
    pub errors: Vec<Diagnostic>,
    /// Warnings from Tectonic itself or its PDF backend.
    pub warnings: Vec<Diagnostic>,
    /// TeX warnings that Tectonic relays as `warning: file:line: ...`. They
    /// are repeated for every pass and are also in the kept log, so callers
    /// with a log should prefer that.
    pub engine_warnings: Vec<Diagnostic>,
}

/// Tectonic's lines around the output of an external tool (biber) that failed.
const TOOL_STDOUT_HEADER: &str = "the external tool exited with an error code; its stdout was:";
const TOOL_STDERR_HEADER: &str = "its stderr was:";
const TOOL_EXIT: &str = "the external tool exited with error code ";

/// Parses Tectonic's console output (`error:`, `warning:` and `note:` lines).
pub fn parse_tectonic_console(output: &str) -> ConsoleReport {
    let mut report = ConsoleReport::default();
    let mut in_dump = false;
    // Whether the last `error:` line was kept, so `caused by:` can extend it.
    let mut last_error_kept = false;
    // A failed external tool's output is quoted between rules too, but it is
    // kept: it holds the only explanation of the failure.
    let mut in_tool_output = false;
    let mut tool_output: Vec<&str> = Vec::new();
    // Named in a note on stdout, which callers pass after stderr.
    let tool = output
        .lines()
        .filter_map(|line| line.strip_prefix("note: Running external tool "))
        .next_back()
        .map_or("the external tool", |rest| {
            rest.trim_end_matches('.').trim()
        });

    for line in output.lines() {
        // Tectonic quotes the engine's terminal output between two rules.
        if line.starts_with("==========") {
            in_dump = !in_dump;
            continue;
        }
        if in_dump {
            if in_tool_output {
                tool_output.push(line);
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("error: ") {
            last_error_kept = false;
            in_tool_output = rest == TOOL_STDOUT_HEADER || rest == TOOL_STDERR_HEADER;
            if in_tool_output {
                continue;
            }
            if let Some(code) = rest.strip_prefix(TOOL_EXIT) {
                let message = tool_failure(tool, code.trim(), &tool_output);
                report
                    .errors
                    .push(Diagnostic::new(Severity::Error, None, None, message));
                tool_output.clear();
                last_error_kept = true;
                continue;
            }
            if rest.starts_with("something bad happened inside")
                || rest.contains("engine had an unrecoverable error")
            {
                continue;
            }
            let diagnostic = match parse_file_line_error(rest, true) {
                Some((file, number, message)) => Diagnostic::new(
                    Severity::Error,
                    Some(file),
                    Some(number),
                    clean_message(&message),
                ),
                None => Diagnostic::new(Severity::Error, None, None, clean_message(rest)),
            };
            report.errors.push(diagnostic);
            last_error_kept = true;
        } else if let Some(rest) = line.strip_prefix("caused by: ") {
            if last_error_kept && let Some(error) = report.errors.last_mut() {
                error.message = format!("{} (caused by: {})", error.message, clean_message(rest));
            }
        } else if let Some(rest) = line.strip_prefix("warning: ") {
            if rest.contains("use --print and/or --keep-logs") {
                continue;
            }
            match parse_file_line_error(rest, true) {
                Some((file, number, message)) => report.engine_warnings.push(Diagnostic::new(
                    Severity::Warning,
                    Some(file),
                    Some(number),
                    clean_message(&message),
                )),
                None => report.warnings.push(Diagnostic::new(
                    Severity::Warning,
                    None,
                    None,
                    clean_message(rest),
                )),
            }
        }
    }

    dedup(&mut report.errors);
    dedup(&mut report.warnings);
    dedup(&mut report.engine_warnings);
    report
}

/// One message for a failed external tool: its exit code and what it
/// printed, less biber's progress and warning lines.
fn tool_failure(tool: &str, code: &str, output: &[&str]) -> String {
    let said: Vec<&str> = output
        .iter()
        .map(|line| line.trim())
        .filter(|line| {
            !line.is_empty() && !line.starts_with("INFO - ") && !line.starts_with("WARN - ")
        })
        .map(|line| line.strip_prefix("ERROR - ").unwrap_or(line))
        .collect();
    if said.is_empty() {
        format!("{tool} exited with error code {code} without printing an error")
    } else {
        format!("{tool} exited with error code {code}: {}", said.join(" "))
    }
}

/// What a BibTeX log (`.blg`) says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlgReport {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// Cited keys that no database file defines.
    pub missing_entries: Vec<String>,
}

/// Parses a `.blg` file, written by BibTeX or by biber.
pub fn parse_blg(log: &str) -> BlgReport {
    const LINE: &str = "---line ";
    const READING: &str = "---while reading file ";
    let mut report = BlgReport::default();
    // The .bib file biber last said it was reading.
    let mut bib_source: Option<String> = None;

    for line in log.lines() {
        // biber starts every line with `[milliseconds] Module.pm:line> `.
        if line.starts_with('[')
            && let Some((_, said)) = line.split_once("> ")
        {
            if let Some(name) = quoted_after(said, "Found BibTeX data source ") {
                bib_source = Some(name);
            } else if let Some(message) = said.strip_prefix("ERROR - ") {
                let error = biber_diagnostic(Severity::Error, message, bib_source.as_deref());
                report.errors.push(error);
            } else if let Some(message) = said.strip_prefix("WARN - ") {
                let warning = biber_diagnostic(Severity::Warning, message, bib_source.as_deref());
                report.warnings.push(warning);
            }
        } else if let Some(rest) = line.strip_prefix("Warning--") {
            if let Some(key) = quoted_after(rest, "I didn't find a database entry for ") {
                push_unique(&mut report.missing_entries, key);
            }
            report.warnings.push(Diagnostic::new(
                Severity::Warning,
                None,
                None,
                format!("BibTeX: {}", rest.trim()),
            ));
        } else if let Some((message, location)) = line.split_once(LINE) {
            // `message---line N of file F`, or the location alone on the line
            // after the message.
            let located = location.split_once(" of file ").and_then(|(number, file)| {
                let file = file.trim();
                (!file.ends_with(".aux")).then(|| (file.to_string(), number.parse::<u32>().ok()))
            });
            let (file, number) = match located {
                Some((file, number)) => (Some(file), number),
                None => (None, None),
            };
            if message.is_empty() {
                if let Some(previous) = report.errors.last_mut().filter(|e| e.file.is_none()) {
                    previous.file = file;
                    previous.line = number;
                }
            } else {
                report.errors.push(Diagnostic::new(
                    Severity::Error,
                    file,
                    number,
                    format!("BibTeX: {message}"),
                ));
            }
        } else if let Some((message, _)) = line.split_once(READING) {
            report.errors.push(Diagnostic::new(
                Severity::Error,
                None,
                None,
                format!("BibTeX: {message}"),
            ));
        } else if line.starts_with("I couldn't open ") {
            report.errors.push(Diagnostic::new(
                Severity::Error,
                None,
                None,
                format!("BibTeX: {}", line.trim()),
            ));
        }
    }
    report
}

/// A biber message. One about a `.bib` file names biber's temporary UTF-8
/// copy of it, whose line numbers are the original's, so it is moved onto
/// the file biber was reading.
fn biber_diagnostic(severity: Severity, message: &str, bib_source: Option<&str>) -> Diagnostic {
    let located = message
        .strip_prefix("BibTeX subsystem: ")
        .and_then(|rest| rest.split_once(", line "))
        .and_then(|(path, rest)| {
            let (number, text) = rest.split_once(", ")?;
            let file = if path.ends_with(".utf8") {
                bib_source?
            } else {
                path
            };
            Some((file.to_string(), number.parse().ok()?, text))
        });
    match located {
        Some((file, number, text)) => Diagnostic::new(
            severity,
            Some(file),
            Some(number),
            format!("biber: {}", text.trim()),
        ),
        None => Diagnostic::new(severity, None, None, format!("biber: {}", message.trim())),
    }
}
