//! bob's raw passthrough tier — bob's `--output-format stream-json` stdout,
//! decoded as **untyped JSON**, for consumers that want to interpret bob's
//! output themselves instead of the neutral [`crate::RunEvent`] vocabulary.
//!
//! Deliberately **untyped**: one stdout line → one [`serde_json::Value`], with
//! no schema imposed on top. This is on purpose — a typed "raw" decode is
//! brittle:
//!
//! * a new field bob adds to a known event would be silently dropped by a
//!   typed struct, but is preserved here;
//! * a new event `type` in a future bob release just flows through;
//! * there is nothing here to keep in sync with bob's protocol.
//!
//! It also means bob's wire grammar (the `type` discriminator, the field
//! names) lives in exactly **one** place —
//! [`parse_bob_line`](super::parser::parse_bob_line), the *neutral* tier that
//! interprets it into `RunEvent`. This raw tier carries none of that
//! knowledge; it is a passthrough.
//!
//! Want the absolute rawest? [`crate::ProcessEvent::Stdout`] already carries
//! the verbatim line — skip this and parse the string yourself.

use serde_json::Value;

/// Decode one line of bob's stream-json stdout into an untyped
/// [`serde_json::Value`], losslessly and without interpretation:
///
/// * a JSON value (object, array, scalar) → that value, **verbatim** — every
///   field preserved, nothing reinterpreted;
/// * a non-JSON line → `Value::String(<the raw, untrimmed line>)`;
/// * a blank line → `Value::Null`.
///
/// No special-casing: `attempt_completion` is just an object with
/// `"type":"tool_use"`, `<thinking>` markers stay inside `content`, and any
/// `type` bob invents later is simply present. The consumer reads whatever
/// fields it needs (`v["type"]`, `v["parameters"]["result"]`, `v["stats"]`, …).
pub fn parse_bob_raw(line: &str) -> Value {
    if line.trim().is_empty() {
        return Value::Null;
    }
    // Valid JSON → as-is. Non-JSON → the raw (untrimmed) line, never dropped.
    serde_json::from_str(line.trim()).unwrap_or_else(|_| Value::String(line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shapes captured from `bob 1.0.4 -o stream-json` (the same
    /// capture the neutral parser is grounded against) — here we assert the
    /// untyped passthrough: no field dropped, no `type` interpreted.
    #[test]
    fn grounded_against_real_bob_capture() {
        let v = parse_bob_raw(r#"{"type":"init","session_id":"s","model":"premium"}"#);
        assert_eq!(v["type"].as_str(), Some("init"));
        assert_eq!(v["session_id"].as_str(), Some("s"));
        assert_eq!(v["model"].as_str(), Some("premium"));

        // Echoed user prompt — kept (role "user"), not dropped.
        let v = parse_bob_raw(r#"{"type":"message","role":"user","content":"list files"}"#);
        assert_eq!(v["role"].as_str(), Some("user"));
        assert_eq!(v["content"].as_str(), Some("list files"));

        // Assistant reasoning — <thinking> tags stay INSIDE content (no split).
        let v = parse_bob_raw(
            r#"{"type":"message","role":"assistant","content":"<thinking>\n","delta":true}"#,
        );
        assert_eq!(v["content"].as_str(), Some("<thinking>\n"));
        assert_eq!(v["delta"].as_bool(), Some(true));

        // tool_use — parameters preserved verbatim.
        let v = parse_bob_raw(
            r#"{"type":"tool_use","tool_name":"list_files","tool_id":"tool-1","parameters":{"dir_path":"/x/docs"}}"#,
        );
        assert_eq!(v["tool_name"].as_str(), Some("list_files"));
        assert_eq!(v["parameters"]["dir_path"].as_str(), Some("/x/docs"));

        // attempt_completion — NOT promoted to "the answer"; just a tool_use.
        let v = parse_bob_raw(
            r#"{"type":"tool_use","tool_id":"tool-2","tool_name":"attempt_completion","parameters":{"result":"The docs directory contains 10 files."}}"#,
        );
        assert_eq!(v["type"].as_str(), Some("tool_use"));
        assert_eq!(v["tool_name"].as_str(), Some("attempt_completion"));
        assert_eq!(
            v["parameters"]["result"].as_str(),
            Some("The docs directory contains 10 files.")
        );

        // result — the full raw stats object is preserved.
        let v = parse_bob_raw(
            r#"{"type":"result","status":"success","stats":{"total_tokens":1280,"session_costs":3,"tool_calls":2}}"#,
        );
        assert_eq!(v["stats"]["total_tokens"].as_i64(), Some(1280));
        assert_eq!(v["stats"]["session_costs"].as_i64(), Some(3));
        assert_eq!(v["stats"]["tool_calls"].as_i64(), Some(2));
    }

    #[test]
    fn blank_line_is_null() {
        assert_eq!(parse_bob_raw("   "), Value::Null);
    }

    #[test]
    fn non_json_line_is_preserved_verbatim() {
        // bob occasionally prints prose / stderr-ish lines — kept untrimmed.
        assert_eq!(
            parse_bob_raw("  hello world  "),
            Value::String("  hello world  ".to_owned())
        );
    }

    #[test]
    fn unknown_type_passes_through_untouched() {
        // A future/unknown event type isn't special-cased — it's just present.
        let v = parse_bob_raw(r#"{"type":"telemetry","foo":42}"#);
        assert_eq!(v["type"].as_str(), Some("telemetry"));
        assert_eq!(v["foo"].as_i64(), Some(42));
    }

    #[test]
    fn unknown_field_on_known_type_is_kept() {
        // The whole point of staying untyped: a NEW field bob adds to a known
        // event survives — a typed decode would have dropped it.
        let v = parse_bob_raw(
            r#"{"type":"message","role":"assistant","content":"hi","usage":{"out":7}}"#,
        );
        assert_eq!(v["content"].as_str(), Some("hi"));
        assert_eq!(v["usage"]["out"].as_i64(), Some(7)); // future-proof
    }

    #[test]
    fn empty_string_fields_are_kept() {
        let v = parse_bob_raw(r#"{"type":"message","role":"assistant","content":""}"#);
        assert_eq!(v["content"].as_str(), Some(""));
    }
}
