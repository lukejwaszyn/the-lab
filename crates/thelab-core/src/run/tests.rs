//! Real scripts through the Run engine, with a stand-in interpreter: a
//! shell script that answers `--version` like Python and runs `.py` files
//! with `/bin/sh`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::*;

const STAND_IN: &str = "#!/bin/sh\n\
    if [ \"$1\" = --version ]; then echo 'Python 3.12.4'; exit 0; fi\n\
    exec /bin/sh \"$@\"\n";

/// Long enough for any script here, short enough to fail fast.
const PATIENCE: Duration = Duration::from_secs(15);

/// A scratch project, removed when dropped.
struct Project(PathBuf);

impl Project {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "thelab-run-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(&path).unwrap())
    }

    /// A project whose `.venv` holds the stand-in interpreter.
    fn with_venv(tag: &str) -> Self {
        let project = Self::new(tag);
        project.interpreter(".venv");
        project
    }

    /// The stand-in as `<venv>/bin/python`, made a virtual environment.
    fn interpreter(&self, venv: &str) -> PathBuf {
        let python = self.write(&format!("{venv}/bin/python"), STAND_IN);
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
        self.write(&format!("{venv}/pyvenv.cfg"), "home = /usr/bin\n");
        python
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A sink that keeps what it is given and lets a test wait for it.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Seen>,
    changed: Condvar,
}

#[derive(Default)]
struct Seen {
    lines: Vec<OutputLine>,
    ends: Vec<(String, RunEnd)>,
}

impl RunSink for Recorder {
    fn output(&self, _run_id: &str, lines: Vec<OutputLine>, _dropped: usize) {
        let mut seen = self.seen.lock().unwrap();
        for line in lines {
            // Replace a stream's open line, as the Output panel does.
            let open = seen
                .lines
                .iter()
                .rposition(|l| l.stream == line.stream && l.open);
            match open {
                Some(i) => seen.lines[i] = line,
                None => seen.lines.push(line),
            }
        }
        self.changed.notify_all();
    }

    fn finished(&self, run_id: &str, end: RunEnd) {
        self.seen
            .lock()
            .unwrap()
            .ends
            .push((run_id.to_string(), end));
        self.changed.notify_all();
    }
}

impl Recorder {
    fn wait_until(&self, done: impl Fn(&Seen) -> bool) {
        let seen = self.seen.lock().unwrap();
        let (seen, _) = self
            .changed
            .wait_timeout_while(seen, PATIENCE, |seen| !done(seen))
            .unwrap();
        assert!(done(&seen), "timed out; saw {:#?}", seen.lines);
    }

    fn wait_for_line(&self, text: &str) {
        self.wait_until(|seen| seen.lines.iter().any(|l| l.text == text));
    }

    fn wait_for_end(&self) -> RunEnd {
        self.wait_until(|seen| !seen.ends.is_empty());
        self.seen.lock().unwrap().ends[0].1.clone()
    }

    /// The script's own lines, with the stream they came on.
    fn script_lines(&self) -> Vec<(Stream, String)> {
        self.seen
            .lock()
            .unwrap()
            .lines
            .iter()
            .filter(|l| l.stream != Stream::Meta)
            .map(|l| (l.stream, l.text.clone()))
            .collect()
    }

    fn meta_lines(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .lines
            .iter()
            .filter(|l| l.stream == Stream::Meta)
            .map(|l| l.text.clone())
            .collect()
    }
}

fn start(
    runs: &Runs,
    project: &Project,
    file: &str,
) -> (Arc<Recorder>, Result<RunStarted, String>) {
    let recorder = Arc::new(Recorder::default());
    let started = runs.start("window", "run-1", &project.0, file, recorder.clone());
    (recorder, started)
}

