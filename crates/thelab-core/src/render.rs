//! Renders a page of a PDF to PNG by calling an external renderer.
//!
//! Poppler (`pdftoppm`) is GPL and MuPDF (`mutool`) is AGPL, so neither is
//! linked: they are run as separate programs if the user has them installed.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::tools::{find_program, run};
use crate::{Error, Result};

pub const DEFAULT_DPI: u32 = 110;
pub const MIN_DPI: u32 = 36;
pub const MAX_DPI: u32 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub page: u32,
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    /// The program that drew the page.
    pub renderer: &'static str,
}

/// Number of pages in a PDF, if `pdfinfo` is installed to ask.
pub fn page_count(pdf: &Path) -> Option<u32> {
    let pdfinfo = find_program("pdfinfo")?;
    let output = run(Command::new(pdfinfo).arg(pdf), Duration::from_secs(30)).ok()?;
    output
        .stdout_text()
        .lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse().ok())
}

/// Renders one page (1-based) of `pdf` at `dpi` dots per inch.
pub fn render_page(pdf: &Path, page: u32, dpi: u32) -> Result<RenderedPage> {
    if !pdf.is_file() {
        return Err(Error::new(format!(
            "No PDF at {}. Compile the document first.",
            pdf.display()
        )));
    }
    if page == 0 {
        return Err(Error::new("Pages are numbered from 1."));
    }
    let dpi = dpi.clamp(MIN_DPI, MAX_DPI);
    if let Some(pages) = page_count(pdf).filter(|&pages| page > pages) {
        return Err(Error::new(format!(
            "Page {page} does not exist: the document has {pages} pages."
        )));
    }

    let (renderer, mut command, scratch) = if let Some(pdftoppm) = find_program("pdftoppm") {
        let mut command = Command::new(pdftoppm);
        // With no output name, pdftoppm writes the image to stdout.
        command.args(["-png", "-singlefile", "-r", &dpi.to_string()]);
        command
            .args(["-f", &page.to_string(), "-l", &page.to_string()])
            .arg(pdf);
        ("pdftoppm", command, None)
    } else if let Some(mutool) = find_program("mutool") {
        let scratch =
            std::env::temp_dir().join(format!("thelab-render-{}-{page}.png", std::process::id()));
        let mut command = Command::new(mutool);
        command
            .arg("draw")
            .arg("-o")
            .arg(&scratch)
            .args(["-r", &dpi.to_string()]);
        command.arg(pdf).arg(page.to_string());
        ("mutool", command, Some(scratch))
    } else {
        return Err(Error::new(
            "No PDF renderer found. Install Poppler (`brew install poppler`) for pdftoppm, or MuPDF for mutool.",
        ));
    };

    let mut output = run(&mut command, Duration::from_secs(60))
        .map_err(|e| Error::new(format!("Cannot run {renderer}: {e}")))?;
    let png = match &scratch {
        Some(path) => {
            let bytes = std::fs::read(path).unwrap_or_default();
            let _ = std::fs::remove_file(path);
            bytes
        }
        None => std::mem::take(&mut output.stdout),
    };

    let failed = !output.status.is_some_and(|status| status.success());
    let Some((width, height)) = png_size(&png).filter(|_| !failed) else {
        let reason = output.stderr.trim();
        let reason = if reason.is_empty() {
            "it produced no image"
        } else {
            reason
        };
        return Err(Error::new(format!(
            "{renderer} could not render page {page}: {reason}"
        )));
    };
    Ok(RenderedPage {
        png,
        page,
        dpi,
        width,
        height,
        renderer,
    })
}

/// Width and height from a PNG header, or `None` if the bytes are not a PNG.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if png.len() < 24 || png[..8] != SIGNATURE {
        return None;
    }
    let number = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Some((number(16), number(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_size_reads_the_header() {
        let mut png = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        png.extend(935u32.to_be_bytes());
        png.extend(1210u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((935, 1210)));
        assert_eq!(png_size(b"%PDF-1.5 not a png at all, really"), None);
        assert_eq!(png_size(&[]), None);
    }
}
