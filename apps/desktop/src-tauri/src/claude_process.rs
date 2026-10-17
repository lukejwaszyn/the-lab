use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tauri::{Emitter, Manager, WebviewWindow};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use crate::claude_control::{self, ControlMessage};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// The Lab: one run's open stdin, kept so the app can answer Claude's
/// permission requests, and the requests it is still waiting on.
pub struct ClaudeControl {
    /// Which spawn this belongs to, so a finished run never closes the stdin
    /// of a newer run in the same tab.
    run_id: u64,
    stdin: ChildStdin,
    /// request_id -> the tool input Claude asked to run with.
    pending: HashMap<String, Value>,
}

#[derive(Clone)]
pub struct ClaudeProcessState {
    pub processes: Arc<Mutex<HashMap<String, Child>>>,
    /// Keyed like `processes` (window label and tab id).
    pub controls: Arc<Mutex<HashMap<String, ClaudeControl>>>,
}

impl Default for ClaudeProcessState {
    fn default() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
            controls: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, serde::Serialize)]
struct ClaudeOutputEvent {
    tab_id: String,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct ClaudeCompleteEvent {
    tab_id: String,
    success: bool,
}

#[derive(Clone, serde::Serialize)]
struct ClaudeErrorEvent {
    tab_id: String,
    data: String,
}

/// Where a run's events go: the window in the app, a channel in tests.
pub(crate) trait ClaudeEventSink: Clone + Send + Sync + 'static {
    fn emit_output(&self, tab_id: &str, data: String);
    fn emit_error(&self, tab_id: &str, data: String);
    fn emit_complete(&self, tab_id: &str, success: bool);
}

impl ClaudeEventSink for WebviewWindow {
    fn emit_output(&self, tab_id: &str, data: String) {
        let _ = self.emit(
            "claude-output",
            ClaudeOutputEvent {
                tab_id: tab_id.to_string(),
                data,
            },
        );
    }

    fn emit_error(&self, tab_id: &str, data: String) {
        let _ = self.emit(
            "claude-error",
            ClaudeErrorEvent {
                tab_id: tab_id.to_string(),
                data,
            },
        );
    }

    fn emit_complete(&self, tab_id: &str, success: bool) {
        let _ = self.emit(
            "claude-complete",
            ClaudeCompleteEvent {
                tab_id: tab_id.to_string(),
                success,
            },
        );
    }
}