fn process_is_gone(pid: &str) -> bool {
    !Command::new("/bin/kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_script_runs_from_the_project_root_with_its_venv() {
    let project = Project::with_venv("venv");
    project.write(
        "analysis/where.py",
        "pwd\necho \"$1\"\necho \"VIRTUAL_ENV=$VIRTUAL_ENV\"\n\
         echo \"PATH starts ${PATH%%:*}\"\necho \"CLAUDECODE=${CLAUDECODE-unset}\"\n\
         echo \"PYTHONUNBUFFERED=$PYTHONUNBUFFERED\"\n",
    );
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "analysis/where.py");
    let started = started.unwrap();
    let end = recorder.wait_for_end();

    let venv = project.0.join(".venv");
    let out = |text: String| (Stream::Out, text);
    assert_eq!(
        recorder.script_lines(),
        [
            out(project.0.display().to_string()),
            out(String::new()),
            out(format!("VIRTUAL_ENV={}", venv.display())),
            out(format!("PATH starts {}", venv.join("bin").display())),
            out("CLAUDECODE=unset".to_string()),
            out("PYTHONUNBUFFERED=1".to_string()),
        ]
    );
    assert_eq!(end.code, Some(0));
    assert!(!end.stopped);
    assert!(
        end.summary.starts_with("[exited 0 after "),
        "{}",
        end.summary
    );
    assert!(end.summary.ends_with(" s]"), "{}", end.summary);

    assert_eq!(started.file, "analysis/where.py");
    assert_eq!(started.folder, None);
    assert_eq!(
        started.interpreter,
        venv.join("bin/python").display().to_string()
    );
    // The command line comes first, then the script's lines.
    let meta = recorder.meta_lines();
    assert_eq!(meta.len(), 1);
    assert!(
        meta[0].starts_with("$ ") && meta[0].ends_with("/.venv/bin/python analysis/where.py"),
        "{meta:?}"
    );
    assert!(!runs.is_running("window"));
}

#[test]
fn the_folder_in_thelab_toml_picks_the_interpreter() {
    let project = Project::with_venv("folders");
    project.interpreter("envs/openems");
    project.write(
        "thelab.toml",
        "[python.folders]\n\"sim\" = \"envs/openems/bin/python\"\n",
    );
    project.write("sim/antenna.py", "echo \"$VIRTUAL_ENV\"\n");
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "sim/antenna.py");
    let started = started.unwrap();
    recorder.wait_for_end();

    assert_eq!(started.folder.as_deref(), Some("sim"));
    assert_eq!(
        recorder.script_lines(),
        [(
            Stream::Out,
            project.0.join("envs/openems").display().to_string()
        )]
    );
}

#[test]
fn stdout_and_stderr_arrive_interleaved_line_by_line() {
    let project = Project::with_venv("streams");
    project.write(
        "both.py",
        "echo one\nsleep 0.2\necho two >&2\nsleep 0.2\necho three\n",
    );
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "both.py");
    started.unwrap();
    recorder.wait_for_end();

    assert_eq!(
        recorder.script_lines(),
        [
            (Stream::Out, "one".to_string()),
            (Stream::Err, "two".to_string()),
            (Stream::Out, "three".to_string()),
        ]
    );
}

#[test]
fn a_failing_script_ends_with_its_exit_code() {
    let project = Project::with_venv("fails");
    project.write("fails.py", "echo 'something broke' >&2\nexit 3\n");
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "fails.py");
    started.unwrap();
    let end = recorder.wait_for_end();

    assert_eq!(end.code, Some(3));
    assert!(!end.stopped);
    assert!(
        end.summary.starts_with("[exited 3 after "),
        "{}",
        end.summary
    );
    assert_eq!(
        recorder.script_lines(),
        [(Stream::Err, "something broke".to_string())]
    );
}

