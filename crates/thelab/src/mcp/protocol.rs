//! MCP over stdio, for both protocol eras.
//!
//! Modern (2026-07-28 and later): there is no handshake. Every request
//! carries its protocol version in `params._meta`, `server/discover`
//! describes the server, and every result has a `resultType`.
//!
//! Legacy (2025-11-25 and earlier): the client opens with `initialize`,
//! and from then on requests carry no version.
//!
//! A request with neither a version nor a preceding `initialize` is served
//! anyway, as modern, which keeps manual testing simple.
//!
//! stdout carries protocol messages only. Anything else goes to stderr.

use std::io::{BufRead, Write};

use serde_json::{Map, Value, json};

use super::tools::{self, Group, Groups};

const SERVER_NAME: &str = "thelab";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Newest first.
const SUPPORTED_VERSIONS: [&str; 4] = ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"];
/// Versions that begin with `initialize`.
const LEGACY_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// The tool list only changes with the binary, so clients may cache it.
const CACHE_TTL_MS: u64 = 3_600_000;

/// Reads newline-delimited JSON-RPC from `input` until it closes, writing
/// one response line per request to `output`.
pub fn serve(input: impl BufRead, mut output: impl Write, groups: Groups) -> std::io::Result<()> {
    let mut server = Server::new(groups);
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line) {
            // serde_json never emits a raw newline, so one message is one line.
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

struct Server {
    /// Set by `initialize`: the client speaks a legacy version.
    initialized: bool,
    /// The tool groups this server was started with.
    groups: Groups,
    instructions: String,
}

impl Server {
    fn new(groups: Groups) -> Self {
        let instructions = tools::instructions(&groups);
        Self {
            initialized: false,
            groups,
            instructions,
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new(Groups::all())
    }
}

/// A JSON-RPC error to send back.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl Server {
    /// Handles one incoming line. Returns the response, or `None` when
    /// nothing is owed (notifications, stray responses).
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("thelab mcp: unreadable message: {error}");
                return Some(error_response(
                    Value::Null,
                    RpcError::new(PARSE_ERROR, "Parse error"),
                ));
            }
        };
        let Some(fields) = message.as_object() else {
            // Batches were removed from MCP in 2025-06-18.
            return Some(error_response(
                Value::Null,
                RpcError::new(INVALID_REQUEST, "Expected a single JSON-RPC message"),
            ));
        };
        let id = fields.get("id").cloned();
        let Some(method) = fields.get("method").and_then(Value::as_str) else {
            // A response to something this server never asked, or junk.
            return match id {
                Some(id) if !fields.contains_key("result") && !fields.contains_key("error") => {
                    Some(error_response(
                        id,
                        RpcError::new(INVALID_REQUEST, "Missing method"),
                    ))
                }
                _ => None,
            };
        };
        // Notifications (`notifications/initialized`, `notifications/cancelled`)
        // get no reply. Tool calls run to completion, so there is nothing to cancel.
        let id = id?;

        let params = fields.get("params").unwrap_or(&Value::Null);
        Some(match self.handle_request(method, params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => error_response(id, error),
        })
    }

    fn handle_request(&mut self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let version = params
            .get("_meta")
            .and_then(|meta| meta.get(META_PROTOCOL_VERSION));
        if let Some(requested) = version {
            let supported = requested
                .as_str()
                .is_some_and(|v| SUPPORTED_VERSIONS.contains(&v));
            if !supported {
                return Err(RpcError {
                    code: UNSUPPORTED_PROTOCOL_VERSION,
                    message: "Unsupported protocol version".to_string(),
                    data: Some(json!({ "supported": SUPPORTED_VERSIONS, "requested": requested })),
                });
            }
        }
        // A dual-era server picks its behavior from how the client opened:
        // per-request metadata means modern, `initialize` means legacy.
        let modern = version.is_some() || !self.initialized;

        let mut result = match method {
            "server/discover" => {
                return Ok(modern_result(json!({
                    "supportedVersions": SUPPORTED_VERSIONS,
                    "capabilities": { "tools": {} },
                    "instructions": self.instructions,
                    "ttlMs": CACHE_TTL_MS,
                    "cacheScope": "public",
                })));
            }
            "initialize" => {
                self.initialized = true;
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let version = requested
                    .filter(|v| LEGACY_VERSIONS.contains(v))
                    .unwrap_or(LEGACY_VERSIONS[0]);
                return Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                    "instructions": self.instructions,
                }));
            }
            // Removed in 2026-07-28, but harmless to answer.
            "ping" => json!({}),
            "tools/list" => {
                let mut result = json!({ "tools": tools::definitions(&self.groups) });
                if modern {
                    result["ttlMs"] = json!(CACHE_TTL_MS);
                    result["cacheScope"] = json!("public");
                }
                result
            }
            "tools/call" => call_tool(params, &self.groups)?,
            other => {
                return Err(RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("Method not found: {other}"),
                ));
            }
        };
        if modern {
            result = modern_result(result);
        }
        Ok(result)
    }
}

fn call_tool(params: &Value, groups: &Groups) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call needs a tool name"))?;
    let no_arguments = Value::Object(Map::new());
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &no_arguments,
        Some(arguments) if arguments.is_object() => arguments,
        Some(_) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "Tool arguments must be an object",
            ));
        }
    };

    // A tool outside the groups this server was started with does not exist
    // as far as the client is concerned, but say why.
    if let Some(group) = Group::of_tool(name).filter(|group| !groups.contains(*group)) {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "Unknown tool: {name} (the {} tools are turned off by --tools)",
                group.name()
            ),
        ));
    }

    // A bug in one tool should not take the server down with it.
    let outcome = std::panic::catch_unwind(|| tools::call(name, arguments));
    let output = match outcome {
        Ok(Some(output)) => output,
        Ok(None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("Unknown tool: {name}"),
            ));
        }
        Err(_) => tools::Output::error(format!(
            "{name} failed unexpectedly. This is a bug in thelab."
        )),
    };

    let mut result = json!({ "content": output.content, "isError": output.is_error });
    if let Some(structured) = output.structured {
        result["structuredContent"] = structured;
    }
    Ok(result)
}

