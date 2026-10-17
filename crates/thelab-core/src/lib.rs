//! Core library for The Lab: everything the CLI, the MCP server and the app
//! need to know about a LaTeX project, with no UI and no protocol code.
//!
//! From github.com/lukejwaszyn/the-lab-native (MIT, Luke Waszyn).

pub mod bib;
pub mod check;
pub mod compile;
pub mod logparse;
pub mod outline;
pub mod overview;
pub mod python;
pub mod render;
pub mod scan;
pub mod state;
pub mod synctex;
pub mod tex;
pub mod tools;

use std::fmt;

/// An error with a message meant to be shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
