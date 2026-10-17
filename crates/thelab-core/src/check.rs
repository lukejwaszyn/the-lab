//! Static checks of cross-references and citations. Nothing is compiled, so
//! these are fast, but they only know the commands listed in `outline`:
//! labels created by unusual packages can show up as false alarms.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::Result;
use crate::bib::{self, BibEntry};
use crate::outline::{Occurrence, Outline};
use crate::scan::Project;

/// A label defined in more than one place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Duplicate {
    pub name: String,
    pub locations: Vec<Location>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub labels: usize,
    pub references: usize,
    /// Distinct keys cited.
    pub citations: usize,
    pub bib_entries: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    /// `\ref` and relatives naming a label that is never defined.
    pub undefined_references: Vec<Occurrence>,
    /// Citations of keys that are in no `.bib` file.
    pub undefined_citations: Vec<Occurrence>,
    pub duplicate_labels: Vec<Duplicate>,
    /// `\input` and `\include` of files that do not exist.
    pub missing_inputs: Vec<Occurrence>,
    /// `.bib` files the document names but that do not exist.
    pub missing_bib_files: Vec<String>,
    /// Labels nothing refers to. Often intentional.
    pub unused_labels: Vec<Occurrence>,
    /// Bibliography entries the document never cites. Often intentional.
    pub uncited_entries: Vec<Occurrence>,
    pub counts: Counts,
}

impl CheckReport {
    /// Whether there is anything that would break the document. Unused
    /// labels and uncited entries do not count.
    pub fn has_problems(&self) -> bool {
        !(self.undefined_references.is_empty()
            && self.undefined_citations.is_empty()
            && self.duplicate_labels.is_empty()
            && self.missing_inputs.is_empty()
            && self.missing_bib_files.is_empty())
    }
}

/// The `.bib` files a document uses: the ones it names, or every `.bib`
/// file in the project if it names none.
pub fn bib_files(project: &Project, outline: &Outline) -> Vec<String> {
    if outline.bib_resources.is_empty() {
        project.bib_files.clone()
    } else {
        outline.bib_resources.clone()
    }
}

/// Loads the bibliography entries available to a document.
pub fn bibliography(project: &Project, outline: &Outline) -> Result<Vec<BibEntry>> {
    let files: Vec<String> = bib_files(project, outline)
        .into_iter()
        .filter(|file| project.root.join(file).is_file())
        .collect();
    bib::load(&project.root, &files)
}

pub fn check(project: &Project, outline: &Outline) -> Result<CheckReport> {
    let entries = bibliography(project, outline)?;
    let missing_bib_files = outline
        .bib_resources
        .iter()
        .filter(|file| !project.root.join(file).is_file())
        .cloned()
        .collect();

    let mut definitions: BTreeMap<&str, Vec<&Occurrence>> = BTreeMap::new();
    for label in &outline.labels {
        definitions.entry(&label.name).or_default().push(label);
    }
    let referenced = |name: &str| outline.references.iter().any(|r| r.name == name);
    let cited = |key: &str| outline.citations.iter().any(|c| c.name == key);
    let known_key = |key: &str| {
        entries.iter().any(|e| e.key == key) || outline.bib_items.iter().any(|b| b.name == key)
    };

    let duplicate_labels = definitions
        .iter()
        .filter(|(_, places)| places.len() > 1)
        .map(|(name, places)| Duplicate {
            name: name.to_string(),
            locations: places
                .iter()
                .map(|p| Location {
                    file: p.file.clone(),
                    line: p.line,
                })
                .collect(),
        })
        .collect();

    let mut cited_keys: Vec<&str> = outline.citations.iter().map(|c| c.name.as_str()).collect();
    cited_keys.sort_unstable();
    cited_keys.dedup();

    Ok(CheckReport {
        undefined_references: outline
            .references
            .iter()
            .filter(|r| !definitions.contains_key(r.name.as_str()))
            .cloned()
            .collect(),
        undefined_citations: outline
            .citations
            .iter()
            .filter(|c| !known_key(&c.name))
            .cloned()
            .collect(),
        duplicate_labels,
        missing_inputs: outline.missing_inputs.clone(),
        missing_bib_files,
        unused_labels: outline
            .labels
            .iter()
            .filter(|label| !referenced(&label.name))
            .cloned()
            .collect(),
        uncited_entries: if outline.cites_everything {
            Vec::new()
        } else {
            entries
                .iter()
                .filter(|entry| !cited(&entry.key))
                .map(|entry| Occurrence {
                    name: entry.key.clone(),
                    file: entry.file.clone(),
                    line: entry.line,
                })
                .collect()
        },
        counts: Counts {
            labels: outline.labels.len(),
            references: outline.references.len(),
            citations: cited_keys.len(),
            bib_entries: entries.len(),
        },
    })
}