/// Adds the fields every modern result carries.
fn modern_result(mut result: Value) -> Value {
    result["resultType"] = json!("complete");
    result["_meta"] =
        json!({ META_SERVER_INFO: { "name": SERVER_NAME, "version": SERVER_VERSION } });
    result
}

fn error_response(id: Value, error: RpcError) -> Value {
    let mut body = json!({ "code": error.code, "message": error.message });
    if let Some(data) = error.data {
        body["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(server: &mut Server, message: Value) -> Value {
        server
            .handle_line(&message.to_string())
            .expect("a response")
    }

    #[test]
    fn instructions_fit_within_what_claude_code_keeps() {
        let instructions = tools::instructions(&Groups::all());
        assert!(
            instructions.len() <= 2048,
            "{} characters",
            instructions.len()
        );
        for tool in tools::definitions(&Groups::all()).as_array().unwrap() {
            let description = tool["description"].as_str().unwrap();
            assert!(
                description.len() <= 2048,
                "{}: {} characters",
                tool["name"],
                description.len()
            );
        }
    }

    #[test]
    fn notifications_and_stray_responses_get_no_reply() {
        let mut server = Server::default();
        for message in [
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } }),
            json!({ "jsonrpc": "2.0", "id": 9, "result": {} }),
        ] {
            assert_eq!(server.handle_line(&message.to_string()), None, "{message}");
        }
    }

    #[test]
    fn malformed_input_gets_a_json_rpc_error() {
        let mut server = Server::default();
        let parse = server.handle_line("{not json").unwrap();
        assert_eq!(parse["error"]["code"], PARSE_ERROR);
        assert_eq!(parse["id"], Value::Null);

        let batch = server.handle_line("[]").unwrap();
        assert_eq!(batch["error"]["code"], INVALID_REQUEST);

        let unknown = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
        );
        assert_eq!(unknown["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(unknown["id"], 1);
    }

    #[test]
    fn modern_results_carry_result_type_and_server_info() {
        let mut server = Server::default();
        let meta = json!({ META_PROTOCOL_VERSION: "2026-07-28" });
        let list = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list", "params": { "_meta": meta } }),
        );
        let result = &list["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["cacheScope"], "public");
        assert!(result["ttlMs"].as_u64().unwrap() > 0);
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], "thelab");
    }

    #[test]
    fn legacy_results_stay_in_their_own_dialect() {
        let mut server = Server::default();
        let init = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                                "clientInfo": { "name": "t", "version": "0" } } }),
        );
        assert_eq!(
            init["result"]["protocolVersion"], "2025-06-18",
            "echoes a supported version"
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "thelab");
        assert!(init["result"].get("resultType").is_none());

        let list = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        );
        assert!(list["result"]["tools"].is_array());
        assert!(list["result"].get("resultType").is_none());
        assert!(list["result"].get("ttlMs").is_none());

        // The same process can still serve a modern client.
        let meta = json!({ META_PROTOCOL_VERSION: "2026-07-28" });
        let modern = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": { "_meta": meta } }),
        );
        assert_eq!(modern["result"]["resultType"], "complete");
    }

    #[test]
    fn initialize_with_an_unknown_version_offers_the_newest_legacy_one() {
        for requested in ["2024-11-05", "2026-07-28", "1999-01-01"] {
            let mut server = Server::default();
            let init = request(
                &mut server,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": { "protocolVersion": requested } }),
            );
            assert_eq!(
                init["result"]["protocolVersion"], "2025-11-25",
                "{requested}"
            );
        }
    }

    #[test]
    fn unsupported_version_is_error_32022_on_any_method() {
        let mut server = Server::default();
        for method in ["server/discover", "tools/list", "tools/call"] {
            let meta = json!({ META_PROTOCOL_VERSION: "1900-01-01" });
            let response = request(
                &mut server,
                json!({ "jsonrpc": "2.0", "id": 5, "method": method, "params": { "_meta": meta } }),
            );
            assert_eq!(
                response["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION,
                "{method}"
            );
            assert_eq!(response["error"]["data"]["requested"], "1900-01-01");
            assert_eq!(
                response["error"]["data"]["supported"],
                json!(SUPPORTED_VERSIONS)
            );
        }
    }

    #[test]
    fn unknown_tool_is_a_protocol_error_and_bad_arguments_are_a_tool_error() {
        let mut server = Server::default();
        let unknown = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "nope" } }),
        );
        assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
        assert_eq!(unknown["error"]["message"], "Unknown tool: nope");

        // The model can fix a bad argument, so that comes back as a result.
        let bad = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "find_citation", "arguments": {} } }),
        );
        assert_eq!(bad["result"]["isError"], true);
        assert!(
            bad["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("query")
        );
    }

    #[test]
    fn a_server_started_with_some_groups_offers_and_runs_only_those() {
        let mut server = Server::new(Groups::parse("latex").unwrap());
        let list = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "project_overview",
                "compile",
                "check_references",
                "find_citation",
                "render_page"
            ]
        );

        let off = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "editor_context" } }),
        );
        assert_eq!(off["error"]["code"], INVALID_PARAMS);
        let message = off["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("editor tools are turned off by --tools"),
            "{message}"
        );

        let discover = request(
            &mut server,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "server/discover" }),
        );
        let instructions = discover["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("compile") && !instructions.contains("editor_context"));
    }
}