#[derive(Clone)]
pub struct SpawnProviderMetadata {
    pub provider: &'static str,
    pub provider_credential_id: String,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeStopMode {
    /// User pressed Stop; terminate the run immediately.
    Terminate,
    /// User wants to guide the next turn; prefer a graceful interrupt so
    /// Claude Code can persist session state before the frontend resumes it.
    Interrupt,
}

fn process_key(window_label: &str, tab_id: &str) -> String {
    format!("{}:{}", window_label, tab_id)
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

/// Write one control line to a run's stdin, if that run is still open.
async fn write_control_line(
    controls: &Mutex<HashMap<String, ClaudeControl>>,
    key: &str,
    run_id: u64,
    line: &str,
) -> Result<(), String> {
    let mut controls = controls.lock().await;
    match controls.get_mut(key) {
        Some(control) if control.run_id == run_id => write_line(&mut control.stdin, line)
            .await
            .map_err(|e| format!("Failed to write to Claude process stdin: {}", e)),
        _ => Err("Claude process stdin is already closed".to_string()),
    }
}

/// Close a run's stdin (dropping it sends end of file), which lets Claude exit
/// once its turn is over. Pending permission requests go with it.
async fn close_control(controls: &Mutex<HashMap<String, ClaudeControl>>, key: &str, run_id: u64) {
    let mut controls = controls.lock().await;
    if controls.get(key).map(|c| c.run_id) == Some(run_id) {
        controls.remove(key);
    }
}

/// Spawn the Claude CLI process and stream output via Tauri events.
/// Events are emitted only to the originating window, tagged with tab_id.
///
/// The Lab: `user_message_line` is the prompt as one stream-json line. Stdin
/// stays open for permission answers until the run's `result` arrives.
pub async fn spawn_claude_process(
    window: WebviewWindow,
    cmd: Command,
    tab_id: String,
    user_message_line: String,
    provider_metadata: Option<SpawnProviderMetadata>,
) -> Result<(), String> {
    let state = window.state::<ClaudeProcessState>().inner().clone();
    let window_label = window.label().to_string();
    spawn_claude_process_with(
        window,
        state,
        &window_label,
        cmd,
        tab_id,
        user_message_line,
        provider_metadata,
    )
    .await
}

pub(crate) async fn spawn_claude_process_with<S: ClaudeEventSink>(
    sink: S,
    state: ClaudeProcessState,
    window_label: &str,
    mut cmd: Command,
    tab_id: String,
    user_message_line: String,
    provider_metadata: Option<SpawnProviderMetadata>,
) -> Result<(), String> {
    let process_key = process_key(window_label, &tab_id);

    cmd.stdin(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        eprintln!(
            "[claude-spawn] Failed to spawn process for tab {}: {}",
            tab_id, e
        );
        format!(
            "Failed to spawn Claude process: {}. Is Claude Code CLI installed?",
            e
        )
    })?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.start_kill();
        return Err("Failed to acquire stdin for Claude process".to_string());
    };
    if let Err(e) = write_line(&mut stdin, &user_message_line).await {
        let _ = child.start_kill();
        return Err(format!(
            "Failed to write prompt to Claude process stdin: {}",
            e
        ));
    }

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

