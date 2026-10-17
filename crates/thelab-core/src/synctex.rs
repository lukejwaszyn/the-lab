//! SyncTeX: which place in the PDF a source line produced, and which source
//! line a place in the PDF came from.
//!
//! Both engines write `build/<main>.synctex.gz` when asked to. This reads
//! that file directly; the `synctex` program from TeX Live is not needed,
//! and people with only Tectonic do not have it.
//!
//! Positions are in PDF points measured from the top-left corner of the
//! page, which is how SyncTeX itself counts.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::compile::BUILD_DIRECTORY;
use crate::scan::Project;
use crate::tools::{find_program, run};
use crate::{Error, Result};

/// TeX's scaled points per PDF point: 65536 sp to a TeX point, 72.27 TeX
/// points to the inch, 72 PDF points to the inch.
const SP_PER_BP: f64 = 65536.0 * 72.27 / 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    VBox,
    HBox,
    /// A box with nothing in it, recorded on one line.
    VoidBox,
    /// Kern, glue, math switch or current point: a place within a line.
    Point,
}

#[derive(Debug, Clone)]
struct Record {
    kind: Kind,
    /// Which input file, by the number SyncTeX gave it.
    tag: u32,
    line: u32,
    h: f64,
    v: f64,
    width: f64,
    height: f64,
    depth: f64,
    /// The innermost box this record is inside, as an index into the page.
    parent: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct Page {
    number: u32,
    records: Vec<Record>,
}

/// The contents of one SyncTeX file.
#[derive(Debug, Clone, Default)]
pub struct SyncTex {
    /// Input number to path, as the engine wrote it.
    inputs: BTreeMap<u32, String>,
    pages: Vec<Page>,
}

/// A rectangle on a page, in PDF points from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where a source line ended up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forward {
    /// 1-based page of the PDF.
    pub page: u32,
    /// The point on the baseline where the line's output starts.
    pub x: f64,
    pub y: f64,
    /// The typeset line that contains it: what to highlight.
    #[serde(rename = "box")]
    pub line_box: Rect,
    /// The source line that was matched, which is the nearest one with any
    /// output if the requested line has none (a comment, a blank line).
    pub line: u32,
}

/// Where a place in the PDF came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inverse {
    /// Relative to the project root for project files, else absolute.
    pub file: String,
    pub line: u32,
}

