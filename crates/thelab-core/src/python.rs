//! Which Python runs which part of a project: the map behind `thelab python`,
//! the app's Run command and its status bar.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::scan::{CONFIG_FILE, Project};
use crate::tools::{find_program, is_executable, run};

/// `[python]` in `thelab.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConfig {
    /// The project's interpreter. Detected when absent.
    pub interpreter: Option<String>,
    /// Interpreters for folders, keyed by folder relative to the project
    /// root. A file uses the longest folder that contains it.
    #[serde(default)]
    pub folders: BTreeMap<String, String>,
}

/// How long `--version` may take before the interpreter counts as broken.
/// Python answers in well under a second; a cold conda environment can be
/// slower.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Virtual environments looked for in the project root, in order.
const VENV_FOLDERS: [&str; 2] = [".venv", "venv"];

const NO_PYTHON: &str = "No Python found. Install it with `brew install python`, \
                         or set `[python] interpreter` in thelab.toml.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Named in `thelab.toml`.
    Config,
    /// A virtual environment in the project root.
    Venv,
    /// `python3` from the tool search path.
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interpreter {
    /// Absolute path, when one was found.
    pub path: Option<String>,
    pub source: Source,
    /// What `thelab.toml` said, for a configured interpreter.
    pub configured: Option<String>,
    /// For example `3.14.0`.
    pub version: Option<String>,
    /// The virtual environment the interpreter belongs to: what
    /// `VIRTUAL_ENV` should be while it runs.
    pub virtual_env: Option<String>,
    /// Why it cannot be used. Absent when it can.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInterpreter {
    /// Relative to the project root, without a trailing slash.
    pub folder: String,
    #[serde(flatten)]
    pub interpreter: Interpreter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonMap {
    pub root: String,
    /// For every file outside the configured folders.
    pub default: Interpreter,
    /// In folder-name order.
    pub folders: Vec<FolderInterpreter>,
    /// Advice that is not a problem with any one interpreter.
    pub notes: Vec<String>,
}

impl PythonMap {
    pub fn has_problems(&self) -> bool {
        self.default.problem.is_some()
            || self
                .folders
                .iter()
                .any(|entry| entry.interpreter.problem.is_some())
    }
}

impl PythonMap {
    /// The interpreter that runs `file`, a path relative to the root with
    /// `/`: the one of the longest configured folder that holds it, else the
    /// default. The folder comes with it, `None` for the default. A folder
    /// whose interpreter has a problem still claims its files, so that a
    /// script never runs with a Python it was not meant for.
    pub fn interpreter_for(&self, file: &str) -> (Option<&str>, &Interpreter) {
        let file = file.trim_start_matches("./");
        self.folders
            .iter()
            .filter(|entry| {
                file.strip_prefix(entry.folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|entry| entry.folder.len())
            .map_or((None, &self.default), |entry| {
                (Some(entry.folder.as_str()), &entry.interpreter)
            })
    }
}

/// The interpreters of the project at `root`, from its `thelab.toml` alone.
/// Unlike `scan` it does not walk the project's files, so a project full of
/// data answers as quickly as an empty one.
pub fn python_map_at(root: &Path) -> crate::Result<PythonMap> {
    let root = crate::scan::project_root(root)?;
    let config = crate::scan::read_config(&root)?;
    Ok(python_map(&Project {
        root,
        main: None,
        tex_files: Vec::new(),
        bib_files: Vec::new(),
        config,
    }))
}

/// The interpreters of a project, each checked by asking it for its version.
pub fn python_map(project: &Project) -> PythonMap {
    python_map_with(project, find_program)
}

/// `python_map` with the lookup of bare program names passed in.
fn python_map_with(project: &Project, find: impl Fn(&str) -> Option<PathBuf>) -> PythonMap {
    let root = &project.root;
    let config = project.config.python.clone().unwrap_or_default();
    let mut notes = Vec::new();

    let default = match &config.interpreter {
        Some(value) => configured(value, root, &find, "interpreter"),
        None => detect(root, &find, &mut notes),
    };

    let mut folders: Vec<FolderInterpreter> = config
        .folders
        .iter()
        .map(|(key, value)| {
            let folder = key
                .trim()
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string();
            let place = format!("folders.\"{key}\"");
            let interpreter = match folder_problem(&folder) {
                Some(problem) => Interpreter {
                    path: None,
                    source: Source::Config,
                    configured: Some(value.clone()),
                    version: None,
                    virtual_env: None,
                    problem: Some(format!("{problem} ({place} in {CONFIG_FILE})")),
                },
                None => {
                    if !root.join(&folder).is_dir() {
                        notes.push(format!(
                            "The folder {folder} named in {CONFIG_FILE} does not exist yet."
                        ));
                    }
                    configured(value, root, &find, &place)
                }
            };
            FolderInterpreter {
                folder,
                interpreter,
            }
        })
        .collect();
    folders.sort_by(|a, b| a.folder.cmp(&b.folder));

    if root.join("uv.lock").is_file() && !root.join(".venv").exists() {
        notes.push("uv.lock is here but .venv is not: run `uv sync` to create it.".to_string());
    }

    PythonMap {
        root: root.display().to_string(),
        default,
        folders,
        notes,
    }
}

/// Why a `[python.folders]` key cannot name a folder in the project, if it cannot.
fn folder_problem(folder: &str) -> Option<String> {
    let path = Path::new(folder);
    if folder.is_empty() {
        return Some("An empty folder name".to_string());
    }
    let escapes = path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)));
    escapes.then(|| format!("The folder {folder} is not a path inside the project"))
}