    let process_arc = state.processes.clone();
    let controls_arc = state.controls.clone();
    let run_id = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);

    {
        let mut processes = process_arc.lock().await;
        if let Some(mut existing) = processes.remove(&process_key) {
            let _ = existing.kill().await;
        }
        processes.insert(process_key.clone(), child);
    }
    controls_arc.lock().await.insert(
        process_key.clone(),
        ClaudeControl {
            run_id,
            stdin,
            pending: HashMap::new(),
        },
    );

    let stdout_reader = BufReader::new(stdout);
    let stderr_reader = BufReader::new(stderr);
    let result_success_holder: Arc<std::sync::Mutex<Option<bool>>> =
        Arc::new(std::sync::Mutex::new(None));

    let start_time = std::time::Instant::now();

    let sink_stdout = sink.clone();
    let result_success_stdout = result_success_holder.clone();
    let tab_id_stdout = tab_id.clone();
    let provider_metadata_stdout = provider_metadata.clone();
    let controls_stdout = controls_arc.clone();
    let process_key_stdout = process_key.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = stdout_reader.lines();
        let mut line_count: u64 = 0;
        while let Ok(Some(mut line)) = lines.next_line().await {
            line_count += 1;
            let elapsed = start_time.elapsed().as_secs_f64();
            let mut close_stdin_after_emit = false;

            if let Ok(mut msg) = serde_json::from_str::<serde_json::Value>(&line) {
                let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                let msg_sub = msg.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                eprintln!(
                    "[claude-stdout] [{}] +{:.1}s #{} type={} sub={} len={}",
                    tab_id_stdout,
                    elapsed,
                    line_count,
                    msg_type,
                    msg_sub,
                    line.len()
                );

                match claude_control::classify(&msg) {
                    Some(ControlMessage::PermissionRequest { request_id, input }) => {
                        // Remember the input before the chat sees the request,
                        // so an answer can never arrive ahead of it.
                        let mut controls = controls_stdout.lock().await;
                        if let Some(control) = controls.get_mut(&process_key_stdout) {
                            if control.run_id == run_id {
                                control.pending.insert(request_id, input);
                            }
                        }
                    }
                    Some(ControlMessage::UnsupportedRequest {
                        request_id,
                        subtype,
                    }) => {
                        eprintln!(
                            "[claude-control] [{}] refusing unsupported control request {} ({})",
                            tab_id_stdout, request_id, subtype
                        );
                        if let Err(e) = write_control_line(
                            &controls_stdout,
                            &process_key_stdout,
                            run_id,
                            &claude_control::unsupported_line(&request_id),
                        )
                        .await
                        {
                            eprintln!("[claude-control] [{}] {}", tab_id_stdout, e);
                        }
                        // Nothing for the chat to show.
                        continue;
                    }
                    Some(ControlMessage::Cancel { request_id }) => {
                        let mut controls = controls_stdout.lock().await;
                        if let Some(control) = controls.get_mut(&process_key_stdout) {
                            if control.run_id == run_id {
                                control.pending.remove(&request_id);
                            }
                        }
                    }
                    None => {}
                }

                if msg.get("type").and_then(|v| v.as_str()) == Some("system")
                    && msg.get("subtype").and_then(|v| v.as_str()) == Some("init")
                {
                    if let Some(metadata) = provider_metadata_stdout.as_ref() {
                        if let Some(object) = msg.as_object_mut() {
                            object.insert(
                                "provider".to_string(),
                                serde_json::Value::String(metadata.provider.to_string()),
                            );
                            object.insert(
                                "provider_credential_id".to_string(),
                                serde_json::Value::String(metadata.provider_credential_id.clone()),
                            );
                            object.insert(
                                "model".to_string(),
                                serde_json::Value::String(metadata.model.clone()),
                            );
                        }
                        line = msg.to_string();
                    }
                }

                if msg.get("type").and_then(|v| v.as_str()) == Some("result") {
                    let is_success = msg.get("subtype").and_then(|v| v.as_str()) == Some("success");
                    if let Ok(mut guard) = result_success_stdout.lock() {
                        *guard = Some(is_success);
                    }
                    // The turn is over: end of file on stdin lets Claude exit.
                    close_stdin_after_emit = true;
                }
            }

            sink_stdout.emit_output(&tab_id_stdout, line);

            if close_stdin_after_emit {
                close_control(&controls_stdout, &process_key_stdout, run_id).await;
            }
        }
        close_control(&controls_stdout, &process_key_stdout, run_id).await;
        eprintln!(
            "[claude-stdout] [{}] stream ended after {} lines ({:.1}s)",
            tab_id_stdout,
            line_count,
            start_time.elapsed().as_secs_f64()
        );
    });

    let sink_stderr = sink.clone();
    let tab_id_stderr = tab_id.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = stderr_reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!(
                "[claude-stderr] [{}] +{:.1}s {}",
                tab_id_stderr,
                start_time.elapsed().as_secs_f64(),
                &line[..line.len().min(200)]
            );
            sink_stderr.emit_error(&tab_id_stderr, line);
        }
    });

    let process_arc_wait = process_arc.clone();
    let sink_wait = sink;
    let process_key_wait = process_key;
    let tab_id_wait = tab_id;
    let result_success_wait = result_success_holder.clone();
    tokio::spawn(async move {
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        let mut processes = process_arc_wait.lock().await;
        let success = if let Some(mut child) = processes.remove(&process_key_wait) {
            match child.wait().await {
                Ok(status) => {
                    let exit_success = status.success();
                    let result_success = result_success_wait.lock().ok().and_then(|guard| *guard);
                    let success = exit_success || result_success == Some(true);
                    eprintln!(
                        "[claude-process] [{}] exited with status={} result_success={:?} final_success={} ({:.1}s)",
                        tab_id_wait,
                        status,
                        result_success,
                        success,
                        start_time.elapsed().as_secs_f64()
                    );
                    success
                }
                Err(e) => {
                    eprintln!(
                        "[claude-process] [{}] wait error: {} ({:.1}s)",
                        tab_id_wait,
                        e,
                        start_time.elapsed().as_secs_f64()
                    );
                    false
                }
            }
        } else {
            eprintln!(
                "[claude-process] [{}] no child found in map ({:.1}s)",
                tab_id_wait,
                start_time.elapsed().as_secs_f64()
            );
            false
        };
        drop(processes);

        sink_wait.emit_complete(&tab_id_wait, success);
    });

    Ok(())
}

