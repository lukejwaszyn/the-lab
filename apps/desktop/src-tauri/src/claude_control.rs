//! The Lab: the stream-json control protocol between the app and Claude Code.
//!
//! Claude runs with `--input-format stream-json --permission-prompt-tool stdio`.
//! The prompt goes in as one JSON line on stdin, and stdin stays open so the
//! app can answer the `control_request` lines Claude prints when a tool needs
//! the user's permission. Everything here builds or reads single lines; the
//! process plumbing lives in `claude_process.rs`.

use serde_json::{json, Map, Value};

/// What Claude is told when the user presses Deny.
pub const DENY_MESSAGE: &str = "The user declined.";

/// The error sent back for control requests The Lab does not handle, so the
/// CLI never waits on an answer that will not come.
pub const UNSUPPORTED_MESSAGE: &str = "not supported by The Lab";

/// A control message from Claude's stdout that the app has to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlMessage {
    /// `can_use_tool`: a tool needs permission. The input is kept so an allow
    /// can hand it back unchanged as `updatedInput`.
    PermissionRequest { request_id: String, input: Value },
    /// Any other control request subtype; answered with an error at once.
    UnsupportedRequest { request_id: String, subtype: String },
    /// Claude withdrew a request it had asked earlier.
    Cancel { request_id: String },
}

/// The prompt as one stream-json line, without the trailing newline.
pub fn user_message_line(prompt: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": prompt },
    })
    .to_string()
}

/// Allow a tool call, passing the input Claude asked for back unchanged.
pub fn allow_line(request_id: &str, input: &Value) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "allow", "updatedInput": input },
        },
    })
    .to_string()
}

/// Deny a tool call; Claude sees `DENY_MESSAGE` as the tool's error.
pub fn deny_line(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "deny", "message": DENY_MESSAGE },
        },
    })
    .to_string()
}

/// Refuse a control request subtype The Lab does not support.
pub fn unsupported_line(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": UNSUPPORTED_MESSAGE,
        },
    })
    .to_string()
}

/// The answer line for a permission request.
pub fn permission_answer_line(request_id: &str, allow: bool, input: &Value) -> String {
    if allow {
        allow_line(request_id, input)
    } else {
        deny_line(request_id)
    }
}

/// Recognise the control messages in one parsed stdout line. Ordinary stream
/// messages (system, assistant, user, result) return `None`.
pub fn classify(msg: &Value) -> Option<ControlMessage> {
    match msg.get("type").and_then(Value::as_str)? {
        "control_request" => {
            let request_id = msg.get("request_id").and_then(Value::as_str)?.to_string();
            let request = msg.get("request");
            let subtype = request
                .and_then(|r| r.get("subtype"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if subtype == "can_use_tool" {
                let input = request
                    .and_then(|r| r.get("input"))
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| Value::Object(Map::new()));
                Some(ControlMessage::PermissionRequest { request_id, input })
            } else {
                Some(ControlMessage::UnsupportedRequest {
                    request_id,
                    subtype,
                })
            }
        }
        "control_cancel_request" => {
            let request_id = msg.get("request_id").and_then(Value::as_str)?.to_string();
            Some(ControlMessage::Cancel { request_id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Value {
        serde_json::from_str(line).unwrap_or(Value::Null)
    }

    #[test]
    fn user_message_line_is_one_json_line() {
        let line = user_message_line("Fix \"this\"\nplease");
        assert!(!line.contains('\n'));
        assert_eq!(
            parse(&line),
            json!({
                "type": "user",
                "message": { "role": "user", "content": "Fix \"this\"\nplease" },
            })
        );
    }

    #[test]
    fn allow_line_returns_the_input_unchanged() {
        let input = json!({ "file_path": "/p/hello.txt", "content": "hi\n" });
        let line = allow_line("aeac4028", &input);
        assert!(!line.contains('\n'));
        assert_eq!(
            parse(&line),
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "aeac4028",
                    "response": { "behavior": "allow", "updatedInput": input },
                },
            })
        );
    }

    #[test]
    fn deny_line_says_the_user_declined() {
        assert_eq!(
            parse(&deny_line("r1")),
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "r1",
                    "response": { "behavior": "deny", "message": "The user declined." },
                },
            })
        );
    }

    #[test]
    fn unsupported_line_is_an_error_response() {
        assert_eq!(
            parse(&unsupported_line("r2")),
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": "r2",
                    "error": "not supported by The Lab",
                },
            })
        );
    }

    #[test]
    fn permission_answer_line_picks_allow_or_deny() {
        let input = json!({ "command": "ls" });
        assert_eq!(
            permission_answer_line("a", true, &input),
            allow_line("a", &input)
        );
        assert_eq!(permission_answer_line("a", false, &input), deny_line("a"));
    }

    #[test]
    fn classify_recognises_can_use_tool() {
        let msg = parse(
            r#"{"type":"control_request","request_id":"aeac4028-1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/x/hello.txt","content":"hi\n"},"description":"hello.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_1"}}"#,
        );
        assert_eq!(
            classify(&msg),
            Some(ControlMessage::PermissionRequest {
                request_id: "aeac4028-1".into(),
                input: json!({ "file_path": "/x/hello.txt", "content": "hi\n" }),
            })
        );
    }

    #[test]
    fn classify_gives_missing_input_an_empty_object() {
        let msg = parse(
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#,
        );
        assert_eq!(
            classify(&msg),
            Some(ControlMessage::PermissionRequest {
                request_id: "r".into(),
                input: json!({}),
            })
        );
    }

    #[test]
    fn classify_marks_other_subtypes_unsupported() {
        let msg = parse(
            r#"{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"c"}}"#,
        );
        assert_eq!(
            classify(&msg),
            Some(ControlMessage::UnsupportedRequest {
                request_id: "h1".into(),
                subtype: "hook_callback".into(),
            })
        );
    }

    #[test]
    fn classify_recognises_cancel() {
        let msg = parse(r#"{"type":"control_cancel_request","request_id":"c1"}"#);
        assert_eq!(
            classify(&msg),
            Some(ControlMessage::Cancel {
                request_id: "c1".into()
            })
        );
    }

    #[test]
    fn classify_ignores_stream_messages_and_requests_without_ids() {
        for line in [
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"assistant","message":{"content":[]}}"#,
            r#"{"type":"result","subtype":"success"}"#,
            r#"{"type":"control_request","request":{"subtype":"can_use_tool"}}"#,
            r#"{"type":"control_response","response":{}}"#,
        ] {
            assert_eq!(classify(&parse(line)), None, "{}", line);
        }
    }
}