#[test]
fn stop_sends_sigint_to_the_script() {
    let project = Project::with_venv("sigint");
    project.write(
        "simulate.py",
        "trap 'echo \"got SIGINT\"; exit 130' INT\necho ready\n\
         while :; do sleep 0.05; done\n",
    );
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "simulate.py");
    started.unwrap();
    recorder.wait_for_line("ready");
    let asked = Instant::now();
    assert!(runs.stop("window"));
    assert!(!runs.stop("window"), "a second Stop starts nothing new");
    let end = recorder.wait_for_end();

    assert!(
        asked.elapsed() < STOP_GRACE,
        "it ended on SIGINT, not the kill"
    );
    assert!(end.stopped);
    assert_eq!(end.code, Some(130));
    assert!(
        end.summary.starts_with("[stopped after "),
        "{}",
        end.summary
    );
    assert_eq!(
        recorder.script_lines(),
        [
            (Stream::Out, "ready".to_string()),
            (Stream::Out, "got SIGINT".to_string()),
        ]
    );
}

#[test]
fn a_script_that_ignores_sigint_is_killed_after_the_grace_period() {
    let project = Project::with_venv("stubborn");
    project.write("stubborn.py", "trap '' INT\necho ready\nsleep 30\n");
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "stubborn.py");
    started.unwrap();
    recorder.wait_for_line("ready");
    let asked = Instant::now();
    assert!(runs.stop("window"));
    let end = recorder.wait_for_end();
    let took = asked.elapsed();

    assert!(
        took >= STOP_GRACE,
        "killed after {took:?}, before the grace period"
    );
    assert!(took < STOP_GRACE + Duration::from_secs(3), "took {took:?}");
    assert!(end.stopped);
    assert_eq!(end.signal, Some(9));
    assert!(
        end.summary.starts_with("[stopped after "),
        "{}",
        end.summary
    );
}

#[test]
fn a_second_run_is_refused_while_one_runs() {
    let project = Project::with_venv("second");
    project.write(
        "long.py",
        "trap 'exit 130' INT\necho ready\nwhile :; do sleep 0.05; done\n",
    );
    project.write("quick.py", "echo quick\n");
    let runs = Runs::default();

    let (first, started) = start(&runs, &project, "long.py");
    started.unwrap();
    first.wait_for_line("ready");

    let (second, refused) = start(&runs, &project, "quick.py");
    let reason = refused.unwrap_err();
    assert!(
        reason.starts_with("long.py is still running (for "),
        "{reason}"
    );
    assert!(reason.ends_with("so nothing else was started. Stop it first."));
    // The refusal left the first run alone, and started nothing.
    assert!(runs.is_running("window"));
    assert!(first.seen.lock().unwrap().ends.is_empty());
    assert!(second.seen.lock().unwrap().lines.is_empty());

    // Another window may run its own script meanwhile.
    let other = Arc::new(Recorder::default());
    runs.start("other", "run-2", &project.0, "quick.py", other.clone())
        .unwrap();
    assert_eq!(other.wait_for_end().code, Some(0));

    runs.stop("window");
    assert!(first.wait_for_end().stopped);
    let (third, started) = start(&runs, &project, "quick.py");
    started.unwrap();
    assert_eq!(third.wait_for_end().code, Some(0));
}

#[test]
fn closing_the_project_ends_its_run() {
    let project = Project::with_venv("close");
    project.write(
        "simulate.py",
        "trap 'echo \"saving partial results\"; exit 130' INT\necho \"pid $$\"\n\
         while :; do sleep 0.05; done\n",
    );
    project.write("next.py", "echo next\n");
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "simulate.py");
    started.unwrap();
    recorder.wait_until(|seen| seen.lines.iter().any(|l| l.text.starts_with("pid ")));
    let pid = recorder.script_lines()[0].1["pid ".len()..].to_string();

    let ending = runs.end("window").expect("a run to end");
    // The slot is free at once: the next project can run a script.
    assert!(!runs.is_running("window"));
    assert!(ending.wait(PATIENCE), "the script did not end");
    assert!(process_is_gone(&pid), "process {pid} is still there");
    let end = recorder.wait_for_end();
    assert!(end.stopped);
    assert!(
        recorder
            .script_lines()
            .contains(&(Stream::Out, "saving partial results".to_string()))
    );

    assert!(runs.end("window").is_none(), "nothing left to end");
    let (next, started) = start(&runs, &project, "next.py");
    started.unwrap();
    assert_eq!(next.wait_for_end().code, Some(0));
}

