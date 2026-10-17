//! File-based IPC with The Lab.
//!
//! The app writes `editor-state.json` whenever the cursor or selection
//! moves, and watches `reveal-request.json` to learn where it should scroll.
//! Both live in one directory; see docs/ARCHITECTURE.md for the formats.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

/// Overrides the state directory, mainly for tests.
pub const STATE_DIR_VARIABLE: &str = "THELAB_STATE_DIR";
pub const EDITOR_STATE_FILE: &str = "editor-state.json";
pub const REVEAL_REQUEST_FILE: &str = "reveal-request.json";

/// The directory shared with the app.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(STATE_DIR_VARIABLE).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/The Lab")
    } else {
        match std::env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir).join("thelab"),
            None => home.join(".local/state/thelab"),
        }
    }
}

/// What the app last reported about the editor. Every field is optional so
/// that a newer or older app does not break the server.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub project_root: Option<String>,
    /// The open file. Absolute, or relative to `project_root`.
    pub file: Option<String>,
    /// The file's language mode, such as `latex` or `python`.
    pub language: Option<String>,
    /// The id of the notebook cell the cursor is in, for `.ipynb` files.
    pub cell_id: Option<String>,
    /// 1-based cursor line.
    pub line: Option<u32>,
    /// 1-based cursor column.
    pub column: Option<u32>,
    /// `null`, the selected text, or `{text, startLine, startColumn, endLine, endColumn}`.
    pub selection: Option<Value>,
    /// ISO 8601 time of the last change.
    pub updated_at: Option<String>,
}

impl EditorState {
    /// The selected text, if there is a selection.
    pub fn selection_text(&self) -> Option<&str> {
        let text = match self.selection.as_ref()? {
            Value::String(text) => text.as_str(),
            Value::Object(fields) => fields.get("text")?.as_str()?,
            _ => return None,
        };
        (!text.is_empty()).then_some(text)
    }
}

/// Reads the editor state. `Ok(None)` means the file is not there, which is
/// how a server can tell that the app is not running.
pub fn read_editor_state() -> Result<Option<EditorState>> {
    let path = state_dir().join(EDITOR_STATE_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::new(format!("Cannot read {}: {e}", path.display()))),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| Error::new(format!("Cannot understand {}: {e}", path.display())))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealRequest {
    /// Absolute path of the file to show.
    pub file: String,
    /// 1-based line to scroll to.
    pub line: u32,
    /// ISO 8601 time of the request, so that asking twice for the same place
    /// still changes the file.
    pub requested_at: String,
}

/// Asks the app to open `file` (absolute) at `line`. Returns the request
/// file's path. The file is replaced atomically so the app never reads half
/// of it.
pub fn write_reveal_request(file: &Path, line: u32) -> Result<PathBuf> {
    let dir = state_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| Error::new(format!("Cannot create {}: {e}", dir.display())))?;
    let request = RevealRequest {
        file: file.display().to_string(),
        line,
        requested_at: iso8601(SystemTime::now()),
    };
    let json = serde_json::to_string_pretty(&request).expect("a reveal request is valid JSON");

    let path = dir.join(REVEAL_REQUEST_FILE);
    let scratch = dir.join(format!(".{REVEAL_REQUEST_FILE}.{}", std::process::id()));
    fs::write(&scratch, json)
        .and_then(|()| fs::rename(&scratch, &path))
        .map_err(|e| Error::new(format!("Cannot write {}: {e}", path.display())))?;
    Ok(path)
}

/// Formats a time as `2026-10-11T18:03:07Z`.
pub fn iso8601(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn iso8601_matches_known_dates() {
        let at = |seconds: u64| iso8601(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_791_741_787), "2026-10-11T18:03:07Z");
        assert_eq!(at(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn selection_can_be_a_string_or_an_object() {
        let state = |json: &str| serde_json::from_str::<EditorState>(json).unwrap();
        assert_eq!(
            state(r#"{"selection": "some text"}"#).selection_text(),
            Some("some text")
        );
        assert_eq!(
            state(r#"{"selection": {"text": "picked", "startLine": 3, "endLine": 4}}"#)
                .selection_text(),
            Some("picked")
        );
        assert_eq!(state(r#"{"selection": null}"#).selection_text(), None);
        assert_eq!(state(r#"{"selection": ""}"#).selection_text(), None);
        assert_eq!(
            state(r#"{"file": "main.tex", "line": 12, "futureField": true}"#).line,
            Some(12)
        );
    }
}
