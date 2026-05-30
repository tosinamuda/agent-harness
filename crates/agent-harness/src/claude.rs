//! Claude Code (`claude`) as a [`Harness`].
//!
//! Same process-spawn shape as the bob adapter — a different binary,
//! flags, and stdout parser. We invoke `claude -p` in headless
//! streaming mode and parse its NDJSON into the shared normalized
//! [`RunEvent`] stream, so the front-end treats Claude exactly like
//! any other harness.
//!
//! Auth: Claude Code manages its own credentials (its OAuth login or
//! its own `ANTHROPIC_API_KEY` in the environment), so Compose does
//! not store or inject a key — `credential().required` is `false`.
//!
//! Wire format reference (verified against the official headless
//! docs, https://code.claude.com/docs/en/headless): with
//! `--output-format stream-json --verbose --include-partial-messages`,
//! streamed text arrives as `stream_event` lines whose
//! `event.delta.type == "text_delta"`. Tool starts arrive as
//! `content_block_start` with a `tool_use` block. Aggregate
//! `assistant` / `result` lines are ignored — their text already
//! streamed via the deltas, so honoring them too would double it.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::{
    normalize_process_event, spawn_streaming, CredentialSpec, Harness, HarnessCapabilities,
    HarnessInfo, HarnessModel, HarnessReadiness, InstallCallback, InstallEvent, ParsedLine,
    RunCallback, RunHandle, RunMode, RunRequest, RunTuning, ToolCallEnd, ToolCallStart,
};

/// Registry id for the Claude Code harness.
pub const CLAUDE_HARNESS_ID: &str = "claude";

/// Claude Code CLI as a [`Harness`].
#[derive(Debug, Default, Clone)]
pub struct ClaudeHarness;

impl ClaudeHarness {
    pub fn new() -> Self {
        Self
    }
}

impl Harness for ClaudeHarness {
    fn info(&self) -> HarnessInfo {
        HarnessInfo {
            id: CLAUDE_HARNESS_ID.to_owned(),
            display_name: "Claude Code".to_owned(),
            description: "Anthropic's Claude Code agent CLI. Uses your existing Claude Code login."
                .to_owned(),
            requires_install: true,
            capabilities: HarnessCapabilities {
                // Claude Code owns its own login; it edits files
                // directly (no previews). Curated model aliases (no
                // free-text) + a turn cap; no reasoning-effort flag.
                credential_required: false,
                previews_edits: false,
                models: vec![
                    HarnessModel { value: "sonnet".to_owned(), label: "Sonnet (latest)".to_owned() },
                    HarnessModel { value: "opus".to_owned(), label: "Opus (latest)".to_owned() },
                    HarnessModel { value: "haiku".to_owned(), label: "Haiku (latest)".to_owned() },
                ],
                allows_custom_model: false,
                supports_effort: false,
                supports_max_turns: true,
                supports_login: true,
            },
        }
    }

    fn readiness(&self) -> HarnessReadiness {
        let Some(version) = probe_version("claude") else {
            return HarnessReadiness {
                harness_id: CLAUDE_HARNESS_ID.to_owned(),
                ready: false,
                installed: false,
                version: None,
                auth_configured: false,
                error: Some("Claude Code (`claude`) is not installed or not on PATH.".to_owned()),
                details: Value::Null,
            };
        };
        // Installed — now distinguish signed-in from not, so the picker
        // can offer "Sign in" instead of failing the first run.
        let signed_in = probe_claude_signed_in();
        HarnessReadiness {
            harness_id: CLAUDE_HARNESS_ID.to_owned(),
            ready: signed_in,
            installed: true,
            version: Some(version),
            auth_configured: signed_in,
            error: if signed_in {
                None
            } else {
                Some(
                    "Claude Code is installed but not signed in. Click Sign in to connect your Anthropic account."
                        .to_owned(),
                )
            },
            details: Value::Null,
        }
    }