/// The Lab: answer a permission request with Allow or Deny. Returns false when
/// the request is no longer pending (already answered, withdrawn, or the run
/// has ended), so a late click is harmless.
pub async fn answer_claude_permission(
    window: WebviewWindow,
    tab_id: String,
    request_id: String,
    allow: bool,
) -> Result<bool, String> {
    let state = window.state::<ClaudeProcessState>();
    answer_permission_request(state.inner(), window.label(), &tab_id, &request_id, allow).await
}

pub(crate) async fn answer_permission_request(
    state: &ClaudeProcessState,
    window_label: &str,
    tab_id: &str,
    request_id: &str,
    allow: bool,
) -> Result<bool, String> {
    let key = process_key(window_label, tab_id);
    let mut controls = state.controls.lock().await;
    let Some(control) = controls.get_mut(&key) else {
        return Ok(false);
    };
    let Some(input) = control.pending.remove(request_id) else {
        return Ok(false);
    };
    let line = claude_control::permission_answer_line(request_id, allow, &input);
    write_line(&mut control.stdin, &line)
        .await
        .map_err(|e| format!("Failed to answer Claude's permission request: {}", e))?;
    eprintln!(
        "[claude-control] [{}] {} request {}",
        tab_id,
        if allow { "allowed" } else { "denied" },
        request_id
    );
    Ok(true)
}

pub async fn stop_claude_process(
    window: WebviewWindow,
    tab_id: String,
    mode: ClaudeStopMode,
) -> Result<bool, String> {
    let state = window.state::<ClaudeProcessState>().inner().clone();
    let window_label = window.label().to_string();
    stop_claude_process_with(window, &state, &window_label, tab_id, mode).await
}

pub(crate) async fn stop_claude_process_with<S: ClaudeEventSink>(
    sink: S,
    state: &ClaudeProcessState,
    window_label: &str,
    tab_id: String,
    mode: ClaudeStopMode,
) -> Result<bool, String> {
    let process_key = process_key(window_label, &tab_id);
    let mut processes = state.processes.lock().await;
    if let Some(mut child) = processes.remove(&process_key) {
        drop(processes);
        let stopped = match mode {
            ClaudeStopMode::Terminate => {
                terminate_process_tree(&mut child).await;
                true
            }
            ClaudeStopMode::Interrupt => interrupt_or_terminate(&mut child).await,
        };
        // Close stdin too: after an interrupt Claude would otherwise wait for
        // another stream-json message. Pending permission requests go with it.
        state.controls.lock().await.remove(&process_key);
        return Ok(stopped);
    }
    drop(processes);
    state.controls.lock().await.remove(&process_key);

    sink.emit_complete(&tab_id, false);
    Ok(false)
}

#[cfg(unix)]
async fn interrupt_or_terminate(child: &mut Child) -> bool {
    if let Some(pid) = child.id() {
        let status = tokio::process::Command::new("kill")
            .arg("-INT")
            .arg(pid.to_string())
            .status()
            .await;
        if matches!(status, Ok(status) if status.success()) {
            return true;
        }
    }
    terminate_process_tree(child).await;
    true
}

#[cfg(not(unix))]
async fn interrupt_or_terminate(child: &mut Child) -> bool {
    // Windows GUI processes do not have a reliable console-control path from
    // Tauri without a PTY/ConPTY session. For guided follow-ups, fall back to
    // terminating the current run so the frontend can immediately continue the
    // same tab with the queued guidance.
    terminate_process_tree(child).await;
    true
}

#[cfg(windows)]
async fn terminate_process_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = Command::new("taskkill")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .await;
    }
    let _ = child.start_kill();
}

