//! Normalized run events — the one shape the UI consumes regardless
//! of which harness produced them.
//!
//! Every adapter (bob's stream-json, Claude Code's stream-json,
//! Codex's format, a raw-API agent loop) parses its own wire format
//! into these variants *on the Rust side*. The front-end then learns
//! exactly one event vocabulary and never grows a per-harness
//! parser. This is the keystone of the harness abstraction: the cost
//! of adding a harness is "write a parser into `RunEvent`," not
//! "teach the UI another format."
//!
//! Suggested edits carry only the *raw* edit (path + byte range +
//! replacement). Turning those into previewable drafts needs the
//! workspace file content and the coordinate mapper, which live in
//! the consuming app layer, so that step stays there — this module's
//! job is just to lift the edit out of the harness's bespoke wire
//! format.

use serde::Serialize;

use cli_stream::ProcessEvent;

/// A UTF-8 byte range into a document. Mirrors the persisted
/// `ByteOffset` discipline (see `docs/editor-guide.md`): positions
/// crossing the harness boundary are bytes, never code units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// A raw suggested edit emitted by a harness. The app layer prepares
/// these into previewable drafts; this is the transport shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedEdit {
    pub file_path: String,
    pub range: ByteRange,
    pub replacement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A tool call beginning — its id + name, so the UI can render a
/// state-ful card (running → done/✗) keyed by `tool_call_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallStart {
    pub tool_call_id: String,
    pub name: String,
}

/// A tool call finishing — matched to its start by `tool_call_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEnd {
    pub tool_call_id: String,
    pub ok: bool,
}

/// The normalized event stream. `#[serde(tag = "kind")]` +
/// camelCase mirrors the existing `ProcessEvent` wire contract the TS
/// store already reads (`event.kind`, `event.runId`, …), so the
/// front-end consumes one shape regardless of which harness produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
// `rename_all` camelCases the variant tags ("suggestedEdits"); serde
// does NOT cascade that to struct-variant fields, so `rename_all_fields`
// is required to get `runId` / `exitCode` on the wire rather than the
// snake_case Rust idents.
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RunEvent {
    /// First event, before any output. UI shows "thinking…".
    Started { run_id: String },
    /// A chunk of assistant text. Appended to the active message.
    Text { run_id: String, delta: String },
    /// A chunk of model reasoning ("thinking"), rendered distinctly from
    /// `Text` so the UI can show reasoning without mixing it into the
    /// answer (e.g. Claude's `thinking_delta`).
    Thinking { run_id: String, delta: String },
    /// A tool call started — render a state-ful card keyed by id.
    ToolStart {
        run_id: String,
        tool_call_id: String,
        name: String,
    },
    /// A tool call finished (matched to its start by id).
    ToolEnd {
        run_id: String,
        tool_call_id: String,
        ok: bool,
    },
    /// One or more proposed edits. The app prepares + previews them.
    SuggestedEdits {
        run_id: String,
        edits: Vec<SuggestedEdit>,
    },
    /// A human-readable status line (tool call, file touch, edit
    /// count). Replaces the message's transient activity text.
    Activity { run_id: String, message: String },
    /// Spawn / IO / parse failure. Terminal — followed by `Exited`.
    Error { run_id: String, message: String },
    /// The run finished. Sent exactly once.
    Exited {
        run_id: String,
        exit_code: Option<i32>,
        cancelled: bool,
    },
}

/// What a single harness output line decoded to. A line can yield
/// text *and* edits at once, so this is not one-event-per-line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub text: Option<String>,
    /// Model reasoning chunk → `RunEvent::Thinking`. Kept separate from
    /// `text` so the UI can render it distinctly.
    pub thinking: Option<String>,
    /// A tool call began → `RunEvent::ToolStart`.
    pub tool_start: Option<ToolCallStart>,
    /// A tool call finished → `RunEvent::ToolEnd`.
    pub tool_end: Option<ToolCallEnd>,
    pub edits: Vec<SuggestedEdit>,
    pub activity: Option<String>,
}

impl ParsedLine {
    /// True when a line decoded to no actionable content. A useful
    /// predicate for adapters + their tests; the normalize skeleton
    /// relies instead on the natural no-op of pushing zero events.
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.thinking.is_none()
            && self.tool_start.is_none()
            && self.tool_end.is_none()
            && self.edits.is_empty()
            && self.activity.is_none()
    }
}