impl SyncTex {
    /// Parses the text of a SyncTeX file (already decompressed).
    pub fn parse(text: &str) -> Self {
        let mut sync = SyncTex::default();
        let mut unit = 1.0;
        let mut magnification = 1000.0;
        let mut offset = (0.0, 0.0);
        let mut page: Option<Page> = None;
        // Indices of the boxes that are open, innermost last.
        let mut open: Vec<usize> = Vec::new();

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Input:") {
                // Input lines also appear between pages, as files are opened.
                if let Some((tag, path)) = rest.split_once(':')
                    && let Ok(tag) = tag.parse()
                {
                    sync.inputs.insert(tag, path.to_string());
                }
                continue;
            }
            if page.is_none() {
                if let Some(value) = line.strip_prefix("Unit:") {
                    unit = value.trim().parse().unwrap_or(1.0);
                } else if let Some(value) = line.strip_prefix("Magnification:") {
                    magnification = value.trim().parse().unwrap_or(1000.0);
                } else if let Some(value) = line.strip_prefix("X Offset:") {
                    offset.0 = value.trim().parse().unwrap_or(0.0);
                } else if let Some(value) = line.strip_prefix("Y Offset:") {
                    offset.1 = value.trim().parse().unwrap_or(0.0);
                }
            }
            let Some(marker) = line.chars().next() else {
                continue;
            };
            let rest = &line[marker.len_utf8()..];
            match marker {
                '{' => {
                    open.clear();
                    page = rest.parse().ok().map(|number| Page {
                        number,
                        records: Vec::new(),
                    });
                }
                '}' => {
                    sync.pages.extend(page.take());
                }
                ']' | ')' => {
                    open.pop();
                }
                '[' | '(' | 'v' | 'h' | 'k' | 'g' | '$' | 'x' => {
                    let Some(page) = page.as_mut() else { continue };
                    let kind = match marker {
                        '[' => Kind::VBox,
                        '(' => Kind::HBox,
                        'v' | 'h' => Kind::VoidBox,
                        _ => Kind::Point,
                    };
                    // To PDF points, the way the SyncTeX parser scales.
                    let scale = unit * magnification / 1000.0 / SP_PER_BP;
                    let Some(mut record) = parse_record(rest, kind, scale) else {
                        continue;
                    };
                    record.h += offset.0 * scale;
                    record.v += offset.1 * scale;
                    record.parent = open.last().copied();
                    page.records.push(record);
                    if matches!(kind, Kind::VBox | Kind::HBox) {
                        open.push(page.records.len() - 1);
                    }
                }
                _ => {}
            }
        }
        sync
    }

    /// Finds where `line` of `file` (relative to `root`) is on the page.
    pub fn forward(&self, root: &Path, file: &str, line: u32) -> Option<Forward> {
        let tags = self.tags_for(root, file);
        if tags.is_empty() {
            return None;
        }
        let from_file = |record: &&Record| tags.contains(&record.tag) && record.line > 0;

        // The requested line, or failing that the nearest line that produced
        // anything; of two equally near, the later one.
        let target = self
            .pages
            .iter()
            .flat_map(|page| page.records.iter())
            .filter(from_file)
            .map(|record| record.line)
            .min_by_key(|&candidate| (candidate.abs_diff(line), std::cmp::Reverse(candidate)))?;

        // On the first page that has it. A place within a line says more
        // than the box around a paragraph, so prefer those.
        let page = self.pages.iter().find(|page| {
            page.records
                .iter()
                .any(|r| from_file(&r) && r.line == target)
        })?;
        let at_line = |record: &&Record| from_file(record) && record.line == target;
        let index = page
            .records
            .iter()
            .position(|r| at_line(&r) && r.kind == Kind::Point)
            .or_else(|| {
                page.records
                    .iter()
                    .position(|r| at_line(&r) && r.kind == Kind::HBox)
            })
            .or_else(|| page.records.iter().position(|r| at_line(&r)))?;
        let record = &page.records[index];

        let line_box = match enclosing_hbox(page, index) {
            Some(hbox) if hbox.width > 0.0 => rect_of(hbox),
            // Nothing sensible to outline: a line's height at the point.
            _ => Rect {
                x: record.h,
                y: record.v - 10.0,
                width: 0.0,
                height: 12.0,
            },
        };
        Some(Forward {
            page: page.number,
            x: record.h,
            y: record.v,
            line_box,
            line: target,
        })
    }

    /// Finds the source line behind the point (`x`, `y`) of a page.
    pub fn inverse(&self, root: &Path, page: u32, x: f64, y: f64) -> Option<Inverse> {
        let page = self
            .pages
            .iter()
            .find(|candidate| candidate.number == page)?;
        let boxes = || {
            page.records
                .iter()
                .enumerate()
                .filter(|(_, r)| r.kind == Kind::HBox && r.width > 0.0 && r.height + r.depth > 0.0)
        };

        // The smallest line box around the point, or the nearest one if the
        // click fell between lines.
        let (index, hbox) = boxes()
            .filter(|(_, r)| distance(rect_of(r), x, y) == 0.0)
            .min_by(|a, b| area(a.1).total_cmp(&area(b.1)))
            .or_else(|| {
                boxes()
                    .map(|(i, r)| (i, r, distance(rect_of(r), x, y)))
                    .filter(|(_, _, far)| *far <= 36.0)
                    .min_by(|a, b| a.2.total_cmp(&b.2).then(area(a.1).total_cmp(&area(b.1))))
                    .map(|(i, r, _)| (i, r))
            })?;

        // Within the line, the recorded place nearest the click.
        let nearest = page
            .records
            .iter()
            .filter(|r| r.parent == Some(index) && r.kind == Kind::Point && r.line > 0)
            .min_by(|a, b| (a.h - x).abs().total_cmp(&(b.h - x).abs()));
        [nearest, Some(hbox)]
            .into_iter()
            .flatten()
            .find_map(|record| {
                Some(Inverse {
                    file: self.project_path(root, record.tag)?,
                    line: record.line,
                })
            })
    }

    /// The input numbers that stand for `file`. A file read twice has two.
    fn tags_for(&self, root: &Path, file: &str) -> Vec<u32> {
        let wanted = clean(&root.join(file).to_string_lossy());
        let exact: Vec<u32> = self
            .inputs
            .iter()
            .filter(|(_, path)| clean(path) == wanted)
            .map(|(tag, _)| *tag)
            .collect();
        if !exact.is_empty() {
            return exact;
        }
        // The project may have moved since it was compiled: match the tail.
        let suffix = format!("/{}", clean(file));
        self.inputs
            .iter()
            .filter(|(_, path)| clean(path).ends_with(&suffix))
            .map(|(tag, _)| *tag)
            .collect()
    }

    /// The path of an input, if it names a file: Tectonic leaves the names
    /// of the files it supplies itself empty.
    fn project_path(&self, root: &Path, tag: u32) -> Option<String> {
        let path = clean(self.inputs.get(&tag)?);
        if path.is_empty() {
            return None;
        }
        let root = clean(&root.to_string_lossy());
        Some(match path.strip_prefix(&format!("{root}/")) {
            Some(relative) => relative.to_string(),
            None => path,
        })
    }
}

