//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub fn example_thesis() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/thesis")
}

pub fn fixture_project(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projects")
        .join(name)
}

/// A scratch directory that is removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "thelab-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes a file, creating its parent directories.
    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("create parent dir");
        fs::write(path, contents).expect("write file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Copies a project into a scratch directory, leaving out any build output,
/// so that compiling it does not touch the repository.
pub fn copy_project(source: &Path, tag: &str) -> TempDir {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("create dir");
        for entry in fs::read_dir(from).expect("read dir").flatten() {
            let name = entry.file_name();
            if name == "build" {
                continue;
            }
            if entry.file_type().expect("file type").is_dir() {
                copy(&entry.path(), &to.join(&name));
            } else {
                fs::copy(entry.path(), to.join(&name)).expect("copy file");
            }
        }
    }
    let dir = TempDir::new(tag);
    copy(source, dir.path());
    dir
}

/// Whether an external program is available. Tests that need one skip when
/// it is missing, unless THELAB_REQUIRE_TOOLS (a comma-separated list, set
/// in CI) names it, in which case they fail.
pub fn have(tool: &str) -> bool {
    if thelab_core::tools::find_program(tool).is_some() {
        return true;
    }
    let required = std::env::var("THELAB_REQUIRE_TOOLS").unwrap_or_default();
    assert!(
        !required.split(',').any(|name| name.trim() == tool),
        "{tool} is required by THELAB_REQUIRE_TOOLS but is not installed"
    );
    eprintln!("skipping: {tool} is not installed");
    false
}
