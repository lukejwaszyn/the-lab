//! Running external programs.
#![cfg(unix)]

use std::process::Command;
use std::time::{Duration, Instant};

use thelab_core::tools::run;

#[test]
fn a_program_that_runs_too_long_is_stopped() {
    let started = Instant::now();
    let output = run(Command::new("sleep").arg("30"), Duration::from_millis(300)).unwrap();
    assert!(
        output.status.is_none(),
        "the status is absent after a timeout"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_timeout_also_stops_the_grandchildren() {
    // latexmk runs pdflatex as a child. Killing only latexmk would leave
    // pdflatex holding the output pipe open, and `run` would wait for it.
    let started = Instant::now();
    let output = run(
        Command::new("sh").args(["-c", "sleep 30; echo done"]),
        Duration::from_millis(300),
    )
    .unwrap();
    assert!(output.status.is_none());
    assert_eq!(output.stdout_text(), "");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn children_get_no_stdin_and_their_output_is_captured() {
    // `cat` with nothing to read ends at once. If it inherited stdin, it
    // could swallow the MCP server's incoming messages.
    let output = run(&mut Command::new("cat"), Duration::from_secs(10)).unwrap();
    assert!(output.status.unwrap().success());
    assert_eq!(output.stdout, b"");

    let output = run(
        Command::new("sh").args(["-c", "printf out; printf err >&2; exit 3"]),
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(output.status.unwrap().code(), Some(3));
    assert_eq!(output.stdout_text(), "out");
    assert_eq!(output.stderr, "err");
}