#[test]
fn a_stale_run_is_ended_by_its_id_without_touching_a_newer_one() {
    let project = Project::with_venv("stale");
    project.write(
        "long.py",
        "trap 'exit 130' INT\necho ready\nwhile :; do sleep 0.05; done\n",
    );
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "long.py");
    started.unwrap();
    recorder.wait_for_line("ready");

    assert!(runs.end_run("window", "another-run").is_none());
    assert!(runs.is_running("window"));
    let ending = runs.end_run("window", "run-1").expect("its own run");
    assert!(!runs.is_running("window"));
    assert!(ending.wait(PATIENCE));
    assert!(recorder.wait_for_end().stopped);
}

#[test]
fn quitting_ends_every_run() {
    let project = Project::with_venv("quit");
    project.write("stubborn.py", "trap '' INT\necho ready\nsleep 30\n");
    let runs = Runs::default();

    let mut recorders = Vec::new();
    for owner in ["a", "b"] {
        let recorder = Arc::new(Recorder::default());
        runs.start(owner, owner, &project.0, "stubborn.py", recorder.clone())
            .unwrap();
        recorder.wait_for_line("ready");
        recorders.push(recorder);
    }

    let endings = runs.end_all();
    assert_eq!(endings.len(), 2);
    for ending in &endings {
        assert!(ending.wait(PATIENCE));
    }
    for recorder in recorders {
        assert_eq!(recorder.wait_for_end().signal, Some(9));
    }
}

#[test]
fn a_refused_run_says_why() {
    let project = Project::new("refused");
    project.write("notes.txt", "");
    project.write("run.py", "echo never\n");
    project.write(
        "thelab.toml",
        "[python]\ninterpreter = \"envs/gone/bin/python\"\n",
    );
    let refused = |file: &str| plan(&project.0, file).unwrap_err();

    assert_eq!(
        refused("notes.txt"),
        "Run works on Python files (.py); notes.txt is not one."
    );
    assert_eq!(
        refused("run.py"),
        format!(
            "run.py was not run: {}/envs/gone/bin/python does not exist \
             (interpreter in thelab.toml)",
            project.0.display()
        )
    );
    assert_eq!(
        refused("gone.py"),
        "gone.py was not run: it is not in the project folder any more."
    );
    assert_eq!(
        refused("../elsewhere.py"),
        "../elsewhere.py is not a file inside the project."
    );
    assert_eq!(
        refused("/etc/run.py"),
        "/etc/run.py is not a file inside the project."
    );

    project.write("thelab.toml", "[python]\ninterpreters = \"x\"\n");
    assert!(
        refused("run.py").starts_with("run.py was not run: Invalid "),
        "{}",
        refused("run.py")
    );

    // A refused start leaves nothing behind.
    let runs = Runs::default();
    let (recorder, started) = start(&runs, &project, "notes.txt");
    assert!(started.is_err());
    assert!(!runs.is_running("window"));
    assert!(recorder.seen.lock().unwrap().lines.is_empty());
}