    fn install(&self, on_event: InstallCallback) -> Result<(), String> {
        // npm global install. Blocking (matches the `install`
        // contract); we capture output and forward it as install
        // events. Streaming live progress is a future refinement.
        (*on_event)(InstallEvent::Step {
            text: "Installing Claude Code via npm…".to_owned(),
        });
        let output = Command::new("npm")
            .args(["install", "-g", "@anthropic-ai/claude-code"])
            .env("PATH", crate::augmented_node_path())
            .output()
            .map_err(|e| format!("failed to run npm: {e}"))?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            (*on_event)(InstallEvent::Stdout {
                text: line.to_owned(),
            });
        }
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            (*on_event)(InstallEvent::Stderr {
                text: line.to_owned(),
            });
        }
        (*on_event)(InstallEvent::Done {
            exit_code: output.status.code(),
            ok: output.status.success(),
        });
        Ok(())
    }

    fn run(&self, request: RunRequest, on_event: RunCallback) -> Result<RunHandle, String> {
        let RunRequest { run_id, prompt, cwd, mode, tuning } = request;
        let args = build_claude_args(prompt, mode, &tuning);
        let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        // No env injected — Claude Code uses its own auth. PATH
        // augmentation inside `spawn_streaming` ensures `node` is
        // found for a Finder-launched .app.
        let handle = spawn_streaming(
            PathBuf::from("claude"),
            args,
            Vec::new(),
            cwd,
            run_id,
            move |event| {
                for normalized in normalize_process_event(event, parse_claude_line) {
                    (*on_event)(normalized);
                }
            },
        )?;
        Ok(Box::new(handle))
    }

    fn credential(&self) -> CredentialSpec {
        CredentialSpec {
            label: "Claude Code login (managed by the claude CLI)".to_owned(),
            keychain_service: "anthropic".to_owned(),
            keychain_account: "ANTHROPIC_API_KEY".to_owned(),
            // Claude Code authenticates itself; Compose need not store
            // a key for it.
            required: false,
        }
    }

    fn login(&self, on_event: InstallCallback) -> Result<(), String> {
        // `claude auth login` runs the CLI's OAuth flow (opens the
        // browser); streamed + blocked-until-exit by the shared helper.
        crate::run_login_command("claude", &["auth", "login"], on_event)
    }
}

/// Probe Claude Code's auth: `claude auth status` prints JSON with a
/// `loggedIn` boolean (exit 0 when signed in). Returns true only when
/// signed in; defensively falls back to the exit code if the JSON is
/// unexpected. Lets [`readiness`] distinguish installed from signed-in.
fn probe_claude_signed_in() -> bool {
    let Ok(output) = Command::new("claude")
        .args(["auth", "status"])
        .env("PATH", crate::augmented_node_path())
        .output()
    else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(stdout.trim()) {
        if let Some(logged_in) = map.get("loggedIn").and_then(Value::as_bool) {
            return logged_in;
        }
    }
    // Fallback: exit 0 with non-empty output ≈ signed in.
    output.status.success() && !stdout.trim().is_empty()
}

/// Build the argv for a `claude -p` headless run. Kept pure (no
/// spawn) so the flag mapping is unit-tested. `tuning.model` →
/// `--model`, `tuning.max_turns` → `--max-turns`; Claude Code has no
/// reasoning-effort `-p` flag, so `tuning.effort` is intentionally
/// ignored here.
fn build_claude_args(prompt: String, mode: RunMode, tuning: &RunTuning) -> Vec<String> {
    let mut args = vec![
        "-p".to_owned(),
        prompt,
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--include-partial-messages".to_owned(),
    ];
    if let Some(model) = tuning.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    if let Some(max_turns) = tuning.max_turns {
        args.push("--max-turns".to_owned());
        args.push(max_turns.to_string());
    }
    if matches!(mode, RunMode::Edit) {
        // Let Claude write files without an interactive prompt in
        // Edit mode; in Ask mode it stays read-only by default.
        args.push("--permission-mode".to_owned());
        args.push("acceptEdits".to_owned());
    }
    args
}

