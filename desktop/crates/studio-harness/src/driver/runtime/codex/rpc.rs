//! Lines the driver writes to `codex app-server` (newline-delimited JSON-RPC without the
//! `jsonrpc` member, as the server itself writes them) and parsing of its requests.

use crate::driver::{QuestionAnswer, QuestionItem, QuestionOption};
use serde_json::{Value, json};
use std::path::Path;

/// The JSON-RPC error code for a server request this client does not handle.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;

pub(crate) fn request(id: u64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

pub(crate) fn notification(method: &str) -> String {
    json!({"method": method}).to_string()
}

pub(crate) fn response(id: &Value, result: Value) -> String {
    json!({"id": id, "result": result}).to_string()
}

pub(crate) fn error_response(id: &Value, code: i64, message: &str) -> String {
    json!({"id": id, "error": {"code": code, "message": message}}).to_string()
}

pub(crate) fn initialize_params() -> Value {
    json!({
        "clientInfo": {
            "name": super::super::CLIENT_NAME,
            "title": "fframes studio",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {"experimentalApi": true},
    })
}

/// The user input of one turn or steer: the text, then each image file.
pub(crate) fn input(text: &str, images: &[&Path]) -> Value {
    let mut items = vec![json!({"type": "text", "text": text, "text_elements": []})];
    items.extend(
        images
            .iter()
            .map(|path| json!({"type": "localImage", "path": path})),
    );
    Value::Array(items)
}

/// The request a slash command maps to; other text, other slash commands included, is
/// sent as a prompt. Both run as a turn of the thread: `/compact` replies `{}` and
/// announces its turn with `turn/started`, `/review` replies with its turn.
pub(crate) fn slash_command(thread: &str, text: &str) -> Option<(&'static str, Value)> {
    let text = text.trim();
    if text == "/compact" {
        return Some(("thread/compact/start", json!({"threadId": thread})));
    }
    let rest = text.strip_prefix("/review")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let target = match rest.trim() {
        "" => json!({"type": "uncommittedChanges"}),
        instructions => json!({"type": "custom", "instructions": instructions}),
    };
    Some((
        "review/start",
        json!({"threadId": thread, "target": target, "delivery": "inline"}),
    ))
}

/// The answer to a command or file-change approval.
pub(crate) fn decision(accept: bool) -> Value {
    json!({"decision": if accept { "accept" } else { "decline" }})
}

/// The answer to an MCP tool-call approval (`mcpServer/elicitation/request` whose `_meta`
/// says `codex_approval_kind: mcp_tool_call`). Accepting sends no `_meta.persist`, which
/// Codex reads as this call only (codex-rs `core/src/mcp_tool_call.rs`,
/// `parse_mcp_tool_approval_elicitation_response`: `persist` `session` or `always` would
/// remember the approval). A decline is "user rejected MCP tool call".
pub(crate) fn tool_call_approval(accept: bool) -> Value {
    if accept {
        json!({"action": "accept", "content": {}})
    } else {
        json!({"action": "decline", "content": null})
    }
}

/// The answer to `item/permissions/requestApproval`: the requested profile for this turn,
/// or nothing granted.
pub(crate) fn permissions(granted: Option<&Value>) -> Value {
    match granted {
        Some(profile) => json!({"permissions": profile, "scope": "turn"}),
        None => json!({"permissions": {}}),
    }
}

/// One question of `item/tool/requestUserInput`, with its wire id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireQuestion {
    pub id: String,
    pub item: QuestionItem,
}

pub(crate) fn parse_questions(params: &Value) -> Vec<WireQuestion> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    params
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .enumerate()
                .map(|(index, question)| WireQuestion {
                    id: question
                        .get("id")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("q{index}"), str::to_owned),
                    item: QuestionItem {
                        header: text(question, "header"),
                        question: text(question, "question"),
                        multi_select: false,
                        options: question
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|options| {
                                options
                                    .iter()
                                    .map(|option| QuestionOption {
                                        label: text(option, "label"),
                                        description: text(option, "description"),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `{answers: {<id>: {answers: [...]}}}`: the chosen labels, then any free text.
pub(crate) fn answers(questions: &[WireQuestion], answers: &[QuestionAnswer]) -> Value {
    let map: serde_json::Map<String, Value> = questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| {
            let mut values = answer.selected.clone();
            values.extend(answer.text.iter().filter(|t| !t.is_empty()).cloned());
            (question.id.clone(), json!({"answers": values}))
        })
        .collect();
    json!({"answers": map})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_keep_their_ids_and_answers_go_back_under_them() {
        let params = json!({"questions": [
            {"id": "color", "header": "Color", "question": "Which?", "options": [{"label": "Red", "description": "warm"}]},
            {"header": "", "question": "Name?", "options": null, "isOther": true},
        ]});
        let questions = parse_questions(&params);
        assert_eq!(questions[0].id, "color");
        assert_eq!(questions[0].item.options[0].description, "warm");
        assert_eq!(questions[1].id, "q1");
        let body = answers(
            &questions,
            &[
                QuestionAnswer {
                    selected: vec!["Red".into()],
                    text: None,
                },
                QuestionAnswer {
                    selected: vec![],
                    text: Some("Ada".into()),
                },
            ],
        );
        assert_eq!(
            body,
            json!({"answers": {"color": {"answers": ["Red"]}, "q1": {"answers": ["Ada"]}}})
        );
    }

    #[test]
    fn compact_and_review_map_to_their_requests_and_other_text_is_a_prompt() {
        assert_eq!(
            slash_command("thr", " /compact "),
            Some(("thread/compact/start", json!({"threadId": "thr"})))
        );
        assert_eq!(
            slash_command("thr", "/review").unwrap().1["target"],
            json!({"type": "uncommittedChanges"})
        );
        assert_eq!(
            slash_command("thr", "/review the timing code").unwrap().1,
            json!({"threadId": "thr", "delivery": "inline",
                   "target": {"type": "custom", "instructions": "the timing code"}})
        );
        for text in [
            "/compact now",
            "/reviewer",
            "/init",
            "please /compact",
            "hi",
        ] {
            assert_eq!(slash_command("thr", text), None, "{text}");
        }
    }

    #[test]
    fn refusing_never_interrupts_the_turn() {
        assert_eq!(decision(false), json!({"decision": "decline"}));
        assert_eq!(decision(true), json!({"decision": "accept"}));
        assert_eq!(permissions(None), json!({"permissions": {}}));
    }
}