/// The interpreter a `thelab.toml` value names, checked.
fn configured(
    value: &str,
    root: &Path,
    find: &impl Fn(&str) -> Option<PathBuf>,
    place: &str,
) -> Interpreter {
    let value = value.trim();
    let unusable = |problem: String| Interpreter {
        path: None,
        source: Source::Config,
        configured: Some(value.to_string()),
        version: None,
        virtual_env: None,
        problem: Some(format!("{problem} ({place} in {CONFIG_FILE})")),
    };

    let path = if let Some(rest) = value.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => return unusable(format!("{value} starts with ~ but HOME is not set")),
        }
    } else if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else if value.contains('/') {
        root.join(value)
    } else {
        match find(value) {
            Some(path) => path,
            None => {
                return unusable(format!("{value} is not on PATH or in the usual places"));
            }
        }
    };

    if !path.exists() {
        return unusable(format!("{} does not exist", path.display()));
    }
    if !is_executable(&path) {
        return unusable(format!("{} is not executable", path.display()));
    }
    let mut interpreter = probe(path, Source::Config);
    interpreter.configured = Some(value.to_string());
    if let Some(problem) = interpreter.problem.take() {
        interpreter.problem = Some(format!("{problem} ({place} in {CONFIG_FILE})"));
    }
    interpreter
}

/// The project's interpreter when `thelab.toml` names none.
fn detect(
    root: &Path,
    find: &impl Fn(&str) -> Option<PathBuf>,
    notes: &mut Vec<String>,
) -> Interpreter {
    for folder in VENV_FOLDERS {
        let python = root.join(folder).join("bin/python");
        if is_executable(&python) {
            return probe(python, Source::Venv);
        }
        if root.join(folder).join("pyvenv.cfg").is_file() {
            notes.push(format!(
                "{folder} has no working bin/python, so it was skipped; recreate it."
            ));
        }
    }
    match find("python3") {
        Some(python) => probe(python, Source::Path),
        None => Interpreter {
            path: None,
            source: Source::Path,
            configured: None,
            version: None,
            virtual_env: None,
            problem: Some(NO_PYTHON.to_string()),
        },
    }
}

