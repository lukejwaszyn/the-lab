//! Reads BibTeX databases and searches them.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::Result;
use crate::scan::read_text;
use crate::tex::plain_text;

/// One entry of a `.bib` file. Field values are plain text: braces, TeX
/// accents and macros are resolved or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BibEntry {
    pub key: String,
    /// `book`, `article`, ... in lower case.
    #[serde(rename = "type")]
    pub kind: String,
    pub file: String,
    pub line: u32,
    /// Field names are lower case.
    pub fields: BTreeMap<String, String>,
}

impl BibEntry {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// `Author (year). Title.`: enough to recognize the work.
    pub fn summary(&self) -> String {
        let who = self
            .field("author")
            .or(self.field("editor"))
            .unwrap_or("Unknown author");
        let year = self.field("year").or(self.field("date")).unwrap_or("n.d.");
        let title = self.field("title").unwrap_or("Untitled");
        format!("{who} ({year}). {title}.")
    }
}

/// Reads every entry of the given `.bib` files (paths relative to `root`).
/// Files that cannot be read are skipped; `check` reports those.
pub fn load(root: &Path, files: &[String]) -> Result<Vec<BibEntry>> {
    let mut entries = Vec::new();
    for file in files {
        if let Ok(text) = read_text(&root.join(file)) {
            entries.extend(parse_bib(&text, file));
        }
    }
    Ok(entries)
}

/// Parses the text of a `.bib` file. Malformed entries are skipped.
pub fn parse_bib(text: &str, file: &str) -> Vec<BibEntry> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        at: 0,
        macros: month_macros(),
    };
    let mut entries = Vec::new();
    while let Some(start) = parser.next_entry_start() {
        let line = text[..start].matches('\n').count() as u32 + 1;
        if let Some((kind, key, fields)) = parser.entry() {
            entries.push(BibEntry {
                key,
                kind,
                file: file.to_string(),
                line,
                fields,
            });
        }
    }
    entries
}

fn month_macros() -> BTreeMap<String, String> {
    [
        ("jan", "January"),
        ("feb", "February"),
        ("mar", "March"),
        ("apr", "April"),
        ("may", "May"),
        ("jun", "June"),
        ("jul", "July"),
        ("aug", "August"),
        ("sep", "September"),
        ("oct", "October"),
        ("nov", "November"),
        ("dec", "December"),
    ]
    .into_iter()
    .map(|(short, long)| (short.to_string(), long.to_string()))
    .collect()
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
    /// `@string` definitions, plus the standard month names.
    macros: BTreeMap<String, String>,
}

impl<'a> Parser<'a> {
    /// Moves to the next `@` and returns its offset. Text between entries is
    /// a comment in BibTeX.
    fn next_entry_start(&mut self) -> Option<usize> {
        let offset = self.bytes.get(self.at..)?.iter().position(|&b| b == b'@')?;
        let start = self.at + offset;
        self.at = start + 1;
        Some(start)
    }

    /// Parses what follows an `@`. Returns `None` for `@comment`, `@string`,
    /// `@preamble` and anything malformed.
    fn entry(&mut self) -> Option<(String, String, BTreeMap<String, String>)> {
        let kind = self.word().to_ascii_lowercase();
        self.skip_space();
        let (open, close) = match self.peek()? {
            b'{' => (b'{', b'}'),
            b'(' => (b'(', b')'),
            _ => return None,
        };
        let open_at = self.at;
        self.at += 1;

        match kind.as_str() {
            "comment" | "preamble" => {
                // Skip the whole body, which may itself look like entries.
                if let Some(end) = crate::tex::matching(self.bytes, open_at, open, close) {
                    self.at = end + 1;
                }
                return None;
            }
            "string" => {
                let name = self.word().to_ascii_lowercase();
                self.skip_space();
                if self.peek()? == b'=' {
                    self.at += 1;
                    let value = self.value()?;
                    self.macros.insert(name, value);
                }
                return None;
            }
            "" => return None,
            _ => {}
        }

        self.skip_space();
        let key_start = self.at;
        while self
            .peek()
            .is_some_and(|b| b != b',' && b != close && !b.is_ascii_whitespace())
        {
            self.at += 1;
        }
        let key = self.text[key_start..self.at].to_string();
        if key.is_empty() {
            return None;
        }

        let mut fields = BTreeMap::new();
        loop {
            self.skip_space();
            match self.peek()? {
                b',' => self.at += 1,
                byte if byte == close => {
                    self.at += 1;
                    break;
                }
                _ => {
                    let name = self.word().to_ascii_lowercase();
                    self.skip_space();
                    if name.is_empty() || self.peek()? != b'=' {
                        return None;
                    }
                    self.at += 1;
                    let value = self.value()?;
                    fields.entry(name).or_insert_with(|| plain_text(&value));
                }
            }
        }
        Some((kind, key, fields))
    }

