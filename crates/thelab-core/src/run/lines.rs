//! Turns a script's output, read from a pipe in arbitrary chunks, into the
//! lines the Output panel shows.
//!
//! There is no terminal, so the few things a terminal does that scripts
//! rely on are done here: a carriage return starts the line again (progress
//! bars), a line still being written is shown as it grows, and colour codes
//! are dropped.

/// A line longer than this is cut, so one runaway line cannot eat memory.
pub const MAX_LINE_BYTES: usize = 8 * 1024;

/// A line of output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    /// The line is still being written: the next segment from the same
    /// stream replaces it rather than following it.
    pub open: bool,
}

/// Splits one stream (stdout or stderr) into lines.
#[derive(Debug, Default)]
pub struct LineSplitter {
    line: Vec<u8>,
    /// A carriage return came: the next character starts the line afresh.
    restart: bool,
    /// The line as it is now was already given out as an open segment.
    shown: bool,
}

impl LineSplitter {
    /// The segments that `bytes` completes or changes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Segment> {
        let mut out = Vec::new();
        for &byte in bytes {
            match byte {
                b'\n' => self.end_line(&mut out),
                b'\r' => self.restart = true,
                _ => {
                    if self.restart {
                        self.restart = false;
                        self.line.clear();
                    }
                    self.line.push(byte);
                    self.shown = false;
                    if self.line.len() >= MAX_LINE_BYTES {
                        self.cut(&mut out);
                    }
                }
            }
        }
        if !self.line.is_empty() && !self.shown {
            out.push(Segment {
                text: decode(complete_prefix(&self.line)),
                open: true,
            });
            self.shown = true;
        }
        out
    }

    /// The last line, when the stream ended without a newline.
    pub fn finish(&mut self) -> Vec<Segment> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut out);
        }
        self.restart = false;
        out
    }

    fn end_line(&mut self, out: &mut Vec<Segment>) {
        out.push(Segment {
            text: decode(&self.line),
            open: false,
        });
        self.line.clear();
        self.restart = false;
        self.shown = false;
    }

    /// Ends an overlong line at a character boundary; the rest starts the next.
    fn cut(&mut self, out: &mut Vec<Segment>) {
        let at = complete_prefix(&self.line).len().max(1);
        let rest = self.line.split_off(at);
        self.end_line(out);
        self.line = rest;
    }
}

/// `bytes` without a UTF-8 character cut off at its end.
fn complete_prefix(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    }
}

/// Text for display: invalid UTF-8 replaced, escape sequences (colours,
/// cursor moves) and other control characters dropped, tabs kept.
fn decode(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte from @ to ~.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC (a window title, a link): up to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(text: &str) -> Segment {
        Segment {
            text: text.to_string(),
            open: false,
        }
    }

    fn open(text: &str) -> Segment {
        Segment {
            text: text.to_string(),
            open: true,
        }
    }

    /// What each chunk gives, then what the end of the stream gives.
    fn split(chunks: &[&[u8]]) -> Vec<Vec<Segment>> {
        let mut splitter = LineSplitter::default();
        let mut out: Vec<Vec<Segment>> = chunks.iter().map(|c| splitter.push(c)).collect();
        out.push(splitter.finish());
        out
    }

    #[test]
    fn whole_lines_come_out_closed() {
        assert_eq!(
            split(&[b"one\ntwo\n"]),
            [vec![closed("one"), closed("two")], vec![]]
        );
    }

    #[test]
    fn empty_lines_are_kept() {
        assert_eq!(
            split(&[b"\n\nend\n"]),
            [vec![closed(""), closed(""), closed("end")], vec![]]
        );
    }

    #[test]
    fn a_line_still_being_written_is_shown_open_then_closed() {
        assert_eq!(
            split(&[b"Loading", b" data", b"... done\n"]),
            [
                vec![open("Loading")],
                vec![open("Loading data")],
                vec![closed("Loading data... done")],
                vec![],
            ]
        );
    }

    #[test]
    fn a_line_left_without_a_newline_is_closed_at_the_end() {
        assert_eq!(
            split(&[b"no newline"]),
            [vec![open("no newline")], vec![closed("no newline")]]
        );
    }

    #[test]
    fn nothing_new_gives_nothing() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"abc"), [open("abc")]);
        assert_eq!(splitter.push(b""), []);
        assert_eq!(splitter.finish(), [closed("abc")]);
        assert_eq!(splitter.finish(), []);
    }

    #[test]
    fn crlf_is_one_newline_even_split_across_chunks() {
        assert_eq!(
            split(&[b"a\r\nb\r\n"]),
            [vec![closed("a"), closed("b")], vec![]]
        );
        assert_eq!(
            split(&[b"a\r", b"\nb\n"]),
            [vec![open("a")], vec![closed("a"), closed("b")], vec![]]
        );
    }

    #[test]
    fn a_progress_bar_redraws_its_line() {
        // tqdm writes "\r" and the whole bar each time, and "\n" at the end.
        assert_eq!(
            split(&[b"\r 10%|#  |", b"\r 50%|## |", b"\r100%|###|", b"\n"]),
            [
                vec![open(" 10%|#  |")],
                vec![open(" 50%|## |")],
                vec![open("100%|###|")],
                vec![closed("100%|###|")],
                vec![],
            ]
        );
    }

    #[test]
    fn redraws_within_one_chunk_show_only_the_last() {
        assert_eq!(
            split(&[b"step 1\rstep 2\rstep 3\ndone\n"]),
            [vec![closed("step 3"), closed("done")], vec![]]
        );
    }

    #[test]
    fn a_carriage_return_without_new_text_keeps_the_line() {
        // A terminal shows "abc" here: \r only moves the cursor.
        assert_eq!(split(&[b"abc\r\r\n"]), [vec![closed("abc")], vec![]]);
        assert_eq!(split(&[b"abc\r"]), [vec![open("abc")], vec![closed("abc")]]);
    }

    #[test]
    fn colours_and_cursor_codes_are_dropped() {
        assert_eq!(
            split(&[b"\x1b[32mok\x1b[0m \x1b[1;31mFAIL\x1b[0m\n\r\x1b[K42%\n"]),
            [vec![closed("ok FAIL"), closed("42%")], vec![]]
        );
        assert_eq!(
            split(&[b"\x1b]0;title\x07shown\x1b]8;;http://x\x1b\\link\n"]),
            [vec![closed("shownlink")], vec![]]
        );
    }

    #[test]
    fn tabs_stay_and_other_control_characters_go() {
        assert_eq!(
            split(&[b"a\tb\x07\x08c\n"]),
            [vec![closed("a\tbc")], vec![]]
        );
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(
            split(&[b"bad \xff byte\n"]),
            [vec![closed("bad \u{fffd} byte")], vec![]]
        );
    }

    #[test]
    fn a_character_cut_between_chunks_is_not_shown_broken() {
        assert_eq!(
            split(&[b"caf\xc3", b"\xa9\n"]),
            [vec![open("caf")], vec![closed("café")], vec![]]
        );
    }

    #[test]
    fn an_overlong_line_is_cut_at_a_character_boundary() {
        // 3-byte characters do not divide MAX_LINE_BYTES evenly.
        let long = "€".repeat(MAX_LINE_BYTES / 3 + 10);
        let mut input = long.clone().into_bytes();
        input.push(b'\n');
        let out = split(&[&input]);
        let first = &out[0][0];
        assert!(!first.open);
        assert!(first.text.len() <= MAX_LINE_BYTES);
        assert!(first.text.chars().all(|c| c == '€'));
        let joined: String = out[0].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, long);
    }
}
