//! Runs a project's Python scripts for the app's Run command: with the
//! interpreter `thelab.toml` names for the script's folder, from the project
//! root, one script at a time per window, with stdout and stderr streamed
//! line by line.
//!
//! A script runs in its own process group. Stop sends it SIGINT, as
//! Control-C in a terminal would, and kills the group if it is still there
//! after `STOP_GRACE`. Its stdin is empty, so a run can never sit waiting
//! for input that no one can give.

pub mod lines;

use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::python;
use lines::{LineSplitter, Segment};

/// How long a script has to end after SIGINT before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// Lines held between two deliveries. Older ones are dropped and counted,
/// so a script that prints faster than the panel can show stays bounded.
pub const MAX_PENDING_LINES: usize = 10_000;
/// How often output is delivered while a script writes.
const DELIVERY_INTERVAL: Duration = Duration::from_millis(40);
/// How long to wait for the pipes to close once the script has exited: a
/// background process it started can hold them open.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Out,
    Err,
    /// Written by The Lab, not the script: the command line.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputLine {
    pub stream: Stream,
    pub text: String,
    /// Still being written: the stream's next line replaces it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEnd {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// Stop was asked for.
    pub stopped: bool,
    pub seconds: f64,
    /// The panel's last line: `[exited 0 after 1.2 s]`, `[stopped after 4.0 s]`.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStarted {
    pub run_id: String,
    /// Relative to the project root.
    pub file: String,
    pub interpreter: String,
    /// The `[python.folders]` entry that chose the interpreter; `None` for
    /// the project's default.
    pub folder: Option<String>,
    /// What runs, as the panel shows it: `~/opt/openEMS/venv/bin/python sim/run.py`.
    pub command: String,
}

/// Where a run's output and its end go.
pub trait RunSink: Send + Sync + 'static {
    /// New lines, in order; `dropped` lines before them were not kept.
    fn output(&self, run_id: &str, lines: Vec<OutputLine>, dropped: usize);
    /// The last call for a run.
    fn finished(&self, run_id: &str, end: RunEnd);
}

/// What Run will do with a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The project root, resolved.
    pub root: PathBuf,
    /// The script, relative to the root.
    pub file: String,
    pub program: PathBuf,
    pub virtual_env: Option<PathBuf>,
    pub folder: Option<String>,
}

/// Decides how `file` (relative to `root`) runs, reading `thelab.toml` and
/// asking the file's interpreter for its version afresh, so an edit to the
/// file counts at once. The error says in plain words why it cannot run.
pub fn plan(root: &Path, file: &str) -> Result<Plan, String> {
    let file = relative_file(file)?;
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    if !file.to_ascii_lowercase().ends_with(".py") {
        return Err(format!(
            "Run works on Python files (.py); {name} is not one."
        ));
    }
    if !root.join(&file).is_file() {
        return Err(format!(
            "{file} was not run: it is not in the project folder any more."
        ));
    }
    let chosen = python::interpreter_for_file(root, &file)
        .map_err(|e| format!("{name} was not run: {e}"))?;
    let interpreter = chosen.interpreter;
    if let Some(problem) = &interpreter.problem {
        return Err(format!("{name} was not run: {problem}"));
    }
    let Some(program) = &interpreter.path else {
        return Err(format!(
            "{name} was not run: no Python interpreter was found."
        ));
    };
    Ok(Plan {
        root: chosen.root,
        file,
        program: PathBuf::from(program),
        virtual_env: interpreter.virtual_env.as_ref().map(PathBuf::from),
        folder: chosen.folder,
    })
}

/// `file` tidied, or why it is not a file inside the project.
fn relative_file(file: &str) -> Result<String, String> {
    let tidy = file.trim().trim_start_matches("./");
    let inside = !tidy.is_empty()
        && Path::new(tidy)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if inside {
        Ok(tidy.to_string())
    } else {
        Err(format!("{file} is not a file inside the project."))
    }
}

/// The script's environment: The Lab's own, without Claude Code's variables
/// (the script is not running inside Claude Code), with `PATH`, and for a
/// virtual environment `VIRTUAL_ENV`, set as activating it would.
pub fn child_env(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    path: &OsStr,
    virtual_env: Option<&Path>,
) -> Vec<(OsString, OsString)> {
    let in_venv = virtual_env.is_some();
    let mut env: Vec<(OsString, OsString)> = base
        .into_iter()
        .filter(|(key, _)| keep_variable(key, in_venv))
        .collect();
    let path = match virtual_env {
        Some(venv) => {
            let bin = venv.join("bin");
            let rest = std::env::split_paths(path).filter(|dir| *dir != bin);
            std::env::join_paths(std::iter::once(bin.clone()).chain(rest))
                .unwrap_or_else(|_| bin.into_os_string())
        }
        None => path.to_os_string(),
    };
    env.push(("PATH".into(), path));
    if let Some(venv) = virtual_env {
        env.push(("VIRTUAL_ENV".into(), venv.into()));
    }
    // Python buffers stdout into a pipe; this shows each print as it happens.
    env.push(("PYTHONUNBUFFERED".into(), "1".into()));
    env
}