/// Parses `tag,line[,column]:h,v[:W[,H,D]]`, scaling lengths to points.
fn parse_record(text: &str, kind: Kind, scale: f64) -> Option<Record> {
    let mut parts = text.split(':');
    let mut source = parts.next()?.split(',');
    let tag = source.next()?.parse().ok()?;
    let line = source.next()?.parse().ok()?;
    let mut position = parts.next()?.split(',');
    let h = position.next()?.parse::<f64>().ok()? * scale;
    let v = position.next()?.parse::<f64>().ok()? * scale;
    let mut size = parts
        .next()
        .unwrap_or("")
        .split(',')
        .map(|n| n.parse::<f64>().unwrap_or(0.0) * scale);
    let (width, height, depth) = (
        size.next().unwrap_or(0.0),
        size.next().unwrap_or(0.0),
        size.next().unwrap_or(0.0),
    );
    Some(Record {
        kind,
        tag,
        line,
        h,
        v,
        width,
        height,
        depth,
        parent: None,
    })
}

/// The line box a record sits in: itself if it is one.
fn enclosing_hbox(page: &Page, index: usize) -> Option<&Record> {
    let mut current = Some(index);
    while let Some(at) = current {
        let record = &page.records[at];
        if record.kind == Kind::HBox {
            return Some(record);
        }
        current = record.parent;
    }
    None
}

/// A box's rectangle. `v` is its baseline: it rises `height` above and
/// drops `depth` below.
fn rect_of(record: &Record) -> Rect {
    Rect {
        x: record.h,
        y: record.v - record.height,
        width: record.width,
        height: record.height + record.depth,
    }
}

fn area(record: &Record) -> f64 {
    record.width * (record.height + record.depth)
}

/// How far a point is from a rectangle; zero inside it.
fn distance(rect: Rect, x: f64, y: f64) -> f64 {
    let dx = (rect.x - x).max(x - (rect.x + rect.width)).max(0.0);
    let dy = (rect.y - y).max(y - (rect.y + rect.height)).max(0.0);
    dx.hypot(dy)
}