    /// A field value: pieces joined by `#`, each of them `{braced}`,
    /// `"quoted"`, a number or a macro name.
    fn value(&mut self) -> Option<String> {
        let mut value = String::new();
        loop {
            self.skip_space();
            match self.peek()? {
                b'{' => {
                    let close = crate::tex::matching(self.bytes, self.at, b'{', b'}')?;
                    value.push_str(&self.text[self.at + 1..close]);
                    self.at = close + 1;
                }
                b'"' => {
                    let start = self.at + 1;
                    let mut depth = 0usize;
                    let mut i = start;
                    loop {
                        match *self.bytes.get(i)? {
                            b'\\' => i += 1,
                            b'{' => depth += 1,
                            b'}' => depth = depth.saturating_sub(1),
                            b'"' if depth == 0 => break,
                            _ => {}
                        }
                        i += 1;
                    }
                    value.push_str(&self.text[start..i]);
                    self.at = i + 1;
                }
                _ => {
                    let word = self.word();
                    if word.is_empty() {
                        return None;
                    }
                    match self.macros.get(&word.to_ascii_lowercase()) {
                        Some(expansion) => value.push_str(expansion),
                        None => value.push_str(word),
                    }
                }
            }
            self.skip_space();
            if self.peek() == Some(b'#') {
                self.at += 1;
            } else {
                return Some(value);
            }
        }
    }

    /// A run of characters that can form a name or a number.
    fn word(&mut self) -> &'a str {
        let text = self.text;
        let start = self.at;
        while self.peek().is_some_and(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.' | b'+' | b'/')
        }) {
            self.at += 1;
        }
        &text[start..self.at]
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }
}

/// A search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match<'a> {
    pub entry: &'a BibEntry,
    /// How many of the query's terms the entry contains.
    pub matched_terms: usize,
}

/// The outcome of a search: either entries that contain every term of the
/// query, or, when there are none, the closest partial matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult<'a> {
    pub terms: Vec<String>,
    /// False when no entry contains all the terms.
    pub exact: bool,
    pub matches: Vec<Match<'a>>,
}

/// Searches entries by free text: author names, title words, a year, a key.
/// Matching ignores case, accents and punctuation.
pub fn search<'a>(entries: &'a [BibEntry], query: &str, limit: usize) -> SearchResult<'a> {
    let terms: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
    if terms.is_empty() {
        return SearchResult {
            terms,
            exact: true,
            matches: Vec::new(),
        };
    }

    let mut matches: Vec<Match> = entries
        .iter()
        .filter_map(|entry| {
            let mut haystack = fold(&entry.key);
            for value in entry.fields.values() {
                haystack.push(' ');
                haystack.push_str(&fold(value));
            }
            let matched_terms = terms
                .iter()
                .filter(|term| haystack.contains(term.as_str()))
                .count();
            (matched_terms > 0).then_some(Match {
                entry,
                matched_terms,
            })
        })
        .collect();

    let exact = matches.iter().any(|m| m.matched_terms == terms.len());
    if exact {
        matches.retain(|m| m.matched_terms == terms.len());
    }
    // Best first; the sort is stable, so ties keep the order of the files.
    matches.sort_by_key(|m| std::cmp::Reverse(m.matched_terms));
    matches.truncate(limit);
    SearchResult {
        terms,
        exact,
        matches,
    }
}

