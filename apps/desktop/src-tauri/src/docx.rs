//! The Lab's Word tab: the state of a `.docx` on disk (its modification
//! time, and whether Word has it open), the save that replaces it in one
//! step, and the account's full name for tracked changes. The editing is
//! SuperDoc's, in the webview; this side only reads and writes files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tauri::ipc::{InvokeBody, Request};
use tauri_plugin_fs::FsExt;

/// What the tab needs to know about its file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocxFileState {
    pub exists: bool,
    /// Milliseconds since the Unix epoch; absent when it cannot be read.
    pub modified_ms: Option<u64>,
    pub size: u64,
    /// Word's owner file ("~$...") is beside the document.
    pub open_in_word: bool,
    /// The file cannot be written, so the tab only shows it.
    pub read_only: bool,
}

/// The names Word gives the owner file of `name` while it has it open: "~$"
/// in place of the name's first two characters, or in front of a short
/// name. Word's rule for how many characters it replaces depends on the
/// name's length and is not documented, so all three forms count.
pub fn word_owner_file_names(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let stem_len = name
        .rfind('.')
        .map(|i| name[..i].chars().count())
        .unwrap_or(chars.len());
    let mut names = Vec::new();
    for drop in 0..=2usize {
        // Keep at least one character of the stem before the extension.
        if drop > 0 && stem_len <= drop {
            continue;
        }
        let rest: String = chars[drop..].iter().collect();
        let candidate = format!("~${rest}");
        if !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    names
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

pub fn file_state(path: &Path) -> DocxFileState {
    let open_in_word = match (path.parent(), path.file_name().and_then(|n| n.to_str())) {
        (Some(dir), Some(name)) => word_owner_file_names(name)
            .iter()
            .any(|owner| dir.join(owner).is_file()),
        _ => false,
    };
    match fs::metadata(path) {
        Ok(metadata) => DocxFileState {
            exists: true,
            modified_ms: modified_ms(&metadata),
            size: metadata.len(),
            open_in_word,
            read_only: metadata.permissions().readonly(),
        },
        Err(_) => DocxFileState {
            exists: false,
            modified_ms: None,
            size: 0,
            open_in_word,
            read_only: false,
        },
    }
}

/// The suffix of the temporary file a save writes beside the document. The
/// file tree hides it (`fs.ts`).
pub const SAVE_SUFFIX: &str = ".thelab-save";

/// Replaces the document at `path` with `bytes` in one step: the bytes go
/// to a temporary file in the same folder, which is then renamed over the
/// document, so a failed or interrupted save leaves the old file whole.
/// The new file keeps the old one's permissions. A symlink is followed and
/// its target replaced. Only a `.docx` that can be written is replaced; one
/// deleted from disk while its tab was open is written again, if its folder
/// is still there.
pub fn save_atomically(path: &Path, bytes: &[u8]) -> Result<DocxFileState, String> {
    let is_docx = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"));
    if !is_docx {
        return Err(format!("{} is not a .docx file.", path.display()));
    }
    let (target, permissions): (PathBuf, Option<fs::Permissions>) = match fs::canonicalize(path) {
        Ok(target) => {
            let metadata = fs::metadata(&target).map_err(|e| e.to_string())?;
            if !metadata.is_file() {
                return Err(format!("{} is not a file.", path.display()));
            }
            if metadata.permissions().readonly() {
                return Err(format!(
                    "{} is read-only, so it was not saved.",
                    path.display()
                ));
            }
            (target, Some(metadata.permissions()))
        }
        // Gone from disk (but not a broken symlink): write it again.
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && fs::symlink_metadata(path).is_err()
                && path.parent().is_some_and(Path::is_dir) =>
        {
            (path.to_path_buf(), None)
        }
        Err(e) => return Err(format!("{} cannot be saved: {e}", path.display())),
    };
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no folder.", path.display()))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no name.", path.display()))?;
    let temp = dir.join(format!(
        ".{name}.{}{SAVE_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));

    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(permissions) = &permissions {
            fs::set_permissions(&temp, permissions.clone())?;
        }
        fs::rename(&temp, &target)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("{} could not be saved: {e}", path.display()));
    }
    // Make the rename itself durable (best effort; not all systems allow it).
    if let Ok(folder) = fs::File::open(dir) {
        let _ = folder.sync_all();
    }
    Ok(file_state(path))
}

/// Decodes the percent-encoding of `encodeURIComponent`.
pub fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| "the path is not percent-encoded correctly".to_string())?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "the path is not valid UTF-8".to_string())
}