#[test]
fn the_environment_drops_claude_code_and_activates_the_venv() {
    let base = [
        ("HOME", "/Users/me"),
        ("LANG", "en_US.UTF-8"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_CODE_ENTRYPOINT", "cli"),
        ("CLAUDE_CODE_SSE_PORT", "1234"),
        ("PATH", "/usr/bin"),
        ("VIRTUAL_ENV", "/somewhere/else"),
        ("PYTHONHOME", "/opt/python"),
    ]
    .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let path = OsString::from("/venv/bin:/opt/homebrew/bin:/usr/bin");
    let as_strings = |env: Vec<(OsString, OsString)>| -> Vec<(String, String)> {
        env.into_iter()
            .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
            .collect()
    };
    let pair = |k: &str, v: &str| (k.to_string(), v.to_string());

    let venv = as_strings(child_env(base.clone(), &path, Some(Path::new("/venv"))));
    assert_eq!(
        venv,
        [
            pair("HOME", "/Users/me"),
            pair("LANG", "en_US.UTF-8"),
            pair("PATH", "/venv/bin:/opt/homebrew/bin:/usr/bin"),
            pair("VIRTUAL_ENV", "/venv"),
            pair("PYTHONUNBUFFERED", "1"),
        ]
    );

    let plain = as_strings(child_env(base, &OsString::from("/usr/bin"), None));
    assert_eq!(
        plain,
        [
            pair("HOME", "/Users/me"),
            pair("LANG", "en_US.UTF-8"),
            pair("PYTHONHOME", "/opt/python"),
            pair("PATH", "/usr/bin"),
            pair("PYTHONUNBUFFERED", "1"),
        ]
    );
}

#[test]
fn the_venv_bin_comes_first_on_path_even_when_it_was_not_there() {
    let env = child_env(
        Vec::new(),
        &OsString::from("/usr/bin:/bin"),
        Some(Path::new("/Users/me/opt/openEMS/venv")),
    );
    assert_eq!(env[0].0, "PATH");
    assert_eq!(env[0].1, "/Users/me/opt/openEMS/venv/bin:/usr/bin:/bin");
}

fn line(stream: Stream, text: &str, open: bool) -> OutputLine {
    OutputLine {
        stream,
        text: text.to_string(),
        open,
    }
}

#[test]
fn pending_output_replaces_a_streams_open_line_in_place() {
    let pending = Pending::default();
    pending.push(
        Stream::Err,
        vec![Segment {
            text: " 10%".into(),
            open: true,
        }],
    );
    pending.push(
        Stream::Out,
        vec![Segment {
            text: "loaded".into(),
            open: false,
        }],
    );
    pending.push(
        Stream::Err,
        vec![Segment {
            text: " 60%".into(),
            open: true,
        }],
    );
    pending.push(
        Stream::Err,
        vec![Segment {
            text: "100%".into(),
            open: false,
        }],
    );
    pending.push(
        Stream::Err,
        vec![Segment {
            text: "done".into(),
            open: false,
        }],
    );

    let (lines, dropped, closed) = pending.take();
    assert_eq!(
        lines,
        [
            line(Stream::Err, "100%", false),
            line(Stream::Out, "loaded", false),
            line(Stream::Err, "done", false),
        ]
    );
    assert_eq!((dropped, closed), (0, false));
}

#[test]
fn an_open_line_already_delivered_is_not_replaced_in_the_next_delivery() {
    let pending = Pending::default();
    pending.push(
        Stream::Out,
        vec![Segment {
            text: "work".into(),
            open: true,
        }],
    );
    let (first, _, _) = pending.take();
    assert_eq!(first, [line(Stream::Out, "work", true)]);

    // The panel replaces it; here it is simply the next line.
    pending.push(
        Stream::Out,
        vec![Segment {
            text: "working".into(),
            open: false,
        }],
    );
    let (second, _, _) = pending.take();
    assert_eq!(second, [line(Stream::Out, "working", false)]);
}

#[test]
fn pending_output_is_bounded_and_counts_what_it_dropped() {
    let pending = Pending::default();
    let segments: Vec<Segment> = (0..MAX_PENDING_LINES + 25)
        .map(|i| Segment {
            text: i.to_string(),
            open: false,
        })
        .collect();
    pending.push(Stream::Out, segments);
    pending.push(
        Stream::Err,
        vec![Segment {
            text: "p".into(),
            open: true,
        }],
    );
    pending.push(
        Stream::Err,
        vec![Segment {
            text: "q".into(),
            open: true,
        }],
    );

    let (lines, dropped, _) = pending.take();
    assert_eq!(lines.len(), MAX_PENDING_LINES);
    assert_eq!(dropped, 26);
    assert_eq!(lines[0].text, "26");
    assert_eq!(lines.last(), Some(&line(Stream::Err, "q", true)));
}

#[test]
fn nothing_is_taken_in_after_the_run_is_over() {
    let pending = Pending::default();
    pending.close();
    pending.push(
        Stream::Out,
        vec![Segment {
            text: "late".into(),
            open: false,
        }],
    );
    assert_eq!(pending.take(), (Vec::new(), 0, true));
}

#[test]
fn durations_read_at_a_glance() {
    assert_eq!(duration(0.04), "0.0 s");
    assert_eq!(duration(12.44), "12.4 s");
    assert_eq!(duration(59.9), "59.9 s");
    assert_eq!(duration(59.97), "60 s (1 min 0 s)");
    assert_eq!(duration(754.2), "754 s (12 min 34 s)");
    assert_eq!(duration(5025.0), "5025 s (1 h 23 min)");
}

#[test]
fn the_command_line_abbreviates_home_and_quotes_spaces() {
    let home = std::env::var("HOME").unwrap();
    let plan = Plan {
        root: PathBuf::from("/p"),
        file: "sim/my run.py".to_string(),
        program: PathBuf::from(format!("{home}/opt/openEMS/venv/bin/python")),
        virtual_env: None,
        folder: None,
    };
    assert_eq!(
        command_line(&plan),
        "~/opt/openEMS/venv/bin/python 'sim/my run.py'"
    );
}

#[test]
fn how_a_run_ended_is_said_plainly() {
    use std::os::unix::process::ExitStatusExt;
    let exited = |code: i32| Some(ExitStatus::from_raw(code << 8));
    let killed = |signal: i32| Some(ExitStatus::from_raw(signal));

    assert_eq!(
        run_end(exited(0), false, 1.25).summary,
        "[exited 0 after 1.2 s]"
    );
    assert_eq!(
        run_end(exited(2), false, 0.5).summary,
        "[exited 2 after 0.5 s]"
    );
    assert_eq!(
        run_end(exited(130), true, 4.0).summary,
        "[stopped after 4.0 s]"
    );
    assert_eq!(
        run_end(killed(9), true, 3.1).summary,
        "[stopped after 3.1 s]"
    );
    assert_eq!(
        run_end(killed(11), false, 2.0).summary,
        "[ended by signal 11 (SIGSEGV) after 2.0 s]"
    );
    assert_eq!(run_end(None, false, 2.0).summary, "[ended after 2.0 s]");
}

/// The same with a real Python: prints show as they happen, a progress bar
/// redraws its line, and Stop raises KeyboardInterrupt in the script.
/// `cargo test -p thelab-core real_python -- --ignored`; needs python3.
#[test]
#[ignore]
fn real_python_streams_and_stops() {
    let python = crate::tools::find_program("python3").expect("python3");
    let project = Project::new("real");
    project.write(
        "thelab.toml",
        &format!("[python]\ninterpreter = \"{}\"\n", python.display()),
    );
    project.write(
        "simulate.py",
        "import sys, time\n\
         for i in range(1, 4):\n    sys.stderr.write(f'\\r{i * 33}%')\n    time.sleep(0.1)\n\
         sys.stderr.write('\\n')\n\
         print('ready')\n\
         try:\n    time.sleep(60)\n\
         except KeyboardInterrupt:\n    print('interrupted, saving')\n    sys.exit(130)\n",
    );
    let runs = Runs::default();

    let (recorder, started) = start(&runs, &project, "simulate.py");
    started.unwrap();
    recorder.wait_for_line("ready");
    assert!(runs.stop("window"));
    let end = recorder.wait_for_end();

    assert_eq!(
        recorder.script_lines(),
        [
            (Stream::Err, "99%".to_string()),
            (Stream::Out, "ready".to_string()),
            (Stream::Out, "interrupted, saving".to_string()),
        ]
    );
    assert!(end.stopped);
    assert_eq!(end.code, Some(130));
}
