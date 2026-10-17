//! A small LaTeX lexer: enough to find commands and their arguments without
//! being fooled by comments or verbatim text. It does not expand macros.

/// Environments whose contents are not LaTeX.
const VERBATIM_ENVIRONMENTS: [&str; 6] = [
    "verbatim",
    "verbatim*",
    "Verbatim",
    "lstlisting",
    "minted",
    "comment",
];

/// A source file with comments and verbatim text blanked out. Byte offsets
/// and line numbers are the same as in the original.
pub struct Source {
    text: String,
    line_starts: Vec<usize>,
}

/// One `\name[options]{argument}` occurrence.
#[derive(Debug, Clone, PartialEq)]
pub struct Command<'a> {
    /// The name without the backslash or a trailing `*`.
    pub name: &'a str,
    pub starred: bool,
    /// Contents of each `[...]` before the first mandatory argument.
    pub options: Vec<&'a str>,
    /// Contents of the first `{...}`, if one follows.
    pub argument: Option<&'a str>,
    /// Byte offset of the backslash.
    pub start: usize,
    /// Byte offset just past the last argument.
    pub end: usize,
    /// 1-based line of the backslash.
    pub line: u32,
}

impl Source {
    pub fn new(raw: &str) -> Self {
        let text = blank_verbatim(blank_comments(raw));
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Self { text, line_starts }
    }

    /// The source with comments and verbatim text replaced by spaces.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 1-based line number of a byte offset.
    pub fn line_of(&self, offset: usize) -> u32 {
        self.line_starts.partition_point(|&start| start <= offset) as u32
    }

    /// Every control word in the file, in order. Commands nested inside
    /// another command's argument are returned as well.
    pub fn commands(&self) -> Vec<Command<'_>> {
        let bytes = self.text.as_bytes();
        let mut commands = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'\\' {
                i += 1;
                continue;
            }
            let name_end = i + 1 + letters(&bytes[i + 1..]);
            if name_end == i + 1 {
                // A control symbol such as `\\` or `\%`: skip what it escapes.
                i += 2;
                continue;
            }
            let mut end = name_end;
            let starred = bytes.get(end) == Some(&b'*');
            if starred {
                end += 1;
            }

            let mut options = Vec::new();
            let mut argument = None;
            loop {
                let next = skip_space(bytes, end);
                match bytes.get(next) {
                    Some(b'[') => match matching(bytes, next, b'[', b']') {
                        Some(close) => {
                            options.push(&self.text[next + 1..close]);
                            end = close + 1;
                        }
                        None => break,
                    },
                    Some(b'{') => {
                        if let Some(close) = matching(bytes, next, b'{', b'}') {
                            argument = Some(&self.text[next + 1..close]);
                            end = close + 1;
                        }
                        break;
                    }
                    _ => break,
                }
            }

            commands.push(Command {
                name: &self.text[i + 1..name_end],
                starred,
                options,
                argument,
                start: i,
                end,
                line: self.line_of(i),
            });
            i = name_end;
        }
        commands
    }
}

fn letters(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|b| b.is_ascii_alphabetic()).count()
}

/// Skips spaces and tabs and at most one line break, which is how far TeX
/// looks for an argument.
fn skip_space(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    let mut newlines = 0;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' if newlines == 0 => {
                newlines += 1;
                i += 1;
            }
            _ => break,
        }
    }
    i
}