/// Asks an interpreter for its version.
fn probe(path: PathBuf, source: Source) -> Interpreter {
    let (version, problem) = match version_of(&path) {
        Ok(version) => (Some(version), None),
        Err(problem) => (None, Some(problem)),
    };
    Interpreter {
        virtual_env: virtual_env(&path).map(|venv| venv.display().to_string()),
        path: Some(path.display().to_string()),
        source,
        configured: None,
        version,
        problem,
    }
}

fn version_of(python: &Path) -> Result<String, String> {
    let name = python.display();
    let output = run(Command::new(python).arg("--version"), VERSION_TIMEOUT)
        .map_err(|e| format!("Cannot run {name}: {e}"))?;
    let Some(status) = output.status else {
        return Err(format!(
            "{name} did not answer --version within {} s",
            VERSION_TIMEOUT.as_secs()
        ));
    };
    // Python 2 printed its version on stderr.
    let said = format!("{}\n{}", output.stdout_text(), output.stderr);
    let said = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if !status.success() {
        return Err(format!("{name} --version failed ({status}): {said}"));
    }
    said.strip_prefix("Python ")
        .map(str::to_string)
        .ok_or_else(|| format!("{name} does not look like Python: --version said \"{said}\""))
}

/// The virtual environment `python` lives in: the folder above its `bin`,
/// when that folder has a `pyvenv.cfg`. Symlinks are not followed, because a
/// venv's `python` links to the interpreter it was made from.
fn virtual_env(python: &Path) -> Option<PathBuf> {
    let bin = python.parent()?;
    let venv = bin.parent()?;
    (bin.file_name()? == "bin" && venv.join("pyvenv.cfg").is_file()).then(|| venv.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::scan::scan;

    /// A scratch project folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "thelab-python-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(&path).unwrap())
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }

        /// A stand-in interpreter: a script that prints `answer` and exits.
        fn python(&self, relative: &str, answer: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.write(relative, &format!("#!/bin/sh\necho '{answer}'\n"));
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn map(&self, find: impl Fn(&str) -> Option<PathBuf>) -> PythonMap {
            python_map_with(&scan(&self.0, None).unwrap(), find)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn nothing(_: &str) -> Option<PathBuf> {
        None
    }

    #[test]
    fn folders_and_the_default_come_from_the_config() {
        let project = Scratch::new("config");
        project.python("tools/py", "Python 3.14.0");
        let openems = project.python("envs/openems/bin/python", "Python 3.12.4");
        project.write("envs/openems/pyvenv.cfg", "home = /usr/bin\n");
        let elsewhere = Scratch::new("elsewhere");
        let ml = elsewhere.python("bin/python3", "Python 3.13.1");
        project.write("sim/run.py", "");
        project.write("src/ml/train.py", "");
        project.write(
            "thelab.toml",
            &format!(
                "[python]\ninterpreter = \"tools/py\"\n\n[python.folders]\n\
                 \"sim\" = \"envs/openems/bin/python\"\n\"src/ml\" = \"{}\"\n",
                ml.display()
            ),
        );

        let map = project.map(nothing);
        assert!(!map.has_problems(), "{map:#?}");
        assert_eq!(map.default.source, Source::Config);
        assert_eq!(map.default.version.as_deref(), Some("3.14.0"));
        assert_eq!(map.default.configured.as_deref(), Some("tools/py"));
        let folders: Vec<(&str, Option<&str>)> = map
            .folders
            .iter()
            .map(|f| (f.folder.as_str(), f.interpreter.version.as_deref()))
            .collect();
        assert_eq!(
            folders,
            [("sim", Some("3.12.4")), ("src/ml", Some("3.13.1"))]
        );
        assert_eq!(
            map.folders[0].interpreter.path,
            Some(openems.display().to_string())
        );
        assert_eq!(
            map.folders[0].interpreter.virtual_env,
            Some(project.0.join("envs/openems").display().to_string())
        );
        assert_eq!(map.folders[1].interpreter.virtual_env, None);
        assert!(map.notes.is_empty(), "{:?}", map.notes);
    }

    #[test]
    fn a_venv_in_the_project_is_used_when_nothing_is_configured() {
        let project = Scratch::new("venv");
        let python = project.python(".venv/bin/python", "Python 3.12.0");
        project.write(".venv/pyvenv.cfg", "home = /usr/bin\n");

        let map = project.map(|_| panic!("a venv was there; nothing should be looked up"));
        assert_eq!(map.default.source, Source::Venv);
        assert_eq!(map.default.path, Some(python.display().to_string()));
        assert_eq!(
            map.default.virtual_env,
            Some(project.0.join(".venv").display().to_string())
        );
    }

    #[test]
    fn without_a_venv_python3_is_looked_up() {
        let project = Scratch::new("path");
        let tools = Scratch::new("tools");
        let python3 = tools.python("python3", "Python 3.14.0");

        let map = project.map(|name| (name == "python3").then(|| python3.clone()));
        assert_eq!(map.default.source, Source::Path);
        assert_eq!(map.default.version.as_deref(), Some("3.14.0"));
        assert_eq!(map.default.problem, None);
    }

    #[test]
    fn no_python_at_all_says_how_to_get_one() {
        let project = Scratch::new("none");
        let map = project.map(nothing);
        assert!(map.has_problems());
        assert_eq!(map.default.problem.as_deref(), Some(NO_PYTHON));
    }

    #[test]
    fn a_missing_interpreter_names_its_path_and_where_it_was_set() {
        let project = Scratch::new("missing");
        project.write(
            "thelab.toml",
            "[python.folders]\n\"sim\" = \"~/thelab-no-such-folder/bin/python\"\n",
        );
        project.write("sim/run.py", "");

        let map = project.map(nothing);
        let problem = map.folders[0].interpreter.problem.clone().unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            problem,
            format!(
                "{home}/thelab-no-such-folder/bin/python does not exist \
                 (folders.\"sim\" in thelab.toml)"
            )
        );
    }

    #[test]
    fn a_program_that_is_not_python_is_reported() {
        let project = Scratch::new("impostor");
        project.python("bin/py", "hello");
        project.write("thelab.toml", "[python]\ninterpreter = \"bin/py\"\n");

        let problem = project.map(nothing).default.problem.unwrap();
        assert!(problem.contains("does not look like Python"), "{problem}");
        assert!(
            problem.ends_with("(interpreter in thelab.toml)"),
            "{problem}"
        );
    }

    #[test]
    fn a_version_printed_on_stderr_is_read() {
        let project = Scratch::new("stderr");
        let python = project.write("bin/py", "#!/bin/sh\necho 'Python 2.7.18' >&2\n");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
        }
        project.write("thelab.toml", "[python]\ninterpreter = \"bin/py\"\n");
        assert_eq!(
            project.map(nothing).default.version.as_deref(),
            Some("2.7.18")
        );
    }

    #[test]
    fn folder_keys_are_tidied_and_must_stay_inside_the_project() {
        let project = Scratch::new("keys");
        project.python("bin/py", "Python 3.14.0");
        project.write("sim/run.py", "");
        project.write(
            "thelab.toml",
            "[python.folders]\n\"./sim/\" = \"bin/py\"\n\"../outside\" = \"bin/py\"\n",
        );

        let map = project.map(nothing);
        let entries: Vec<(&str, bool)> = map
            .folders
            .iter()
            .map(|f| (f.folder.as_str(), f.interpreter.problem.is_some()))
            .collect();
        assert_eq!(entries, [("../outside", true), ("sim", false)]);
    }

    #[test]
    fn notes_cover_a_uv_lock_without_its_venv_and_a_folder_not_made_yet() {
        let project = Scratch::new("notes");
        project.python("bin/py", "Python 3.14.0");
        project.write("uv.lock", "");
        project.write("thelab.toml", "[python.folders]\n\"later\" = \"bin/py\"\n");

        let notes = project.map(nothing).notes;
        assert_eq!(
            notes,
            [
                "The folder later named in thelab.toml does not exist yet.",
                "uv.lock is here but .venv is not: run `uv sync` to create it.",
            ]
        );
    }

    #[test]
    fn an_unknown_key_under_python_is_an_error() {
        let project = Scratch::new("typo");
        project.write("thelab.toml", "[python]\ninterpreters = \"python3\"\n");
        let error = scan(&project.0, None).unwrap_err().to_string();
        assert!(error.contains("unknown field `interpreters`"), "{error}");
    }

    #[test]
    fn a_file_uses_the_longest_folder_that_holds_it() {
        let project = Scratch::new("for-file");
        project.python("bin/py", "Python 3.14.0");
        project.python("bin/sim", "Python 3.12.4");
        project.python("bin/ml", "Python 3.13.1");
        project.write("src/ml/train.py", "");
        project.write("sim/run.py", "");
        project.write(
            "thelab.toml",
            "[python]\ninterpreter = \"bin/py\"\n\n[python.folders]\n\
             \"sim\" = \"bin/sim\"\n\"src/ml\" = \"bin/ml\"\n\"src\" = \"bin/py\"\n",
        );
        let map = project.map(nothing);

        let version = |file: &str| {
            let (folder, interpreter) = map.interpreter_for(file);
            (folder, interpreter.version.as_deref())
        };
        assert_eq!(version("src/ml/train.py"), (Some("src/ml"), Some("3.13.1")));
        assert_eq!(
            version("src/ml/deep/net.py"),
            (Some("src/ml"), Some("3.13.1"))
        );
        assert_eq!(version("src/processing.py"), (Some("src"), Some("3.14.0")));
        assert_eq!(version("sim/run.py"), (Some("sim"), Some("3.12.4")));
        assert_eq!(version("./sim/run.py"), (Some("sim"), Some("3.12.4")));
        // A folder name is not a prefix of a longer one, and the root is the default.
        assert_eq!(version("simulation/run.py"), (None, Some("3.14.0")));
        assert_eq!(version("src_old/x.py"), (None, Some("3.14.0")));
        assert_eq!(version("tools/build.py"), (None, Some("3.14.0")));
        assert_eq!(version("run.py"), (None, Some("3.14.0")));
    }

    #[test]
    fn a_folder_whose_interpreter_has_a_problem_still_claims_its_files() {
        let project = Scratch::new("for-file-problem");
        project.python("bin/py", "Python 3.14.0");
        project.write(
            "thelab.toml",
            "[python]\ninterpreter = \"bin/py\"\n\n[python.folders]\n\
             \"sim\" = \"bin/missing\"\n",
        );
        let map = project.map(nothing);

        // Falling back to the default would run the script with the wrong Python.
        let (folder, interpreter) = map.interpreter_for("sim/run.py");
        assert_eq!(folder, Some("sim"));
        assert!(interpreter.problem.is_some());
    }

    #[test]
    fn the_map_can_be_read_without_scanning_the_project() {
        let project = Scratch::new("at-root");
        project.python("bin/py", "Python 3.14.0");
        project.write("thelab.toml", "[python]\ninterpreter = \"bin/py\"\n");

        let map = python_map_at(&project.0).unwrap();
        assert_eq!(map.root, project.0.display().to_string());
        assert_eq!(map.default.version.as_deref(), Some("3.14.0"));
        assert_eq!(map, project.map(nothing));
    }

    #[test]
    fn reading_the_map_reports_a_broken_config_and_a_missing_folder() {
        let project = Scratch::new("at-root-bad");
        project.write("thelab.toml", "[python]\ninterpreters = \"python3\"\n");
        let error = python_map_at(&project.0).unwrap_err().to_string();
        assert!(error.contains("unknown field `interpreters`"), "{error}");

        let gone = project.0.join("no-such-folder");
        let error = python_map_at(&gone).unwrap_err().to_string();
        assert!(error.contains("Cannot open project folder"), "{error}");
    }
}
