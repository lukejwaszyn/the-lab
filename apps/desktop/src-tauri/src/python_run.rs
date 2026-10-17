//! The Lab's Python commands: which interpreter runs which folder (for the
//! interpreter label), and Run and Stop for the open `.py` file. The work is
//! thelab_core's (`python` and `run`); this is the Tauri side. One script
//! runs at a time per window, and it ends with its project or its window.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, State, WebviewWindow};
use thelab_core::python::{self, Interpreter, PythonMap};
use thelab_core::run::{OutputLine, RunEnd, RunSink, RunStarted, Runs, STOP_GRACE};

/// How long quitting waits for scripts to end: the grace period after
/// SIGINT, the kill, and a little time for the kill to land.
pub const QUIT_WAIT: Duration = Duration::from_secs(STOP_GRACE.as_secs() + 2);

/// The interpreters of the project at `project_root`, each asked for its
/// version. The app asks when a project opens and when thelab.toml changes.
#[tauri::command]
pub async fn python_interpreters(project_root: String) -> Result<PythonMap, String> {
    blocking(move || python::python_map_at(Path::new(&project_root)).map_err(|e| e.to_string()))
        .await
}

/// The interpreter for one file, relative to the project root: that of the
/// longest folder in `[python.folders]` that holds it, else the default.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInterpreter {
    /// The folder that chose it; absent for the project's default.
    pub folder: Option<String>,
    #[serde(flatten)]
    pub interpreter: Interpreter,
}

#[tauri::command]
pub async fn python_interpreter_for(
    project_root: String,
    file: String,
) -> Result<FileInterpreter, String> {
    blocking(move || {
        let map = python::python_map_at(Path::new(&project_root)).map_err(|e| e.to_string())?;
        Ok(interpreter_for(&map, &file))
    })
    .await
}

fn interpreter_for(map: &PythonMap, file: &str) -> FileInterpreter {
    let (folder, interpreter) = map.interpreter_for(file);
    FileInterpreter {
        folder: folder.map(str::to_string),
        interpreter: interpreter.clone(),
    }
}

/// Runs `file` (relative to the project root) for this window. The
/// frontend picks `run_id` so that it knows the run's output before this
/// returns. Refused, with the reason, while the window's script runs, for a
/// file that is not Python, or when its interpreter has a problem.
#[tauri::command]
pub async fn python_run_start(
    window: WebviewWindow,
    runs: State<'_, Runs>,
    run_id: String,
    project_root: String,
    file: String,
) -> Result<RunStarted, String> {
    let runs = runs.inner().clone();
    let owner = window.label().to_string();
    let sink = Arc::new(WindowSink { window });
    blocking(move || runs.start(&owner, &run_id, Path::new(&project_root), &file, sink)).await
}

/// Stops this window's script: SIGINT, then a kill after 3 seconds.
#[tauri::command]
pub fn python_run_stop(window: WebviewWindow, runs: State<'_, Runs>) -> bool {
    runs.stop(window.label())
}

/// The window's project is closing or switching: its script is stopped and
/// the window can start another at once.
#[tauri::command]
pub fn python_run_close_project(window: WebviewWindow, runs: State<'_, Runs>) -> bool {
    runs.end(window.label()).is_some()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// Sends a run's output and its end to the window that started it.
struct WindowSink {
    window: WebviewWindow,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputEvent {
    run_id: String,
    lines: Vec<OutputLine>,
    dropped: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FinishedEvent {
    run_id: String,
    end: RunEnd,
}

impl RunSink for WindowSink {
    fn output(&self, run_id: &str, lines: Vec<OutputLine>, dropped: usize) {
        let event = OutputEvent {
            run_id: run_id.to_string(),
            lines,
            dropped,
        };
        // A window closed meanwhile cannot be told; its run is ending anyway.
        let _ = self
            .window
            .emit_to(self.window.label(), "python-run-output", event);
    }

    fn finished(&self, run_id: &str, end: RunEnd) {
        let event = FinishedEvent {
            run_id: run_id.to_string(),
            end,
        };
        let _ = self
            .window
            .emit_to(self.window.label(), "python-run-finished", event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_gets_the_interpreter_of_its_folder_and_says_which() {
        let project = tempfile::tempdir().unwrap();
        let python = project.path().join("bin/python");
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(
            &python,
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'Python 3.12.4'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(
            project.path().join("thelab.toml"),
            "[python.folders]\n\"sim\" = \"bin/python\"\n",
        )
        .unwrap();
        let map = python::python_map_at(project.path()).unwrap();

        let sim = serde_json::to_value(interpreter_for(&map, "sim/antenna.py")).unwrap();
        assert_eq!(sim["folder"], "sim");
        assert_eq!(sim["version"], "3.12.4");
        assert_eq!(sim["source"], "config");
        assert_eq!(sim["configured"], "bin/python");
        assert!(sim["problem"].is_null());

        let other = serde_json::to_value(interpreter_for(&map, "tools/build.py")).unwrap();
        assert!(other["folder"].is_null());
        assert_ne!(other["source"], "config");
    }

    /// Prints the interpreter map of a real project and the interpreter of
    /// each file named, as the app sees them: `THELAB_PYTHON_PROJECT=<folder>
    /// THELAB_PYTHON_FILES="sim/a.py tools/b.py" cargo test
    /// python_map_of_a_named_project -- --ignored --nocapture`. It only reads.
    #[test]
    #[ignore]
    fn python_map_of_a_named_project() {
        let Some(project) = std::env::var_os("THELAB_PYTHON_PROJECT") else {
            panic!("set THELAB_PYTHON_PROJECT to a project folder");
        };
        let map = python::python_map_at(Path::new(&project)).unwrap();
        println!("{}", serde_json::to_string_pretty(&map).unwrap());
        let files = std::env::var("THELAB_PYTHON_FILES").unwrap_or_default();
        for file in files.split_whitespace() {
            let chosen = interpreter_for(&map, file);
            println!(
                "{file}: {} (from {}){}",
                chosen.interpreter.path.as_deref().unwrap_or("none"),
                chosen.folder.as_deref().unwrap_or("the default"),
                chosen
                    .interpreter
                    .problem
                    .map(|p| format!(" PROBLEM: {p}"))
                    .unwrap_or_default()
            );
        }
    }
}