/// The first comma-separated field of a GECOS entry, which holds the name.
pub fn name_from_gecos(gecos: &str) -> Option<String> {
    let name = gecos.split(',').next().unwrap_or("").trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn account_full_name_now() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        // `id -F` prints the account's full name (its RealName).
        let output = std::process::Command::new("/usr/bin/id")
            .arg("-F")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        name_from_gecos(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let user = std::env::var("USER").ok()?;
        let output = std::process::Command::new("getent")
            .args(["passwd", &user])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&output.stdout);
        name_from_gecos(line.trim().split(':').nth(4)?)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The account's full name, for the author of tracked changes. None when
/// it cannot be read; the Settings field then starts empty.
#[tauri::command]
pub async fn account_full_name() -> Option<String> {
    tauri::async_runtime::spawn_blocking(account_full_name_now)
        .await
        .ok()
        .flatten()
}

#[tauri::command]
pub async fn docx_file_state(path: String) -> DocxFileState {
    tauri::async_runtime::spawn_blocking(move || file_state(Path::new(&path)))
        .await
        .unwrap_or(DocxFileState {
            exists: false,
            modified_ms: None,
            size: 0,
            open_in_word: false,
            read_only: false,
        })
}

/// Saves a Word tab. The body is the file's bytes and the `path` header
/// its percent-encoded path, which must be in a folder the app opened as a
/// project.
#[tauri::command]
pub async fn docx_save(
    app: tauri::AppHandle,
    request: Request<'_>,
) -> Result<DocxFileState, String> {
    let path = request
        .headers()
        .get("path")
        .and_then(|p| p.to_str().ok())
        .ok_or_else(|| "The save named no file.".to_string())
        .and_then(percent_decode)?;
    let bytes: Vec<u8> = match request.body() {
        InvokeBody::Raw(data) => data.clone(),
        InvokeBody::Json(serde_json::Value::Array(data)) => data
            .iter()
            .filter_map(|v| v.as_u64().map(|v| v as u8))
            .collect(),
        _ => return Err("The save carried no bytes.".to_string()),
    };
    if !app.fs_scope().is_allowed(&path) {
        return Err(format!("{path} is not in an open project."));
    }
    tauri::async_runtime::spawn_blocking(move || save_atomically(Path::new(&path), &bytes))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_file_names_cover_every_form_word_uses() {
        assert_eq!(
            word_owner_file_names("Document1.docx"),
            vec!["~$Document1.docx", "~$ocument1.docx", "~$cument1.docx"]
        );
        // A short stem keeps at least one of its characters.
        assert_eq!(
            word_owner_file_names("ab.docx"),
            vec!["~$ab.docx", "~$b.docx"]
        );
        assert_eq!(word_owner_file_names("a.docx"), vec!["~$a.docx"]);
        // Characters, not bytes, are dropped.
        assert_eq!(
            word_owner_file_names("Ünïcode name.docx"),
            vec![
                "~$Ünïcode name.docx",
                "~$nïcode name.docx",
                "~$ïcode name.docx"
            ]
        );
    }

    #[test]
    fn a_file_is_open_in_word_when_any_owner_file_is_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("Thesis draft.docx");
        fs::write(&doc, b"PK").unwrap();
        assert!(!file_state(&doc).open_in_word);
        fs::write(dir.path().join("~$esis draft.docx"), b"owner").unwrap();
        let state = file_state(&doc);
        assert!(state.open_in_word);
        assert!(state.exists);
        assert_eq!(state.size, 2);
        assert!(state.modified_ms.is_some());
    }

    #[test]
    fn a_missing_file_has_no_time() {
        let dir = tempfile::tempdir().unwrap();
        let state = file_state(&dir.path().join("gone.docx"));
        assert!(!state.exists);
        assert_eq!(state.modified_ms, None);
    }

    #[test]
    fn a_save_replaces_the_file_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("report.docx");
        fs::write(&doc, b"old bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&doc, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let state = save_atomically(&doc, b"new bytes").unwrap();
        assert_eq!(fs::read(&doc).unwrap(), b"new bytes");
        assert_eq!(state.size, 9);
        assert_eq!(state, file_state(&doc));
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["report.docx"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&doc).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_save_through_a_symlink_replaces_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.docx");
        let link = dir.path().join("link.docx");
        fs::write(&real, b"old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        save_atomically(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }

    #[test]
    fn only_a_writable_docx_in_an_existing_folder_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        fs::write(&text, b"keep").unwrap();
        assert!(save_atomically(&text, b"x").is_err());
        assert_eq!(fs::read(&text).unwrap(), b"keep");
        assert!(save_atomically(&dir.path().join("no folder/missing.docx"), b"x").is_err());

        let locked = dir.path().join("locked.docx");
        fs::write(&locked, b"keep").unwrap();
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&locked, permissions).unwrap();
        assert!(file_state(&locked).read_only);
        assert!(save_atomically(&locked, b"x").is_err());
        assert_eq!(fs::read(&locked).unwrap(), b"keep");
        let names = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(names, 2, "a refused save leaves no temporary file");
    }

    #[test]
    fn a_file_deleted_while_open_is_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("gone.docx");
        let state = save_atomically(&doc, b"edits").unwrap();
        assert!(state.exists);
        assert!(!state.read_only);
        assert_eq!(fs::read(&doc).unwrap(), b"edits");
    }

    #[test]
    fn paths_are_decoded_as_encode_uri_component_wrote_them() {
        assert_eq!(
            percent_decode("%2FUsers%2Fa%20b%2FTh%C3%A8se%20(1).docx").unwrap(),
            "/Users/a b/Thèse (1).docx"
        );
        assert!(percent_decode("%zz").is_err());
        assert!(percent_decode("%2").is_err());
    }

    #[test]
    fn the_name_is_the_first_gecos_field() {
        assert_eq!(
            name_from_gecos("Ada Lovelace\n").as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(
            name_from_gecos("Ada Lovelace,Room 1,555").as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(name_from_gecos("  \n"), None);
        assert_eq!(name_from_gecos(""), None);
    }

    #[test]
    fn reading_the_account_name_never_fails() {
        // Whatever the account, the answer is a trimmed name or nothing.
        if let Some(name) = account_full_name_now() {
            assert_eq!(name, name.trim());
            assert!(!name.is_empty());
        }
    }
}
