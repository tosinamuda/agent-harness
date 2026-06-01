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
/// state-ful card (running → done/✗) keyed by `tool_call_id`. `input`
/// carries the call's arguments when the harness delivers them inline
/// at the start (bob's `parameters`, codex's `command`); it is `None`
/// when the harness streams them incrementally (Claude's
/// `input_json_delta`), so the card stays correct either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallStart {
    pub tool_call_id: String,
    pub name: String,
    pub input: Option<String>,
}

/// A tool call finishing — matched to its start by `tool_call_id`.
/// `output` carries the tool's result when the harness reports it
/// inline at completion (bob's `tool_result.output`, codex's
/// `aggregated_output`, Claude's `tool_result.content`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEnd {
    pub tool_call_id: String,
    pub ok: bool,
    pub output: Option<String>,
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
// New event kinds (a richer Usage, a new lifecycle signal, …) can be added
// without breaking consumers — they must carry a `_` arm. Adding `Session` /
// `Usage` earlier was a breaking change precisely because this was missing.
#[non_exhaustive]
pub enum RunEvent {
    /// First event, before any output. UI shows "thinking…". Fired the
    /// instant the process spawns — *before* the CLI reports its
    /// session/model, which arrive separately as [`RunEvent::Session`].
    Started { run_id: String },
    /// The agent session is established — its id and the model in use.
    /// Distinct from `Started` because it arrives a beat later, in the
    /// CLI's first output line (bob's `init`, Claude's `system/init`,
    /// codex's `thread.started`); keeping `Started` instant matters for
    /// the "thinking…" feedback. Either field may be absent when the CLI
    /// doesn't report it (e.g. codex gives a thread id but no model).
    Session {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A chunk of assistant text. Appended to the active message.
    Text { run_id: String, delta: String },
    /// A chunk of model reasoning ("thinking"), rendered distinctly from
    /// `Text` so the UI can show reasoning without mixing it into the
    /// answer (e.g. Claude's `thinking_delta`).
    Thinking { run_id: String, delta: String },
    /// A tool call started — render a state-ful card keyed by id.
    /// `input` is the call's arguments when delivered inline (omitted
    /// from the wire when absent, e.g. Claude streams them separately).
    ToolStart {
        run_id: String,
        tool_call_id: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<String>,
    },
    /// A tool call finished (matched to its start by id). `output` is the
    /// tool's result when the harness reports it inline (omitted when absent).
    ToolEnd {
        run_id: String,
        tool_call_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// One or more proposed edits. The app prepares + previews them.
    SuggestedEdits {
        run_id: String,
        edits: Vec<SuggestedEdit>,
    },
    /// A human-readable status line (tool call, file touch, edit
    /// count). Replaces the message's transient activity text.
    Activity { run_id: String, message: String },
    /// Token accounting for the run, emitted near its end (from the
    /// CLI's `result` / `turn.completed`). Neutral tokens only —
    /// harness-specific costs/credits (bob's coins) are NOT here; a
    /// consumer that wants them reads the harness's own output. Any
    /// field may be absent when the CLI doesn't break usage down.
    Usage {
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total_tokens: Option<u64>,
    },
    /// Spawn / IO / parse failure. Terminal — followed by `Exited`.
    Error { run_id: String, message: String },
    /// The run finished. Sent exactly once.
    Exited {
        run_id: String,
        exit_code: Option<i32>,
        cancelled: bool,
    },
}

/// Session identity decoded from a harness's init line → `RunEvent::Session`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: Option<String>,
    pub model: Option<String>,
}

/// Token accounting decoded from a harness's result line → `RunEvent::Usage`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UsageInfo {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// What a single harness output line decoded to. A line can yield
/// text *and* edits at once, so this is not one-event-per-line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub text: Option<String>,
    /// Model reasoning chunk → `RunEvent::Thinking`. Kept separate from
    /// `text` so the UI can render it distinctly.
    pub thinking: Option<String>,
    /// Session identity (id + model) → `RunEvent::Session`.
    pub session: Option<SessionInfo>,
    /// A tool call began → `RunEvent::ToolStart`.
    pub tool_start: Option<ToolCallStart>,
    /// A tool call finished → `RunEvent::ToolEnd`.
    pub tool_end: Option<ToolCallEnd>,
    pub edits: Vec<SuggestedEdit>,
    /// Token accounting → `RunEvent::Usage`.
    pub usage: Option<UsageInfo>,
    pub activity: Option<String>,
}

impl ParsedLine {
    /// True when a line decoded to no actionable content. A useful
    /// predicate for adapters + their tests; the normalize skeleton
    /// relies instead on the natural no-op of pushing zero events.
    pub fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.thinking.is_none()
            && self.session.is_none()
            && self.tool_start.is_none()
            && self.tool_end.is_none()
            && self.edits.is_empty()
            && self.usage.is_none()
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
        ProcessEvent::Stdout { run_id, line } => run_events_from_parsed(&run_id, parse_line(&line)),
        // `ProcessEvent` is #[non_exhaustive]; a future variant yields no
        // events until an adapter learns to handle it.
        _ => Vec::new(),
    }
}