fn keep_variable(key: &OsStr, in_venv: bool) -> bool {
    let Some(key) = key.to_str() else {
        return true;
    };
    let claude_code = key == "CLAUDECODE" || key.starts_with("CLAUDE_CODE_");
    let replaced = matches!(
        key,
        "PATH" | "VIRTUAL_ENV" | "PYTHONUNBUFFERED" | "__PYVENV_LAUNCHER__"
    );
    // Activating a venv unsets PYTHONHOME, which would point Python elsewhere.
    !(claude_code || replaced || (in_venv && key == "PYTHONHOME"))
}

/// The scripts running, one at most per owner (a window of the app).
#[derive(Clone)]
pub struct Runs {
    active: Arc<Mutex<HashMap<String, Active>>>,
    grace: Duration,
}

impl Default for Runs {
    fn default() -> Self {
        Self {
            active: Arc::default(),
            grace: STOP_GRACE,
        }
    }
}

struct Active {
    run_id: String,
    file: String,
    started: Instant,
    process: Arc<Process>,
}

/// A run being ended; `wait` blocks until its process is gone.
pub struct Ending(Arc<Process>);

impl Ending {
    /// Whether the process ended within `timeout`.
    pub fn wait(&self, timeout: Duration) -> bool {
        self.0.wait(timeout)
    }
}

impl Runs {
    /// Starts `file` for `owner`, unless a script of that owner is still
    /// running: Run never stops or replaces one.
    pub fn start(
        &self,
        owner: &str,
        run_id: &str,
        root: &Path,
        file: &str,
        sink: Arc<dyn RunSink>,
    ) -> Result<RunStarted, String> {
        if let Some(active) = lock(&self.active).get(owner) {
            return Err(still_running(active));
        }
        // Asking the interpreters for their versions takes a moment, so it
        // happens before the lock is taken again.
        let plan = plan(root, file)?;
        let env = child_env(
            std::env::vars_os(),
            &crate::tools::child_path(),
            plan.virtual_env.as_deref(),
        );

        let mut active = lock(&self.active);
        if let Some(running) = active.get(owner) {
            return Err(still_running(running));
        }
        let mut command = Command::new(&plan.program);
        command
            .arg(&plan.file)
            .current_dir(&plan.root)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let started = Instant::now();
        let child = command.spawn().map_err(|e| {
            format!(
                "{} was not run: {} could not be started: {e}",
                plan.file,
                plan.program.display()
            )
        })?;
        let process = Arc::new(Process::new(child.id()));
        active.insert(
            owner.to_string(),
            Active {
                run_id: run_id.to_string(),
                file: plan.file.clone(),
                started,
                process: Arc::clone(&process),
            },
        );
        drop(active);

        let command_line = command_line(&plan);
        watch(Watch {
            runs: self.clone(),
            owner: owner.to_string(),
            run_id: run_id.to_string(),
            child,
            process,
            started,
            header: format!("$ {command_line}"),
            sink,
        });
        Ok(RunStarted {
            run_id: run_id.to_string(),
            file: plan.file,
            interpreter: plan.program.display().to_string(),
            folder: plan.folder,
            command: command_line,
        })
    }

    /// Whether `owner` has a script running.
    pub fn is_running(&self, owner: &str) -> bool {
        lock(&self.active).contains_key(owner)
    }

    /// Stops `owner`'s script: SIGINT now, SIGKILL after the grace period.
    /// False when nothing runs or a stop is already under way.
    pub fn stop(&self, owner: &str) -> bool {
        let process = lock(&self.active)
            .get(owner)
            .map(|active| Arc::clone(&active.process));
        process.is_some_and(|process| process.stop(self.grace))
    }

    /// Ends `owner`'s script because its project or window is closing:
    /// stopped as `stop` does, and forgotten at once, so the next project
    /// can run a script straight away.
    pub fn end(&self, owner: &str) -> Option<Ending> {
        let active = lock(&self.active).remove(owner)?;
        active.process.stop(self.grace);
        Some(Ending(active.process))
    }

    /// `end`, but only if `owner`'s script is the run `run_id`: for a run
    /// found to be stale, without ending a newer one.
    pub fn end_run(&self, owner: &str, run_id: &str) -> Option<Ending> {
        let mut active = lock(&self.active);
        if active.get(owner)?.run_id != run_id {
            return None;
        }
        let ended = active.remove(owner)?;
        drop(active);
        ended.process.stop(self.grace);
        Some(Ending(ended.process))
    }

