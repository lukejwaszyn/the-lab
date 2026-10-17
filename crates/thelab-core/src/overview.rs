//! A one-call summary of a project, for orienting a reader (or a model).

use serde::Serialize;

use crate::Result;
use crate::check::{self, Counts};
use crate::outline::{Heading, Unit, read_outline};
use crate::scan::Project;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub root: String,
    /// The file that gets compiled, relative to `root`.
    pub main: String,
    /// Every `.tex` file in the project folder.
    pub tex_files: Vec<String>,
    pub bib_files: Vec<String>,
    /// The files that make up the main document, in reading order.
    pub document_files: Vec<String>,
    pub outline: Vec<Heading>,
    /// Word counts per chapter (per section if there are no chapters).
    pub units: Vec<Unit>,
    pub total_words: u32,
    pub counts: Counts,
}

pub fn overview(project: &Project) -> Result<Overview> {
    let outline = read_outline(project)?;
    let report = check::check(project, &outline)?;
    Ok(Overview {
        root: project.root.display().to_string(),
        main: outline.main.clone(),
        tex_files: project.tex_files.clone(),
        bib_files: project.bib_files.clone(),
        document_files: outline.files.clone(),
        units: outline.units(),
        total_words: outline.total_words(),
        outline: outline.headings,
        counts: report.counts,
    })
}
