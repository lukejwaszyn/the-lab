//! Locating and running the external programs The Lab relies on (tectonic,
//! latexmk, pdftoppm, mutool). None of them is linked; all are optional.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// If set, external programs are looked up only in these directories
/// (separated like `PATH`).
pub const TOOLS_PATH_VARIABLE: &str = "THELAB_TOOLS_PATH";

/// Where Homebrew and MacTeX install things. Apps launched from the Finder do
/// not inherit the shell's `PATH`, so these are always searched.
const USUAL_DIRECTORIES: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/Library/TeX/texbin"];

/// The same, under the home folder: uv's and Claude Code's installers use
/// `~/.local/bin`, and rustup uses `~/.cargo/bin`. The list comes from
/// ClaudePrism's `find_uv_binary` (MIT, github.com/delibae/claude-prism).
const HOME_DIRECTORIES: [&str; 2] = [".local/bin", ".cargo/bin"];

/// The directories searched for external programs, in order.
pub fn search_directories() -> Vec<PathBuf> {
    directories_from(
        std::env::var_os(TOOLS_PATH_VARIABLE),
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// `search_directories` without reading the environment: the override, the
/// shell's `PATH`, then the usual places, then the ones under `home`.
fn directories_from(
    only: Option<OsString>,
    path: Option<OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(only) = only {
        return std::env::split_paths(&only).collect();
    }
    let mut directories: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let usual = USUAL_DIRECTORIES.iter().map(PathBuf::from);
    let under_home = home.iter().flat_map(|home| {
        HOME_DIRECTORIES
            .iter()
            .map(move |directory| home.join(directory))
    });
    for directory in usual.chain(under_home) {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

/// Full path of an installed program, or `None`.
pub fn find_program(name: &str) -> Option<PathBuf> {
    search_directories()
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|path| is_executable(path))
}

pub(crate) fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// A `PATH` for child processes, so that latexmk can find pdflatex and
/// bibtex even when The Lab itself was started with a minimal environment.
pub fn child_path() -> OsString {
    std::env::join_paths(search_directories()).unwrap_or_default()
}

pub struct Output {
    /// `None` if the program was killed for running too long.
    pub status: Option<ExitStatus>,
    /// Raw, because a renderer's stdout is an image.
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Runs a program to completion, capturing its output.
///
/// The child gets no stdin: the MCP server's stdin carries protocol
/// messages, and nothing else may read from it.
pub fn run(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env("PATH", child_path());
    #[cfg(unix)]
    {
        // Its own process group, so a timeout can stop latexmk's children too.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // Drain both pipes on their own threads so that a chatty child never
    // blocks on a full pipe.
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Ok(Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
    })
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_shell_path_comes_first_then_the_usual_places_then_the_home_ones() {
        let found = directories_from(None, Some("/a:/b".into()), Some("/Users/me".into()));
        assert_eq!(
            found,
            dirs(&[
                "/a",
                "/b",
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/Library/TeX/texbin",
                "/Users/me/.local/bin",
                "/Users/me/.cargo/bin",
            ])
        );
    }

    #[test]
    fn a_directory_already_on_the_path_is_not_listed_twice() {
        let found = directories_from(
            None,
            Some("/Users/me/.local/bin:/opt/homebrew/bin".into()),
            Some("/Users/me".into()),
        );
        assert_eq!(
            found,
            dirs(&[
                "/Users/me/.local/bin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/Library/TeX/texbin",
                "/Users/me/.cargo/bin",
            ])
        );
    }

    #[test]
    fn without_a_home_only_the_system_places_are_added() {
        let found = directories_from(None, None, None);
        assert_eq!(
            found,
            dirs(&["/opt/homebrew/bin", "/usr/local/bin", "/Library/TeX/texbin"])
        );
    }

    #[test]
    fn the_override_replaces_the_search_entirely() {
        let found = directories_from(
            Some("/only/here:/and/here".into()),
            Some("/a".into()),
            Some("/Users/me".into()),
        );
        assert_eq!(found, dirs(&["/only/here", "/and/here"]));
    }
}