/// Index of the delimiter that closes the one at `open_at`. Braces nest
/// inside brackets, and backslash-escaped delimiters do not count.
pub(crate) fn matching(bytes: &[u8], open_at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut braces = 0usize;
    let mut i = open_at;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\\' {
            i += 2;
            continue;
        }
        if open != b'{' {
            // Inside [...], a `]` within braces does not close the bracket.
            match byte {
                b'{' => braces += 1,
                b'}' => braces = braces.saturating_sub(1),
                _ => {}
            }
        }
        if braces == 0 {
            if byte == open {
                depth += 1;
            } else if byte == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Replaces every byte in `range` with a space, keeping line breaks.
fn blank(bytes: &mut [u8], range: std::ops::Range<usize>) {
    for byte in &mut bytes[range] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn blank_comments(raw: &str) -> Vec<u8> {
    let mut bytes = raw.as_bytes().to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // `\%` is a percent sign, and `\\%` is a line break and a comment.
            b'\\' => i += 2,
            b'%' => {
                let end = bytes[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| i + n);
                blank(&mut bytes, i..end);
                i = end;
            }
            _ => i += 1,
        }
    }
    bytes
}

fn blank_verbatim(mut bytes: Vec<u8>) -> String {
    for environment in VERBATIM_ENVIRONMENTS {
        let begin = format!("\\begin{{{environment}}}");
        let end = format!("\\end{{{environment}}}");
        let mut from = 0;
        while let Some(start) = find(&bytes, begin.as_bytes(), from) {
            let mut body = start + begin.len();
            // Keep `[label=..., caption=...]`: those options are LaTeX.
            if bytes.get(body) == Some(&b'[')
                && let Some(close) = matching(&bytes, body, b'[', b']')
            {
                body = close + 1;
            }
            let stop = find(&bytes, end.as_bytes(), body).unwrap_or(bytes.len());
            blank(&mut bytes, body..stop);
            from = stop;
        }
    }
    // Blanking only ever turns whole comments or environment bodies into
    // ASCII spaces, and both end at ASCII delimiters, so this stays UTF-8.
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

/// Control words that print as text.
const WORDS: [(&str, &str); 19] = [
    ("TeX", "TeX"),
    ("LaTeX", "LaTeX"),
    ("LaTeXe", "LaTeX2e"),
    ("BibTeX", "BibTeX"),
    ("XeTeX", "XeTeX"),
    ("LuaTeX", "LuaTeX"),
    ("ss", "ss"),
    ("ae", "ae"),
    ("AE", "AE"),
    ("oe", "oe"),
    ("OE", "OE"),
    ("aa", "a"),
    ("AA", "A"),
    ("o", "o"),
    ("O", "O"),
    ("l", "l"),
    ("L", "L"),
    ("i", "i"),
    ("j", "j"),
];

/// Reduces LaTeX markup to readable text: `The {\TeX}book` becomes
/// `The TeXbook`, `\emph{word}` becomes `word`, and accents are dropped.
/// Good for titles and bibliography fields, not for typesetting.
pub fn plain_text(latex: &str) -> String {
    let mut out = String::with_capacity(latex.len());
    let mut chars = latex.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut name = String::new();
                while let Some(&next) = chars.peek().filter(|n| n.is_ascii_alphabetic()) {
                    name.push(next);
                    chars.next();
                }
                if !name.is_empty() {
                    if let Some((_, text)) = WORDS.iter().find(|(word, _)| *word == name) {
                        out.push_str(text);
                    }
                    continue;
                }
                match chars.next() {
                    // Escaped characters that stand for themselves.
                    Some(symbol @ ('&' | '%' | '$' | '#' | '_' | '{' | '}')) => out.push(symbol),
                    Some('\\' | ' ' | ',' | ';' | ':') => out.push(' '),
                    // Anything else is an accent: keep the letter that follows.
                    _ => {}
                }
            }
            '{' | '}' | '$' => {}
            '~' => out.push(' '),
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                if chars.peek() == Some(&'-') {
                    chars.next();
                    out.push('—');
                } else {
                    out.push('–');
                }
            }
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Commands whose argument is a name or a setting, not prose.
const NON_TEXT_COMMANDS: [&str; 28] = [
    "label",
    "input",
    "include",
    "subfile",
    "includegraphics",
    "bibliography",
    "bibliographystyle",
    "addbibresource",
    "usepackage",
    "documentclass",
    "vspace",
    "hspace",
    "setlength",
    "addtolength",
    "setcounter",
    "addtocounter",
    "pagestyle",
    "thispagestyle",
    "pagenumbering",
    "hypersetup",
    "newcommand",
    "renewcommand",
    "url",
    "color",
    "bibitem",
    "put",
    "framebox",
    "vector",
];