/// Run `<program> --version`, returning the trimmed stdout on
/// success. Used by readiness to detect the CLI on PATH.
fn probe_version(program: &str) -> Option<String> {
    // Augment PATH so a packaged `.app` (minimal launchd PATH) can find a
    // CLI installed via nvm / Homebrew / official installer — otherwise an
    // installed CLI is mis-reported as "not installed".
    let output = Command::new(program)
        .arg("--version")
        .env("PATH", crate::augmented_node_path())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse one line of Claude Code's `stream-json` output into the
/// shared [`ParsedLine`] shape. See the module docs for the wire
/// format. Claude edits files directly via its tools (reflected on
/// disk by the file watcher), so it never emits suggested-edit
/// previews — `edits` is always empty here.
pub fn parse_claude_line(line: &str) -> ParsedLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParsedLine::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        // Claude shouldn't emit non-JSON in stream-json mode; ignore.
        return ParsedLine::default();
    };
    let Some(obj) = value.as_object() else {
        return ParsedLine::default();
    };

    match obj.get("type").and_then(Value::as_str) {
        Some("stream_event") => {
            let Some(event) = obj.get("event").and_then(Value::as_object) else {
                return ParsedLine::default();
            };
            let event_type = event.get("type").and_then(Value::as_str);

            // Streamed assistant text.
            if event_type == Some("content_block_delta") {
                if let Some(delta) = event.get("delta").and_then(Value::as_object) {
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                return ParsedLine {
                                    text: Some(text.to_owned()),
                                    ..ParsedLine::default()
                                };
                            }
                        }
                    }
                    // Extended-thinking deltas (Anthropic streaming):
                    // `delta.type == "thinking_delta"` carries the reasoning
                    // text in `delta.thinking`. Surfaced as a distinct
                    // Thinking event (harmless no-op when the model isn't
                    // thinking, since the arm never matches).
                    if delta.get("type").and_then(Value::as_str) == Some("thinking_delta") {
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            if !thinking.is_empty() {
                                return ParsedLine {
                                    thinking: Some(thinking.to_owned()),
                                    ..ParsedLine::default()
                                };
                            }
                        }
                    }
                }
            }

            // A tool call beginning → structured ToolStart (id + name) so
            // the UI renders a state-ful card, ended by the matching
            // tool_result (the `user` arm below).
            if event_type == Some("content_block_start") {
                if let Some(block) = event.get("content_block").and_then(Value::as_object) {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                        return ParsedLine {
                            tool_start: Some(ToolCallStart {
                                tool_call_id: id.to_owned(),
                                name: name.to_owned(),
                            }),
                            ..ParsedLine::default()
                        };
                    }
                }
            }

            ParsedLine::default()
        }
        Some("system") => {
            // Surface retry progress; ignore init + other system events.
            if obj.get("subtype").and_then(Value::as_str) == Some("api_retry") {
                let attempt = obj.get("attempt").and_then(Value::as_u64).unwrap_or(1);
                return ParsedLine {
                    activity: Some(format!("Retrying (attempt {attempt})…")),
                    ..ParsedLine::default()
                };
            }
            ParsedLine::default()
        }
        Some("user") => {
            // Tool results arrive as a `user` message carrying
            // tool_result blocks; each ends a tool call (matched by
            // tool_use_id; `ok` = not is_error). Grounded from a real
            // claude stream-json run.
            if let Some(content) = obj
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                for item in content {
                    let Some(block) = item.as_object() else {
                        continue;
                    };
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                            let is_error =
                                block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            return ParsedLine {
                                tool_end: Some(ToolCallEnd {
                                    tool_call_id: id.to_owned(),
                                    ok: !is_error,
                                }),
                                ..ParsedLine::default()
                            };
                        }
                    }
                }
            }
            ParsedLine::default()
        }
        // Aggregate assistant/result lines: text already streamed via
        // deltas, so ignore to avoid double-counting.
        _ => ParsedLine::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReasoningEffort;

    fn text_delta(text: &str) -> String {
        serde_json::json!({
            "type": "stream_event",
            "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": text } }
        })
        .to_string()
    }

    #[test]
    fn streams_text_deltas() {
        let parsed = parse_claude_line(&text_delta("Hello"));
        assert_eq!(parsed.text.as_deref(), Some("Hello"));
        assert!(parsed.activity.is_none());
        assert!(parsed.edits.is_empty());
    }

    #[test]
    fn empty_text_delta_yields_nothing() {
        let parsed = parse_claude_line(&text_delta(""));
        assert!(parsed.text.is_none());
    }

    fn thinking_delta(text: &str) -> String {
        serde_json::json!({
            "type": "stream_event",
            "event": { "type": "content_block_delta", "delta": { "type": "thinking_delta", "thinking": text } }
        })
        .to_string()
    }

    #[test]
    fn streams_thinking_deltas() {
        let parsed = parse_claude_line(&thinking_delta("Let me reason"));
        assert_eq!(parsed.thinking.as_deref(), Some("Let me reason"));
        assert!(parsed.text.is_none());
        assert!(parsed.activity.is_none());
    }

    #[test]
    fn tool_use_start_becomes_tool_start() {
        let line = serde_json::json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_start",
                "content_block": { "type": "tool_use", "name": "Edit", "id": "toolu_1" }
            }
        })
        .to_string();
        let parsed = parse_claude_line(&line);
        let start = parsed.tool_start.expect("tool_start");
        assert_eq!(start.tool_call_id, "toolu_1");
        assert_eq!(start.name, "Edit");
        assert!(parsed.activity.is_none());
    }

    #[test]
    fn tool_result_becomes_tool_end() {
        let ok_line = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "is_error": false, "content": "ok" }
            ]}
        })
        .to_string();
        let end = parse_claude_line(&ok_line).tool_end.expect("tool_end");
        assert_eq!(end.tool_call_id, "toolu_1");
        assert!(end.ok);

        let err_line = serde_json::json!({
            "type": "user",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "toolu_2", "is_error": true }
            ]}
        })
        .to_string();
        assert!(!parse_claude_line(&err_line).tool_end.unwrap().ok);
    }

    #[test]
    fn api_retry_becomes_activity() {
        let line = serde_json::json!({
            "type": "system", "subtype": "api_retry", "attempt": 2, "max_retries": 5
        })
        .to_string();
        assert_eq!(
            parse_claude_line(&line).activity.as_deref(),
            Some("Retrying (attempt 2)…")
        );
    }

    #[test]
    fn init_and_aggregate_lines_are_ignored() {
        // system/init
        assert!(parse_claude_line(
            &serde_json::json!({ "type": "system", "subtype": "init", "model": "x" }).to_string()
        )
        .text
        .is_none());
        // aggregate assistant message — text already came via deltas
        let assistant = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "full turn" }] }
        })
        .to_string();
        let parsed = parse_claude_line(&assistant);
        assert!(parsed.text.is_none() && parsed.activity.is_none());
        // result line
        assert!(parse_claude_line(
            &serde_json::json!({ "type": "result", "subtype": "success", "is_error": false }).to_string()
        )
        .text
        .is_none());
    }

    #[test]
    fn non_json_is_ignored_not_echoed() {
        // Unlike bob, Claude's stream-json is always JSON; a stray
        // line should be dropped, not surfaced as assistant text.
        assert!(parse_claude_line("not json").text.is_none());
    }

    #[test]
    fn claude_info_and_credential() {
        let h = ClaudeHarness::new();
        assert_eq!(h.info().id, CLAUDE_HARNESS_ID);
        assert!(h.info().requires_install);
        // Claude manages its own auth — Compose doesn't require a key.
        assert!(!h.credential().required);
    }

    /// Value of the arg immediately following `flag`, if present.
    fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    #[test]
    fn claude_args_default_omit_model_and_turn_cap() {
        let args = build_claude_args("hi".to_owned(), RunMode::Ask, &RunTuning::default());
        // Prompt is the positional right after `-p`.
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "hi");
        assert!(!args.iter().any(|a| a == "--model"));
        assert!(!args.iter().any(|a| a == "--max-turns"));
        assert!(!args.iter().any(|a| a == "--permission-mode"));
    }

    #[test]
    fn claude_args_carry_model_and_max_turns_and_ignore_effort() {
        let tuning = RunTuning {
            model: Some("opus".to_owned()),
            effort: Some(ReasoningEffort::High),
            max_turns: Some(5),
        };
        let args = build_claude_args("hi".to_owned(), RunMode::Ask, &tuning);
        assert_eq!(flag_value(&args, "--model"), Some("opus"));
        assert_eq!(flag_value(&args, "--max-turns"), Some("5"));
        // Claude Code has no reasoning-effort `-p` flag — it must not leak.
        assert!(!args.iter().any(|a| a.contains("reasoning_effort")));
    }

    #[test]
    fn claude_blank_model_is_treated_as_unset() {
        let tuning = RunTuning { model: Some("   ".to_owned()), ..RunTuning::default() };
        let args = build_claude_args("hi".to_owned(), RunMode::Ask, &tuning);
        assert!(!args.iter().any(|a| a == "--model"));
    }

    #[test]
    fn claude_edit_mode_accepts_edits() {
        let args = build_claude_args("hi".to_owned(), RunMode::Edit, &RunTuning::default());
        assert_eq!(flag_value(&args, "--permission-mode"), Some("acceptEdits"));
    }
}