/// Translate one raw process event into zero or more normalized
/// [`RunEvent`]s, using `parse_line` to decode the harness's stdout
/// wire format. Lifecycle events (Started / Exited / Error) and
/// stderr are harness-neutral and handled here; only the stdout
/// parsing differs per harness — so every process-backed adapter
/// shares this skeleton and supplies just its own line parser.
pub fn normalize_process_event(
    event: ProcessEvent,
    mut parse_line: impl FnMut(&str) -> ParsedLine,
) -> Vec<RunEvent> {
    match event {
        ProcessEvent::Started { run_id } => vec![RunEvent::Started { run_id }],
        ProcessEvent::Exited {
            run_id,
            exit_code,
            cancelled,
        } => vec![RunEvent::Exited {
            run_id,
            exit_code,
            cancelled,
        }],
        ProcessEvent::Error { run_id, message } => vec![RunEvent::Error { run_id, message }],
        ProcessEvent::Stderr { run_id, line } => {
            // stderr is warnings/progress; surface as activity,
            // truncated like the TS store did (240 chars).
            let message = truncate(&line, 240);
            if message.is_empty() {
                vec![]
            } else {
                vec![RunEvent::Activity { run_id, message }]
            }
        }
        ProcessEvent::Stdout { run_id, line } => {
            let parsed = parse_line(&line);
            let mut out = Vec::new();
            if let Some(text) = parsed.text {
                out.push(RunEvent::Text {
                    run_id: run_id.clone(),
                    delta: text,
                });
            }
            if let Some(thinking) = parsed.thinking {
                out.push(RunEvent::Thinking {
                    run_id: run_id.clone(),
                    delta: thinking,
                });
            }
            if let Some(start) = parsed.tool_start {
                out.push(RunEvent::ToolStart {
                    run_id: run_id.clone(),
                    tool_call_id: start.tool_call_id,
                    name: start.name,
                });
            }
            if let Some(end) = parsed.tool_end {
                out.push(RunEvent::ToolEnd {
                    run_id: run_id.clone(),
                    tool_call_id: end.tool_call_id,
                    ok: end.ok,
                });
            }
            if !parsed.edits.is_empty() {
                out.push(RunEvent::SuggestedEdits {
                    run_id: run_id.clone(),
                    edits: parsed.edits,
                });
            }
            if let Some(activity) = parsed.activity {
                out.push(RunEvent::Activity {
                    run_id,
                    message: activity,
                });
            }
            out
        }
    }
}

/// Take the first `max_chars` characters (not bytes) of `s`. Bounds the
/// stderr activity line without splitting a multi-byte char.
fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line parser that yields nothing — exercises the neutral
    /// skeleton without any harness-specific decoding.
    fn empty_parser(_: &str) -> ParsedLine {
        ParsedLine::default()
    }

    #[test]
    fn normalize_passes_through_lifecycle_events() {
        assert!(matches!(
            normalize_process_event(ProcessEvent::Started { run_id: "r".into() }, empty_parser)
                .as_slice(),
            [RunEvent::Started { .. }]
        ));
        assert!(matches!(
            normalize_process_event(
                ProcessEvent::Exited {
                    run_id: "r".into(),
                    exit_code: Some(0),
                    cancelled: false
                },
                empty_parser
            )
            .as_slice(),
            [RunEvent::Exited { exit_code: Some(0), cancelled: false, .. }]
        ));
    }

    #[test]
    fn stderr_becomes_truncated_activity() {
        let long = "x".repeat(500);
        let events = normalize_process_event(
            ProcessEvent::Stderr {
                run_id: "r1".into(),
                line: long,
            },
            empty_parser,
        );
        match events.as_slice() {
            [RunEvent::Activity { run_id, message }] => {
                assert_eq!(run_id, "r1");
                assert_eq!(message.chars().count(), 240);
            }
            other => panic!("expected one Activity, got {other:?}"),
        }
        // Empty stderr line → no event.
        assert!(normalize_process_event(
            ProcessEvent::Stderr {
                run_id: "r1".into(),
                line: String::new(),
            },
            empty_parser,
        )
        .is_empty());
    }

    #[test]
    fn thinking_normalizes_and_serializes() {
        let events = normalize_process_event(
            ProcessEvent::Stdout {
                run_id: "r1".to_owned(),
                line: "ignored".to_owned(),
            },
            |_| ParsedLine {
                thinking: Some("pondering".to_owned()),
                ..ParsedLine::default()
            },
        );
        assert!(matches!(
            events.as_slice(),
            [RunEvent::Thinking { run_id, delta }] if run_id == "r1" && delta == "pondering"
        ));
        let json = serde_json::to_value(RunEvent::Thinking {
            run_id: "r1".to_owned(),
            delta: "d".to_owned(),
        })
        .unwrap();
        assert_eq!(json["kind"], "thinking");
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["delta"], "d");
    }

    #[test]
    fn run_event_serializes_with_kind_and_camelcase() {
        let json = serde_json::to_value(RunEvent::Exited {
            run_id: "r1".to_owned(),
            exit_code: Some(2),
            cancelled: true,
        })
        .unwrap();
        assert_eq!(json["kind"], "exited");
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["exitCode"], 2);
        assert_eq!(json["cancelled"], true);
    }

    #[test]
    fn suggested_edits_event_serializes_camelcase() {
        let json = serde_json::to_value(RunEvent::SuggestedEdits {
            run_id: "r1".to_owned(),
            edits: vec![SuggestedEdit {
                file_path: "a.md".to_owned(),
                range: ByteRange { start: 1, end: 2 },
                replacement: "x".to_owned(),
                title: None,
            }],
        })
        .unwrap();
        assert_eq!(json["kind"], "suggestedEdits");
        assert_eq!(json["edits"][0]["filePath"], "a.md");
        assert_eq!(json["edits"][0]["range"]["start"], 1);
        // title omitted when None
        assert!(json["edits"][0].get("title").is_none());
    }
}