/// Environments that hold mathematics or drawings.
const NON_TEXT_ENVIRONMENTS: [&str; 12] = [
    "equation",
    "align",
    "gather",
    "multline",
    "eqnarray",
    "displaymath",
    "math",
    "alignat",
    "flalign",
    "tikzpicture",
    "picture",
    "thebibliography",
];

/// A rough count of the words of prose in a piece of LaTeX. Mathematics,
/// command names, labels, citation keys and the like are left out. Expects
/// text that already has its comments removed (see [`Source::text`]).
pub fn count_words(latex: &str) -> u32 {
    let bytes = latex.as_bytes();
    let mut prose: Vec<u8> = Vec::with_capacity(bytes.len());
    let skip_to = |needle: &str, from: usize| {
        find(bytes, needle.as_bytes(), from).map_or(bytes.len(), |at| at + needle.len())
    };

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let name_end = i + 1 + letters(&bytes[i + 1..]);
                if name_end == i + 1 {
                    i = match bytes.get(i + 1) {
                        Some(b'[') => skip_to("\\]", i + 2),
                        Some(b'(') => skip_to("\\)", i + 2),
                        _ => i + 2,
                    };
                    prose.push(b' ');
                    continue;
                }
                let name = &latex[i + 1..name_end];
                i = name_end;
                if bytes.get(i) == Some(&b'*') {
                    i += 1;
                }
                // Optional arguments are never prose worth counting.
                loop {
                    let next = skip_space(bytes, i);
                    match bytes
                        .get(next)
                        .filter(|&&b| b == b'[')
                        .and_then(|_| matching(bytes, next, b'[', b']'))
                    {
                        Some(close) => i = close + 1,
                        None => break,
                    }
                }
                let next = skip_space(bytes, i);
                let argument = bytes
                    .get(next)
                    .filter(|&&b| b == b'{')
                    .and_then(|_| matching(bytes, next, b'{', b'}'))
                    .map(|close| (&latex[next + 1..close], close + 1));

                if name == "begin" || name == "end" {
                    if let Some((environment, after)) = argument {
                        i = after;
                        let base = environment.trim_end_matches('*');
                        if name == "begin" && NON_TEXT_ENVIRONMENTS.contains(&base) {
                            i = skip_to(&format!("\\end{{{environment}}}"), i);
                        }
                    }
                } else if (NON_TEXT_COMMANDS.contains(&name)
                    || name.to_ascii_lowercase().contains("cite")
                    || name.ends_with("ref"))
                    && let Some((_, after)) = argument
                {
                    i = after;
                }
                prose.push(b' ');
            }
            b'$' => {
                let double = bytes.get(i + 1) == Some(&b'$');
                i = if double {
                    skip_to("$$", i + 2)
                } else {
                    skip_to("$", i + 1)
                };
                prose.push(b' ');
            }
            b'{' | b'}' | b'~' | b'&' => {
                prose.push(b' ');
                i += 1;
            }
            byte => {
                prose.push(byte);
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&prose)
        .split_whitespace()
        // A word starts with a letter; "12", "5.6" and "1mm" are not prose.
        .filter(|token| {
            token
                .chars()
                .find(|c| c.is_alphanumeric())
                .is_some_and(char::is_alphabetic)
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &Source) -> Vec<&str> {
        source.commands().iter().map(|c| c.name).collect()
    }

    #[test]
    fn comments_are_ignored_but_escaped_percent_is_not_a_comment() {
        let source = Source::new("50\\% of \\alpha % \\hidden\n\\beta\\\\% \\gone\n\\gamma");
        assert_eq!(names(&source), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn verbatim_bodies_are_ignored() {
        let source = Source::new(
            "\\section{A}\n\\begin{verbatim}\n\\section{Not a heading}\n\\end{verbatim}\n\\section{B}",
        );
        let headings: Vec<_> = source
            .commands()
            .into_iter()
            .filter(|c| c.name == "section")
            .map(|c| (c.argument.unwrap(), c.line))
            .collect();
        assert_eq!(headings, [("A", 1), ("B", 5)]);
    }

    #[test]
    fn arguments_options_and_stars() {
        let source = Source::new("\\chapter*[Short]{Long \\emph{title}}\n\\item text");
        let commands = source.commands();
        assert_eq!(commands[0].name, "chapter");
        assert!(commands[0].starred);
        assert_eq!(commands[0].options, ["Short"]);
        assert_eq!(commands[0].argument, Some("Long \\emph{title}"));
        // Commands inside an argument are found too.
        assert_eq!(commands[1].name, "emph");
        assert_eq!(commands[1].argument, Some("title"));
        assert_eq!(commands[2].name, "item");
        assert_eq!(commands[2].argument, None);
        assert_eq!(commands[2].line, 2);
    }

    #[test]
    fn two_optional_arguments_and_brackets_inside_braces() {
        let source = Source::new("\\citep[see][p.~5]{knuth}\n\\includegraphics[width={0.5]}]{fig}");
        let commands = source.commands();
        assert_eq!(commands[0].options, ["see", "p.~5"]);
        assert_eq!(commands[0].argument, Some("knuth"));
        assert_eq!(commands[1].options, ["width={0.5]}"]);
        assert_eq!(commands[1].argument, Some("fig"));
    }

    #[test]
    fn an_argument_may_start_on_the_next_line_but_not_after_a_blank_line() {
        let source = Source::new("\\section\n{Title}\n\\foo\n\n{not an argument}");
        let commands = source.commands();
        assert_eq!(commands[0].argument, Some("Title"));
        assert_eq!(commands[1].argument, None);
    }

    #[test]
    fn plain_text_strips_markup() {
        assert_eq!(plain_text("The {\\TeX}book"), "The TeXbook");
        assert_eq!(plain_text("The \\TeX{} Pipeline"), "The TeX Pipeline");
        assert_eq!(
            plain_text("{\\LaTeX}: A  Document\nPreparation System"),
            "LaTeX: A Document Preparation System"
        );
        assert_eq!(plain_text("\\emph{Make} --- A Program"), "Make — A Program");
        assert_eq!(plain_text("97--111"), "97–111");
        assert_eq!(
            plain_text("{\\L}ukasz Kaiser and G{\\\"o}del"),
            "Lukasz Kaiser and Godel"
        );
        assert_eq!(plain_text("Barnes \\& Noble, 50\\%"), "Barnes & Noble, 50%");
        // Arguments of unknown commands are kept: better too much than too little.
        assert_eq!(plain_text("Chapter~\\ref{x}"), "Chapter x");
    }

    #[test]
    fn word_count_leaves_out_markup_and_mathematics() {
        assert_eq!(count_words("Four plain words here."), 4);
        assert_eq!(
            count_words("See Chapter~\\ref{chap:x} and \\citep{knuth1984texbook}."),
            3
        );
        assert_eq!(count_words("An \\emph{emphasized} word."), 3);
        assert_eq!(count_words("If $N = 2^k$ then \\[ x = y \\] holds."), 3);
        assert_eq!(
            count_words(
                "Before.\n\\begin{equation}\n  I = \\log_2 N\n  \\label{eq:i}\n\\end{equation}\nAfter."
            ),
            2
        );
        assert_eq!(
            count_words("\\begin{itemize}\n  \\item first item\n  \\item second\n\\end{itemize}"),
            3
        );
        assert_eq!(count_words("12 34 --- 5.6 1mm"), 0);
        assert_eq!(count_words("``Quoted'' (parenthesized) words."), 3);
    }
}