/// Lower case, without accents, with punctuation turned into spaces.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'à'..='å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'ç' | 'ć' | 'č' => out.push('c'),
            'è'..='ë' | 'ē' | 'ę' | 'ě' => out.push('e'),
            'ì'..='ï' | 'ī' | 'ı' => out.push('i'),
            'ñ' | 'ń' | 'ň' => out.push('n'),
            'ò'..='ö' | 'ø' | 'ő' => out.push('o'),
            'ù'..='ü' | 'ū' | 'ů' | 'ű' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ś' | 'š' | 'ş' => out.push('s'),
            'ź' | 'ž' | 'ż' => out.push('z'),
            'ł' => out.push('l'),
            'ř' => out.push('r'),
            'ğ' => out.push('g'),
            'ß' => out.push_str("ss"),
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
This line is a comment, as is anything outside an entry.

@string{aw = "Addison-Wesley"}
@comment{ @book{ignored, title = {Not an entry}} }

@book{knuth1984texbook,
  author    = {Donald E. Knuth},
  title     = {The {\TeX}book},
  publisher = aw,
  year      = 1984,
}

@ARTICLE(godel1931,
  author  = "Kurt G{\"o}del",
  title   = "{\"U}ber formal unentscheidbare S{\"a}tze",
  journal = "Monatshefte f{\"u}r Mathematik" # " und Physik",
  month   = jan,
  year    = "1931"
)

@misc{broken, title = {No closing brace}

@inproceedings{vaswani2017attention,
  author    = {Ashish Vaswani and {\L}ukasz Kaiser},
  title     = {Attention Is All You Need},
  booktitle = {Advances in Neural Information Processing Systems 30},
  year      = {2017}
}
"#;

    fn sample() -> Vec<BibEntry> {
        parse_bib(SAMPLE, "refs.bib")
    }

    #[test]
    fn parses_entries_with_their_location() {
        let entries = sample();
        let found: Vec<(&str, &str, u32)> = entries
            .iter()
            .map(|e| (e.key.as_str(), e.kind.as_str(), e.line))
            .collect();
        assert_eq!(
            found,
            [
                ("knuth1984texbook", "book", 7),
                ("godel1931", "article", 14),
                ("vaswani2017attention", "inproceedings", 24),
            ]
        );
    }

    #[test]
    fn values_are_plain_text() {
        let entries = sample();
        let knuth = &entries[0];
        assert_eq!(knuth.field("title"), Some("The TeXbook"));
        assert_eq!(
            knuth.field("publisher"),
            Some("Addison-Wesley"),
            "@string macro"
        );
        assert_eq!(knuth.field("year"), Some("1984"), "bare number");

        let godel = &entries[1];
        assert_eq!(godel.field("author"), Some("Kurt Godel"));
        assert_eq!(
            godel.field("journal"),
            Some("Monatshefte fur Mathematik und Physik"),
            "# joins pieces"
        );
        assert_eq!(godel.field("month"), Some("January"));
        assert_eq!(
            godel.summary(),
            "Kurt Godel (1931). Uber formal unentscheidbare Satze."
        );
    }

    #[test]
    fn search_needs_every_term() {
        let entries = sample();
        let keys = |query: &str| -> Vec<&str> {
            search(&entries, query, 10)
                .matches
                .iter()
                .map(|m| m.entry.key.as_str())
                .collect()
        };
        assert_eq!(keys("knuth"), ["knuth1984texbook"]);
        assert_eq!(keys("Attention is all you need"), ["vaswani2017attention"]);
        assert_eq!(keys("kaiser 2017"), ["vaswani2017attention"]);
        assert_eq!(keys("GÖDEL"), ["godel1931"], "case and accents are ignored");
        assert_eq!(
            keys("texbook, 1984."),
            ["knuth1984texbook"],
            "punctuation is ignored"
        );
    }

    #[test]
    fn search_falls_back_to_partial_matches_and_says_so() {
        let entries = sample();
        let result = search(&entries, "knuth 2017", 10);
        assert!(!result.exact);
        let keys: Vec<&str> = result
            .matches
            .iter()
            .map(|m| m.entry.key.as_str())
            .collect();
        assert_eq!(keys, ["knuth1984texbook", "vaswani2017attention"]);

        let nothing = search(&entries, "feynman", 10);
        assert!(nothing.matches.is_empty());
        assert!(!nothing.exact);
    }
}