#[cfg(not(windows))]
async fn terminate_process_tree(child: &mut Child) {
    let _ = child.start_kill();
}

/// Kill all Claude processes associated with a specific window label.
/// Called when a window is destroyed.
pub async fn kill_process_for_window(state: &ClaudeProcessState, window_label: &str) {
    let mut processes = state.processes.lock().await;
    let prefix = format!("{}:", window_label);
    let keys_to_remove: Vec<String> = processes
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys_to_remove {
        if let Some(mut child) = processes.remove(&key) {
            let _ = child.kill().await;
        }
    }
    drop(processes);
    state
        .controls
        .lock()
        .await
        .retain(|key, _| !key.starts_with(&prefix));
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::ClaudeEventSink;
    use tokio::sync::mpsc;

    #[derive(Debug, Clone, PartialEq)]
    pub enum TestEvent {
        Output(String),
        Error(String),
        Complete(bool),
    }

    /// Collects a run's events on a channel instead of a window.
    #[derive(Clone)]
    pub struct ChannelSink(pub mpsc::UnboundedSender<TestEvent>);

    impl ClaudeEventSink for ChannelSink {
        fn emit_output(&self, _tab_id: &str, data: String) {
            let _ = self.0.send(TestEvent::Output(data));
        }
        fn emit_error(&self, _tab_id: &str, data: String) {
            let _ = self.0.send(TestEvent::Error(data));
        }
        fn emit_complete(&self, _tab_id: &str, success: bool) {
            let _ = self.0.send(TestEvent::Complete(success));
        }
    }

    pub fn channel_sink() -> (ChannelSink, mpsc::UnboundedReceiver<TestEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (ChannelSink(tx), rx)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::test_support::{channel_sink, TestEvent};
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;

    /// A stand-in for `claude`: reads the prompt line, sends a control request
    /// The Lab does not support, then a permission request, and prints each
    /// answer it reads back. After the result it waits for end of file, so the
    /// run only finishes if the app closes stdin.
    const FAKE_CLAUDE: &str = r#"#!/bin/sh
IFS= read -r prompt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fake-session"}'
printf '%s\n' '{"type":"control_request","request_id":"req-other","request":{"subtype":"hook_callback","callback_id":"c1"}}'
IFS= read -r other
printf '{"type":"fake_echo","which":"other","answer":%s}\n' "$other"
printf '%s\n' '{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/p/hello.txt","content":"hi\n"},"description":"hello.txt","tool_use_id":"toolu_1"}}'
IFS= read -r answer
printf '{"type":"fake_echo","which":"permission","answer":%s,"prompt":%s}\n' "$answer" "$prompt"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
if IFS= read -r extra; then
  echo "stdin was left open: $extra" >&2
  exit 3
fi
exit 0
"#;

    /// Waits on a permission request forever, for the stop tests.
    const FAKE_CLAUDE_WAITS: &str = r#"#!/bin/sh
IFS= read -r prompt
printf '%s\n' '{"type":"control_request","request_id":"req-wait","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
exec sleep 30
"#;

    fn fake_command(dir: &tempfile::TempDir, script: &str) -> Command {
        let path = dir.path().join("claude");
        if let Err(e) = std::fs::write(&path, script) {
            panic!("write fake claude: {}", e);
        }
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(&path);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd
    }

    async fn next_event(rx: &mut UnboundedReceiver<TestEvent>) -> TestEvent {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the fake claude"),
        }
    }

    fn parse(line: &str) -> Value {
        serde_json::from_str(line).unwrap_or(Value::Null)
    }

    async fn run_fake_permission_flow(allow: bool) {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {}", e));
        let state = ClaudeProcessState::default();
        let (sink, mut rx) = channel_sink();
        let prompt_line = claude_control::user_message_line("Write hello.txt");

        spawn_claude_process_with(
            sink,
            state.clone(),
            "main",
            fake_command(&dir, FAKE_CLAUDE),
            "tab-1".to_string(),
            prompt_line.clone(),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("spawn: {}", e));

        let mut outputs = Vec::new();
        let mut answered = false;
        let complete = loop {
            match next_event(&mut rx).await {
                TestEvent::Output(line) => {
                    let msg = parse(&line);
                    if msg["type"] == "control_request" {
                        assert_eq!(msg["request_id"], "req-1", "only can_use_tool reaches the chat");
                        let first = answer_permission_request(&state, "main", "tab-1", "req-1", allow)
                            .await
                            .unwrap_or_else(|e| panic!("answer: {}", e));
                        assert!(first);
                        let again = answer_permission_request(&state, "main", "tab-1", "req-1", allow)
                            .await
                            .unwrap_or_else(|e| panic!("answer: {}", e));
                        assert!(!again, "a request is answered only once");
                        answered = true;
                    }
                    outputs.push(msg);
                }
                TestEvent::Error(line) => panic!("fake claude stderr: {}", line),
                TestEvent::Complete(success) => break success,
            }
        };

        assert!(answered);
        assert!(complete, "the run completes once stdin is closed");

        let other = outputs
            .iter()
            .find(|m| m["which"] == "other")
            .unwrap_or_else(|| panic!("no echo of the unsupported answer"));
        assert_eq!(
            other["answer"],
            parse(&claude_control::unsupported_line("req-other"))
        );

        let permission = outputs
            .iter()
            .find(|m| m["which"] == "permission")
            .unwrap_or_else(|| panic!("no echo of the permission answer"));
        let expected = if allow {
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "req-1",
                    "response": {
                        "behavior": "allow",
                        "updatedInput": { "file_path": "/p/hello.txt", "content": "hi\n" },
                    },
                },
            })
        } else {
            parse(&claude_control::deny_line("req-1"))
        };
        assert_eq!(permission["answer"], expected);
        assert_eq!(permission["prompt"], parse(&prompt_line));
        assert!(outputs.iter().any(|m| m["type"] == "result"));
        assert!(state.controls.lock().await.is_empty());
        assert!(state.processes.lock().await.is_empty());
    }

    #[tokio::test]
    async fn fake_claude_receives_allow_and_exits_after_result() {
        run_fake_permission_flow(true).await;
    }

    #[tokio::test]
    async fn fake_claude_receives_deny_and_exits_after_result() {
        run_fake_permission_flow(false).await;
    }

    async fn stop_while_waiting(mode: ClaudeStopMode) {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {}", e));
        let state = ClaudeProcessState::default();
        let (sink, mut rx) = channel_sink();

        spawn_claude_process_with(
            sink.clone(),
            state.clone(),
            "main",
            fake_command(&dir, FAKE_CLAUDE_WAITS),
            "tab-2".to_string(),
            claude_control::user_message_line("List files"),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("spawn: {}", e));

        match next_event(&mut rx).await {
            TestEvent::Output(line) => assert_eq!(parse(&line)["request_id"], "req-wait"),
            other => panic!("unexpected event {:?}", other),
        }

        let stopped = stop_claude_process_with(sink, &state, "main", "tab-2".to_string(), mode)
            .await
            .unwrap_or_else(|e| panic!("stop: {}", e));
        assert!(stopped);

        loop {
            match next_event(&mut rx).await {
                TestEvent::Complete(success) => {
                    assert!(!success);
                    break;
                }
                TestEvent::Output(_) | TestEvent::Error(_) => {}
            }
        }
        let late = answer_permission_request(&state, "main", "tab-2", "req-wait", true)
            .await
            .unwrap_or_else(|e| panic!("answer: {}", e));
        assert!(!late, "a request still pending when the run stops is dropped");
        assert!(state.controls.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stop_ends_a_run_waiting_on_permission() {
        stop_while_waiting(ClaudeStopMode::Terminate).await;
    }

    #[tokio::test]
    async fn interrupt_ends_a_run_waiting_on_permission() {
        stop_while_waiting(ClaudeStopMode::Interrupt).await;
    }
}