    /// Ends every script, for quitting the app.
    pub fn end_all(&self) -> Vec<Ending> {
        let owners: Vec<String> = lock(&self.active).keys().cloned().collect();
        owners.iter().filter_map(|owner| self.end(owner)).collect()
    }

    fn forget(&self, owner: &str, run_id: &str) {
        let mut active = lock(&self.active);
        if active.get(owner).is_some_and(|a| a.run_id == run_id) {
            active.remove(owner);
        }
    }
}

fn still_running(active: &Active) -> String {
    format!(
        "{} is still running (for {}), so nothing else was started. Stop it first.",
        active.file,
        duration(active.started.elapsed().as_secs_f64())
    )
}

/// A script's process: the leader of its own process group.
struct Process {
    group: u32,
    exited: Mutex<bool>,
    exited_changed: Condvar,
    stopping: AtomicBool,
}

impl Process {
    fn new(group: u32) -> Self {
        Self {
            group,
            exited: Mutex::new(false),
            exited_changed: Condvar::new(),
            stopping: AtomicBool::new(false),
        }
    }

    /// SIGINT to the group, then SIGKILL after `grace` if the script has not
    /// ended. False if it had ended or a stop was already under way.
    fn stop(self: &Arc<Self>, grace: Duration) -> bool {
        if self.stopping.swap(true, Ordering::SeqCst) || *lock(&self.exited) {
            return false;
        }
        signal_group(self.group, "INT");
        let process = Arc::clone(self);
        std::thread::spawn(move || {
            let exited = lock(&process.exited);
            let (exited, _) = process
                .exited_changed
                .wait_timeout_while(exited, grace, |exited| !*exited)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Still holding the lock: the script cannot be reaped and its
            // group number reused between this check and the signal.
            if !*exited {
                signal_group(process.group, "KILL");
            }
        });
        true
    }

    fn mark_exited(&self) {
        *lock(&self.exited) = true;
        self.exited_changed.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let exited = lock(&self.exited);
        let (exited, _) = self
            .exited_changed
            .wait_timeout_while(exited, timeout, |exited| !*exited)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *exited
    }
}