/// Expand a decoded [`ParsedLine`] into its [`RunEvent`]s for `run_id`, in a
/// stable order: session (the run's init) → text → thinking → tool
/// start/end → edits → usage (end of turn) → activity.
///
/// Used by [`normalize_process_event`] and by adapters that wrap the line
/// parser in their own per-run state (e.g. codex's preamble-vs-answer state
/// machine, which decides *where* a message goes but still relies on this
/// for everything else) — so the `ParsedLine` → `RunEvent` mapping lives in
/// exactly one place.
///
/// Public so an **out-of-tree** harness can build a stateful parser the same
/// way: decide your own routing per line, then call this to expand a
/// `ParsedLine` into events with the canonical ordering — instead of
/// hand-rolling (and drifting from) the mapping. See `examples/custom_harness.rs`.
pub fn run_events_from_parsed(run_id: &str, parsed: ParsedLine) -> Vec<RunEvent> {
    let mut out = Vec::new();
    if let Some(session) = parsed.session {
        out.push(RunEvent::Session {
            run_id: run_id.to_owned(),
            session_id: session.session_id,
            model: session.model,
        });
    }
    if let Some(text) = parsed.text {
        out.push(RunEvent::Text {
            run_id: run_id.to_owned(),
            delta: text,
        });
    }
    if let Some(thinking) = parsed.thinking {
        out.push(RunEvent::Thinking {
            run_id: run_id.to_owned(),
            delta: thinking,
        });
    }
    if let Some(start) = parsed.tool_start {
        out.push(RunEvent::ToolStart {
            run_id: run_id.to_owned(),
            tool_call_id: start.tool_call_id,
            name: start.name,
            input: start.input,
        });
    }
    if let Some(end) = parsed.tool_end {
        out.push(RunEvent::ToolEnd {
            run_id: run_id.to_owned(),
            tool_call_id: end.tool_call_id,
            ok: end.ok,
            output: end.output,
        });
    }
    if !parsed.edits.is_empty() {
        out.push(RunEvent::SuggestedEdits {
            run_id: run_id.to_owned(),
            edits: parsed.edits,
        });
    }
    if let Some(usage) = parsed.usage {
        out.push(RunEvent::Usage {
            run_id: run_id.to_owned(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
        });
    }
    if let Some(activity) = parsed.activity {
        out.push(RunEvent::Activity {
            run_id: run_id.to_owned(),
            message: activity,
        });
    }
    out
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
    fn session_normalizes_and_serializes() {
        let events = normalize_process_event(
            ProcessEvent::Stdout {
                run_id: "r1".to_owned(),
                line: "ignored".to_owned(),
            },
            |_| ParsedLine {
                session: Some(SessionInfo {
                    session_id: Some("sess-1".to_owned()),
                    model: Some("opus".to_owned()),
                }),
                ..ParsedLine::default()
            },
        );
        assert!(matches!(
            events.as_slice(),
            [RunEvent::Session { run_id, session_id, model }]
                if run_id == "r1"
                    && session_id.as_deref() == Some("sess-1")
                    && model.as_deref() == Some("opus")
        ));
        let json = serde_json::to_value(RunEvent::Session {
            run_id: "r1".to_owned(),
            session_id: Some("sess-1".to_owned()),
            model: None,
        })
        .unwrap();
        assert_eq!(json["kind"], "session");
        assert_eq!(json["sessionId"], "sess-1");
        // model omitted from the wire when None (backward-compatible).
        assert!(json.get("model").is_none());
    }

    #[test]
    fn usage_normalizes_and_serializes() {
        let events = normalize_process_event(
            ProcessEvent::Stdout {
                run_id: "r1".to_owned(),
                line: "ignored".to_owned(),
            },
            |_| ParsedLine {
                usage: Some(UsageInfo {
                    input_tokens: Some(10),
                    output_tokens: Some(20),
                    total_tokens: Some(30),
                }),
                ..ParsedLine::default()
            },
        );
        assert!(matches!(
            events.as_slice(),
            [RunEvent::Usage { run_id, input_tokens: Some(10), output_tokens: Some(20), total_tokens: Some(30) }]
                if run_id == "r1"
        ));
        let json = serde_json::to_value(RunEvent::Usage {
            run_id: "r1".to_owned(),
            input_tokens: Some(10),
            output_tokens: None,
            total_tokens: Some(30),
        })
        .unwrap();
        assert_eq!(json["kind"], "usage");
        assert_eq!(json["inputTokens"], 10);
        assert_eq!(json["totalTokens"], 30);
        assert!(json.get("outputTokens").is_none()); // omitted when None
    }

    #[test]
    fn tool_io_is_carried_and_omitted_when_absent() {
        // input on ToolStart, output on ToolEnd — distinct events, distinct moments.
        let start = normalize_process_event(
            ProcessEvent::Stdout {
                run_id: "r1".to_owned(),
                line: "ignored".to_owned(),
            },
            |_| ParsedLine {
                tool_start: Some(ToolCallStart {
                    tool_call_id: "t1".to_owned(),
                    name: "ls".to_owned(),
                    input: Some("{\"dir\":\"/x\"}".to_owned()),
                }),
                ..ParsedLine::default()
            },
        );
        assert!(matches!(
            start.as_slice(),
            [RunEvent::ToolStart { input: Some(i), .. }] if i == "{\"dir\":\"/x\"}"
        ));
        // A ToolStart with no input omits the field on the wire (byte-identical
        // to the pre-enrichment shape).
        let json = serde_json::to_value(RunEvent::ToolStart {
            run_id: "r1".to_owned(),
            tool_call_id: "t1".to_owned(),
            name: "ls".to_owned(),
            input: None,
        })
        .unwrap();
        assert_eq!(json["kind"], "toolStart");
        assert_eq!(json["toolCallId"], "t1");
        assert!(json.get("input").is_none());

        let json = serde_json::to_value(RunEvent::ToolEnd {
            run_id: "r1".to_owned(),
            tool_call_id: "t1".to_owned(),
            ok: true,
            output: Some("done".to_owned()),
        })
        .unwrap();
        assert_eq!(json["kind"], "toolEnd");
        assert_eq!(json["output"], "done");
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
