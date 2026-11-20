// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! Claude Code's `--output-format stream-json`, one JSON object a line,
//! read into what the chat column shows.

use serde_json::Value;

/// One thing the chat column shows or acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    /// Words from Claude.
    Text(String),
    /// Claude used one of Concat's tools; `name` without `mcp__concat__`.
    Tool { name: String },
    /// The turn is over; the session to resume next time, when known.
    Done { session_id: Option<String> },
    /// The turn failed; what Claude Code said about it.
    Error(String),
}

/// The events in one line of the stream; none for a line that is not one.
pub fn parse_line(line: &str) -> Vec<ChatEvent> {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match event["type"].as_str() {
        Some("assistant") => {
            for block in event["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default();
                        if !text.trim().is_empty() {
                            out.push(ChatEvent::Text(text.to_owned()));
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default();
                        out.push(ChatEvent::Tool {
                            name: name
                                .strip_prefix("mcp__concat__")
                                .unwrap_or(name)
                                .to_owned(),
                        });
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            let subtype = event["subtype"].as_str().unwrap_or("success");
            if event["is_error"].as_bool() == Some(true) || subtype != "success" {
                let said = event["result"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty());
                out.push(ChatEvent::Error(said.unwrap_or(subtype).to_owned()));
            }
            out.push(ChatEvent::Done {
                session_id: event["session_id"].as_str().map(str::to_owned),
            });
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_tool_events() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Đã đổi."},{"type":"tool_use","id":"t1","name":"mcp__concat__title_add","input":{}}]}}"#;
        assert_eq!(
            parse_line(line),
            vec![
                ChatEvent::Text("Đã đổi.".into()),
                ChatEvent::Tool {
                    name: "title_add".into()
                }
            ]
        );
    }

    #[test]
    fn result_success_gives_done_with_session() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s-1"}"#;
        assert_eq!(
            parse_line(line),
            vec![ChatEvent::Done {
                session_id: Some("s-1".into())
            }]
        );
    }

    #[test]
    fn result_error_becomes_error_event() {
        let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login","session_id":"s-2"}"#;
        assert_eq!(
            parse_line(line),
            vec![
                ChatEvent::Error("Invalid API key · Please run /login".into()),
                ChatEvent::Done {
                    session_id: Some("s-2".into())
                }
            ]
        );
        let max =
            r#"{"type":"result","subtype":"error_max_turns","is_error":false,"session_id":"s-3"}"#;
        assert_eq!(
            parse_line(max)[0],
            ChatEvent::Error("error_max_turns".into())
        );
    }

    #[test]
    fn garbage_line_ignored() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line(r#"{"type":"system","subtype":"init","session_id":"x"}"#).is_empty());
        assert!(
            parse_line(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"  "}]}}"#
            )
            .is_empty()
        );
    }
}