/// Removes `.` and `..` from a path without touching the disk.
/// pdflatex writes `/project/./main.tex`.
fn clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if path.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Reads the SyncTeX data of a project's last compile.
pub fn load(project: &Project) -> Result<SyncTex> {
    let main = project.require_main()?;
    let stem = Path::new(main)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");
    let build = project.root.join(BUILD_DIRECTORY);
    let compressed = build.join(format!("{stem}.synctex.gz"));
    let plain = build.join(format!("{stem}.synctex"));

    if plain.is_file() {
        return crate::scan::read_text(&plain).map(|text| SyncTex::parse(&text));
    }
    if !compressed.is_file() {
        return Err(Error::new(format!(
            "No SyncTeX data at {}. Compile the document first.",
            compressed.display()
        )));
    }
    // gzip is part of every macOS and Linux; using it saves linking a
    // decompression library for one small file.
    let gzip = find_program("gzip")
        .ok_or_else(|| Error::new("gzip was not found, so the SyncTeX data cannot be read."))?;
    let output = run(
        Command::new(gzip).arg("-dc").arg(&compressed),
        Duration::from_secs(30),
    )
    .map_err(|e| Error::new(format!("Cannot run gzip: {e}")))?;
    if !output.status.is_some_and(|status| status.success()) {
        return Err(Error::new(format!(
            "Cannot decompress {}: {}",
            compressed.display(),
            output.stderr.trim()
        )));
    }
    Ok(SyncTex::parse(&output.stdout_text()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_cleaned_without_touching_the_disk() {
        assert_eq!(
            clean("/project/./chapters/../main.tex"),
            "/project/main.tex"
        );
        assert_eq!(clean("chapters/./one.tex"), "chapters/one.tex");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn a_small_file_by_hand() {
        // One page: a line box from x=72 to x=372, baseline at y=100, ten
        // points high and two deep, with three places inside it.
        let sp = |points: f64| (points * SP_PER_BP).round() as i64;
        let text = format!(
            "SyncTeX Version:1\nInput:1:/p/./main.tex\nInput:2:\nOutput:pdf\nMagnification:1000\nUnit:1\n\
             X Offset:0\nY Offset:0\nContent:\n!10\n{{1\n[1,9:{l},{b}:{w},{h},0\n({t},9:{l},{y}:{w},{h},{d}\n\
             g1,7:{x1},{y}\nk1,8:{x2},{y}:100\n$2,3:{x3},{y}\n)\n]\n}}1\nPostamble:\nCount:5\n",
            l = sp(72.0),
            b = sp(700.0),
            w = sp(300.0),
            h = sp(10.0),
            d = sp(2.0),
            y = sp(100.0),
            x1 = sp(80.0),
            x2 = sp(200.0),
            x3 = sp(300.0),
            t = 1,
        );
        let sync = SyncTex::parse(&text);
        let root = Path::new("/p");

        let forward = sync.forward(root, "main.tex", 8).expect("line 8");
        assert_eq!(forward.page, 1);
        assert!(
            (forward.x - 200.0).abs() < 0.01 && (forward.y - 100.0).abs() < 0.01,
            "{forward:?}"
        );
        let b = forward.line_box;
        assert!(
            (b.x - 72.0).abs() < 0.01 && (b.y - 90.0).abs() < 0.01,
            "{b:?}"
        );
        assert!(
            (b.width - 300.0).abs() < 0.01 && (b.height - 12.0).abs() < 0.01,
            "{b:?}"
        );
        // Line 6 produced nothing; line 7 is the nearest that did.
        assert_eq!(sync.forward(root, "main.tex", 6).map(|f| f.line), Some(7));
        assert_eq!(sync.forward(root, "other.tex", 1), None);

        let at = |x: f64, y: f64| sync.inverse(root, 1, x, y).map(|i| (i.file, i.line));
        assert_eq!(at(85.0, 95.0), Some(("main.tex".to_string(), 7)));
        assert_eq!(at(210.0, 99.0), Some(("main.tex".to_string(), 8)));
        // The nearest place there belongs to a file with no name (one
        // Tectonic supplied), so the line box answers instead.
        assert_eq!(at(310.0, 99.0), Some(("main.tex".to_string(), 9)));
        // Just under the line still finds it; far away finds nothing.
        assert_eq!(at(210.0, 110.0), Some(("main.tex".to_string(), 8)));
        assert_eq!(at(210.0, 400.0), None);
        assert_eq!(sync.inverse(root, 2, 210.0, 99.0), None);
    }
}