fn signal_group(group: u32, signal: &str) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args([format!("-{signal}"), "--".to_string(), format!("-{group}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    {
        // No signals: end the process tree.
        let _ = signal;
        let _ = Command::new("taskkill")
            .args(["/PID", &group.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct Watch {
    runs: Runs,
    owner: String,
    run_id: String,
    child: std::process::Child,
    process: Arc<Process>,
    started: Instant,
    header: String,
    sink: Arc<dyn RunSink>,
}

/// Reads the script's output and waits for it to end, on threads of their own.
fn watch(watch: Watch) {
    let Watch {
        runs,
        owner,
        run_id,
        mut child,
        process,
        started,
        header,
        sink,
    } = watch;
    let pending = Arc::new(Pending::default());
    pending.push(
        Stream::Meta,
        vec![Segment {
            text: header,
            open: false,
        }],
    );

    let (pipe_closed, pipes_closed) = mpsc::channel::<()>();
    let pipes: [(Stream, Option<Box<dyn Read + Send>>); 2] = [
        (
            Stream::Out,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            Stream::Err,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ];
    let mut readers = 0;
    for (stream, pipe) in pipes {
        let Some(pipe) = pipe else { continue };
        readers += 1;
        let pending = Arc::clone(&pending);
        let pipe_closed = pipe_closed.clone();
        std::thread::spawn(move || {
            read_stream(pipe, stream, &pending);
            let _ = pipe_closed.send(());
        });
    }
    drop(pipe_closed);

    let delivery = {
        let pending = Arc::clone(&pending);
        let sink = Arc::clone(&sink);
        let run_id = run_id.clone();
        std::thread::spawn(move || deliver(&pending, sink.as_ref(), &run_id))
    };

    std::thread::spawn(move || {
        let status = child.wait();
        let seconds = started.elapsed().as_secs_f64();
        process.mark_exited();
        let stopped = process.stopping.load(Ordering::SeqCst);
        if stopped {
            // What the script started goes with it.
            signal_group(process.group, "KILL");
        }
        let deadline = Instant::now() + DRAIN_TIMEOUT;
        for _ in 0..readers {
            let left = deadline.saturating_duration_since(Instant::now());
            if pipes_closed.recv_timeout(left).is_err() {
                break;
            }
        }
        pending.close();
        let _ = delivery.join();
        // Forgotten before the end is reported, so a Run that answers the
        // report is not refused.
        runs.forget(&owner, &run_id);
        sink.finished(&run_id, run_end(status.ok(), stopped, seconds));
    });
}

fn read_stream(mut pipe: impl Read, stream: Stream, pending: &Pending) {
    let mut splitter = LineSplitter::default();
    let mut buffer = [0u8; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => pending.push(stream, splitter.push(&buffer[..n])),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    pending.push(stream, splitter.finish());
}

fn deliver(pending: &Pending, sink: &dyn RunSink, run_id: &str) {
    loop {
        let (lines, dropped, closed) = pending.take();
        if !lines.is_empty() || dropped > 0 {
            sink.output(run_id, lines, dropped);
        }
        if closed {
            break;
        }
        std::thread::sleep(DELIVERY_INTERVAL);
    }
}

/// Output read but not yet delivered.
#[derive(Default)]
struct Pending {
    state: Mutex<PendingState>,
    changed: Condvar,
}

#[derive(Default)]
struct PendingState {
    lines: VecDeque<OutputLine>,
    /// How many lines left `lines` from the front, delivered or dropped:
    /// `lines[i]` is line number `first + i` of the run.
    first: usize,
    /// The number of the open line of stdout and of stderr, if any.
    open: [Option<usize>; 2],
    dropped: usize,
    closed: bool,
}

impl Pending {
    fn push(&self, stream: Stream, segments: Vec<Segment>) {
        if segments.is_empty() {
            return;
        }
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        for segment in segments {
            state.add(OutputLine {
                stream,
                text: segment.text,
                open: segment.open,
            });
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Everything pending, waiting until there is something or the run
    /// is over; and whether it is over.
    fn take(&self) -> (Vec<OutputLine>, usize, bool) {
        let mut state = lock(&self.state);
        while state.lines.is_empty() && state.dropped == 0 && !state.closed {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        let lines: Vec<OutputLine> = state.lines.drain(..).collect();
        state.first += lines.len();
        // A delivered open line is replaced where it was delivered to.
        state.open = [None, None];
        let dropped = std::mem::take(&mut state.dropped);
        (lines, dropped, state.closed)
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.changed.notify_all();
    }
}

impl PendingState {
    /// Adds a line, replacing its stream's open line in place if it has one
    /// (the same rule the Output panel follows across deliveries).
    fn add(&mut self, line: OutputLine) {
        let slot = match line.stream {
            Stream::Out => Some(0),
            Stream::Err => Some(1),
            Stream::Meta => None,
        };
        let open = line.open;
        if let Some(slot) = slot {
            let held = self.open[slot]
                .and_then(|number| number.checked_sub(self.first))
                .filter(|&i| i < self.lines.len());
            if let Some(i) = held {
                self.lines[i] = line;
                if !open {
                    self.open[slot] = None;
                }
                return;
            }
        }
        let number = self.first + self.lines.len();
        self.lines.push_back(line);
        if let Some(slot) = slot {
            self.open[slot] = open.then_some(number);
        }
        while self.lines.len() > MAX_PENDING_LINES {
            self.lines.pop_front();
            self.first += 1;
            self.dropped += 1;
        }
    }
}

fn run_end(status: Option<ExitStatus>, stopped: bool, seconds: f64) -> RunEnd {
    let code = status.and_then(|s| s.code());
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.and_then(|s| s.signal())
    };
    #[cfg(not(unix))]
    let signal = None;
    let after = duration(seconds);
    let summary = if stopped {
        format!("[stopped after {after}]")
    } else if let Some(code) = code {
        format!("[exited {code} after {after}]")
    } else if let Some(signal) = signal {
        let name = signal_name(signal)
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        format!("[ended by signal {signal}{name} after {after}]")
    } else {
        format!("[ended after {after}]")
    };
    RunEnd {
        code,
        signal,
        stopped,
        seconds,
        summary,
    }
}

fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        10 => "SIGBUS",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return None,
    })
}

/// `0.4 s`, `12.4 s`, and from a minute on `754 s (12 min 34 s)` or
/// `5025 s (1 h 23 min)`, so a long simulation's time reads at a glance.
pub fn duration(seconds: f64) -> String {
    if seconds < 59.95 {
        return format!("{seconds:.1} s");
    }
    let whole = seconds.round() as u64;
    let (hours, minutes, rest) = (whole / 3600, whole / 60 % 60, whole % 60);
    let long = if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min {rest} s")
    };
    format!("{whole} s ({long})")
}

/// The command as the panel shows it, with `~` for the home folder.
fn command_line(plan: &Plan) -> String {
    let program = plan.program.display().to_string();
    let program = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && program.starts_with(&format!("{home}/")) => {
            format!("~{}", &program[home.len()..])
        }
        _ => program,
    };
    format!("{} {}", quoted(&program), quoted(&plan.file))
}

fn quoted(word: &str) -> String {
    let plain = word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~+=:,@".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, unix))]
mod tests;
