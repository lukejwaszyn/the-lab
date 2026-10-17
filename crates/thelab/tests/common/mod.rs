//! Helpers shared by the integration tests of the `thelab` binary.
#![allow(dead_code)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

pub const BINARY: &str = env!("CARGO_BIN_EXE_thelab");
pub const MODERN: &str = "2026-07-28";

/// The `crates` folder: the workspace these crates share inside The Lab.
pub fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn example_thesis() -> PathBuf {
    repository().join("examples/thesis")
}

pub fn fixture_project(name: &str) -> PathBuf {
    repository()
        .join("thelab-core/tests/fixtures/projects")
        .join(name)
}

/// A scratch directory that is removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "thelab-bin-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        // Canonical, so that paths compare equal to the ones the server reports.
        Self(fs::canonicalize(path).expect("canonical temp dir"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

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

/// Copies a project into a scratch directory, leaving out build output.
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
/// it is missing, unless THELAB_REQUIRE_TOOLS (set in CI) names it.
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

pub fn have_engine() -> bool {
    thelab_core::tools::find_program("tectonic").is_some() || have("latexmk")
}

/// Runs `thelab` with arguments and waits for it.
pub fn run_cli(arguments: &[&str], directory: &Path) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .current_dir(directory)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run thelab")
}

/// A running `thelab mcp` process and its pipes.
pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    /// Starts the server in `directory` with a private state directory.
    pub fn start(directory: &Path, state_dir: &Path, environment: &[(&str, &str)]) -> Self {
        Self::start_with(&[], directory, state_dir, environment)
    }

    /// Like `start`, with extra arguments after `thelab mcp`.
    pub fn start_with(
        arguments: &[&str],
        directory: &Path,
        state_dir: &Path,
        environment: &[(&str, &str)],
    ) -> Self {
        let mut command = Command::new(BINARY);
        command
            .arg("mcp")
            .args(arguments)
            .current_dir(directory)
            .env_remove("CLAUDE_PROJECT_DIR")
            .env("THELAB_STATE_DIR", state_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("start thelab mcp");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    pub fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("write request");
        stdin.flush().expect("flush");
    }

    /// Reads one line of stdout, which must be a JSON-RPC message.
    pub fn receive(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "the server closed stdout instead of answering");
        let message: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout carried something that is not JSON ({e}): {line}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
        message
    }

    /// Sends a request with no protocol metadata, as a legacy client does
    /// after `initialize`.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(&message);
        let response = self.receive();
        assert_eq!(response["id"], id, "responses are matched by id");
        response
    }

    /// Sends a request the modern way, with per-request metadata.
    pub fn modern(&mut self, method: &str, params: Value) -> Value {
        self.versioned(MODERN, method, params)
    }

    pub fn versioned(&mut self, version: &str, method: &str, mut params: Value) -> Value {
        if params.is_null() {
            params = json!({});
        }
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": { "name": "thelab-tests", "version": "0" },
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        self.request(method, params)
    }

    /// Calls a tool the modern way and returns the result object.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let response = self.modern(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        assert!(response.get("error").is_none(), "{tool}: {response}");
        assert_eq!(response["result"]["resultType"], "complete", "{tool}");
        response["result"].clone()
    }

    /// Closes stdin and checks that the server exits cleanly, having written
    /// nothing more to stdout.
    pub fn finish(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("wait for thelab mcp");
        assert!(status.success(), "thelab mcp exited with {status}");
        let mut rest = String::new();
        self.stdout
            .read_to_string(&mut rest)
            .expect("read rest of stdout");
        assert_eq!(rest, "", "unexpected trailing output on stdout");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text of a tool result's first text block.
pub fn text_of(result: &Value) -> &str {
    result["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|block| block["text"].as_str())
        .unwrap_or_else(|| panic!("no text block in {result}"))
}

pub fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes().filter(|&b| b != b'=') {
        let value = ALPHABET
            .iter()
            .position(|&a| a == byte)
            .expect("base64 character") as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}
