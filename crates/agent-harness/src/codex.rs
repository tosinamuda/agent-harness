//! OpenAI Codex (`codex`) as a [`Harness`].
//!
//! Same process-spawn shape as the bob and Claude adapters — a
//! different binary, flags, and stdout parser. We invoke
//! `codex exec --json` and parse its JSONL into the shared
//! normalized [`RunEvent`] stream.
//!
//! Auth: like Claude Code, Codex manages its own credentials (its
//! `codex login` / ChatGPT auth or its own `OPENAI_API_KEY` in the
//! environment), so Compose does not store or inject a key —
//! `credential().required` is `false`.
//!
//! Wire format reference (verified against the official docs,
//! https://developers.openai.com/codex/noninteractive): `--json`
//! emits one JSON object per line. The assistant's reply is an
//! `item.completed` event whose `item.type == "agent_message"` with
//! the full text in `item.text` — Codex sends the whole message at
//! once, not token deltas. Command executions arrive as
//! `command_execution` items; `thread.started` / `turn.*` are
//! lifecycle and ignored (process start/exit drives Started/Exited).

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Map, Value};

use crate::{
    normalize_process_event, spawn_streaming, CredentialSpec, Harness, HarnessCapabilities,
    HarnessInfo, HarnessReadiness, InstallCallback, InstallEvent, ParsedLine, RunCallback,
    RunHandle, RunMode, RunRequest, RunTuning, ToolCallEnd, ToolCallStart,
};

/// Registry id for the Codex harness.
pub const CODEX_HARNESS_ID: &str = "codex";

/// OpenAI Codex CLI as a [`Harness`].
#[derive(Debug, Default, Clone)]
pub struct CodexHarness;

impl CodexHarness {
    pub fn new() -> Self {
        Self
    }
}

impl Harness for CodexHarness {
    fn info(&self) -> HarnessInfo {
        HarnessInfo {
            id: CODEX_HARNESS_ID.to_owned(),
            display_name: "Codex".to_owned(),
            description: "OpenAI's Codex agent CLI. Uses your existing Codex login.".to_owned(),
            requires_install: true,
            capabilities: HarnessCapabilities {
                // Codex owns its own login and edits files directly.
                // Model names change often, so allow free-text entry
                // rather than a curated list; it exposes reasoning
                // effort but no turn cap.
                credential_required: false,
                previews_edits: false,
                models: Vec::new(),
                allows_custom_model: true,
                supports_effort: true,
                supports_max_turns: false,
                supports_login: true,
            },
        }
    }

    fn readiness(&self) -> HarnessReadiness {
        let Some(version) = probe_version("codex") else {
            return HarnessReadiness {
                harness_id: CODEX_HARNESS_ID.to_owned(),
                ready: false,
                installed: false,
                version: None,
                auth_configured: false,
                error: Some("Codex (`codex`) is not installed or not on PATH.".to_owned()),
                details: Value::Null,
            };
        };
        // Installed — distinguish signed-in from not so the picker can
        // offer "Sign in" instead of failing the first run.
        let signed_in = probe_codex_signed_in();
        HarnessReadiness {
            harness_id: CODEX_HARNESS_ID.to_owned(),
            ready: signed_in,
            installed: true,
            version: Some(version),
            auth_configured: signed_in,
            error: if signed_in {
                None
            } else {
                Some(
                    "Codex is installed but not signed in. Click Sign in to connect your ChatGPT/OpenAI account."
                        .to_owned(),
                )
            },
            details: Value::Null,
        }
    }

    fn install(&self, on_event: InstallCallback) -> Result<(), String> {
        (*on_event)(InstallEvent::Step {
            text: "Installing Codex via npm…".to_owned(),
        });
        let output = Command::new("npm")
            .args(["install", "-g", "@openai/codex"])
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
        let args = build_codex_args(prompt, mode, &tuning);
        let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        // No env injected — Codex uses its own auth. PATH augmentation
        // in spawn_streaming ensures `node` is found for a
        // Finder-launched .app.
        let handle = spawn_streaming(
            PathBuf::from("codex"),
            args,
            Vec::new(),
            cwd,
            run_id,
            move |event| {
                for normalized in normalize_process_event(event, parse_codex_line) {
                    (*on_event)(normalized);
                }
            },
        )?;
        Ok(Box::new(handle))
    }

    fn credential(&self) -> CredentialSpec {
        CredentialSpec {
            label: "Codex login (managed by the codex CLI)".to_owned(),
            keychain_service: "openai".to_owned(),
            keychain_account: "OPENAI_API_KEY".to_owned(),
            required: false,
        }
    }

    fn login(&self, on_event: InstallCallback) -> Result<(), String> {
        // `codex login` runs the CLI's OAuth flow (opens the browser).
        crate::run_login_command("codex", &["login"], on_event)
    }
}

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

/// Probe Codex's auth: `codex login status` exits 0 when signed in.
/// Lets [`readiness`] distinguish installed from signed-in (so the
/// picker can offer "Sign in").
fn probe_codex_signed_in() -> bool {
    Command::new("codex")
        .args(["login", "status"])
        .env("PATH", crate::augmented_node_path())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build the argv for a `codex exec --json` headless run. Kept pure
/// (no spawn) so the flag mapping is unit-tested. `tuning.model` →
/// `--model`; `tuning.effort` → `-c model_reasoning_effort="..."`
/// (codex's config override, value parsed as TOML); Codex has no
/// turn-cap flag, so `tuning.max_turns` is intentionally ignored.
/// Options precede the positional prompt, as `codex exec` expects.
fn build_codex_args(prompt: String, mode: RunMode, tuning: &RunTuning) -> Vec<String> {
    let mut args = vec!["exec".to_owned(), "--json".to_owned()];
    if let Some(model) = tuning.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    if let Some(effort) = tuning.effort {
        args.push("-c".to_owned());
        args.push(format!("model_reasoning_effort=\"{}\"", effort.as_cli_value()));
    }
    if matches!(mode, RunMode::Edit) {
        // Low-friction sandboxed auto-execution so Codex can apply
        // edits without interactive approval. (Exact sandbox flags
        // vary by codex version; --full-auto is the stable one.)
        args.push("--full-auto".to_owned());
    }
    args.push(prompt);
    args
}

/// Display label for a codex tool `item`, or `None` if the item isn't
/// a tool we surface as a card. Grounded in the `codex exec --json`
/// item types (`command_execution` carries the literal `command`;
/// `file_change` / `web_search` / `mcp_tool_call` get a fixed label).
fn codex_tool_label(item: &Map<String, Value>) -> Option<String> {
    match item.get("type").and_then(Value::as_str)? {
        "command_execution" => {
            let command = item.get("command").and_then(Value::as_str).unwrap_or("");
            Some(if command.is_empty() {
                "Running a command".to_owned()
            } else {
                format!("Running: {}", truncate(command, 80))
            })
        }
        "file_change" => Some("Editing files".to_owned()),
        "web_search" => Some("Searching the web".to_owned()),
        "mcp_tool_call" => Some("Tool · MCP".to_owned()),
        _ => None,
    }
}

/// Did a codex tool item succeed? `command_execution` reports
/// `exit_code` (0 = ok); otherwise fall back to `status` (anything but
/// an explicit failure is treated as ok, since not every tool type
/// carries an exit code).
fn codex_tool_ok(item: &Map<String, Value>) -> bool {
    if let Some(code) = item.get("exit_code").and_then(Value::as_i64) {
        return code == 0;
    }
    !matches!(
        item.get("status").and_then(Value::as_str),
        Some("failed") | Some("error")
    )
}

/// Parse one line of `codex exec --json` JSONL into the shared
/// [`ParsedLine`]. Assistant text is the full `agent_message` on
/// `item.completed`; tool items (`command_execution`, `file_change`,
/// `web_search`, `mcp_tool_call`) become structured tool cards
/// (`ToolStart`/`ToolEnd`). Codex edits files directly via tools
/// (reflected on disk by the file watcher), so it never emits
/// suggested-edit previews — `edits` stays empty.
pub fn parse_codex_line(line: &str) -> ParsedLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParsedLine::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return ParsedLine::default();
    };
    let Some(obj) = value.as_object() else {
        return ParsedLine::default();
    };

    match obj.get("type").and_then(Value::as_str) {
        Some("item.completed") => {
            let Some(item) = obj.get("item").and_then(Value::as_object) else {
                return ParsedLine::default();
            };
            // The assistant's reply: full text in one shot.
            if item.get("type").and_then(Value::as_str) == Some("agent_message") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        return ParsedLine {
                            text: Some(text.to_owned()),
                            ..ParsedLine::default()
                        };
                    }
                }
            }
            // A tool item finished. Grounded in codex 0.125.0's
            // `--json` schema: command_execution / web_search /
            // file_change / mcp_tool_call items arrive on
            // `item.completed` carrying `id` + `status` (+ `exit_code`
            // for commands). It does NOT emit an `item.started` for
            // these, so emit BOTH start and end from this one event —
            // that's what makes a card appear. The frontend dedups a
            // repeated start by id, so a future codex that *does* send
            // `item.started` still renders correctly.
            if let Some(label) = codex_tool_label(item) {
                return match item.get("id").and_then(Value::as_str) {
                    Some(id) => {
                        let id = id.to_owned();
                        ParsedLine {
                            tool_start: Some(ToolCallStart {
                                tool_call_id: id.clone(),
                                name: label,
                            }),
                            tool_end: Some(ToolCallEnd {
                                tool_call_id: id,
                                ok: codex_tool_ok(item),
                            }),
                            ..ParsedLine::default()
                        }
                    }
                    None => ParsedLine {
                        activity: Some(label),
                        ..ParsedLine::default()
                    },
                };
            }
            ParsedLine::default()
        }
        Some("item.started") => {
            let Some(item) = obj.get("item").and_then(Value::as_object) else {
                return ParsedLine::default();
            };
            // A tool announced before it finishes → a "running" card; the
            // matching `item.completed` flips it to done/error. If the
            // item has no `id` to key the card, degrade to a plain
            // activity line rather than drop it.
            if let Some(label) = codex_tool_label(item) {
                return match item.get("id").and_then(Value::as_str) {
                    Some(id) => ParsedLine {
                        tool_start: Some(ToolCallStart {
                            tool_call_id: id.to_owned(),
                            name: label,
                        }),
                        ..ParsedLine::default()
                    },
                    None => ParsedLine {
                        activity: Some(label),
                        ..ParsedLine::default()
                    },
                };
            }
            ParsedLine::default()
        }
        Some("error") => {
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex error");
            ParsedLine {
                activity: Some(truncate(message, 240)),
                ..ParsedLine::default()
            }
        }
        // thread.started / turn.started / turn.completed / turn.failed
        // and item.updated: lifecycle / partials — ignored.
        _ => ParsedLine::default(),
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReasoningEffort;

    #[test]
    fn agent_message_completed_becomes_text() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "id": "item_3", "type": "agent_message", "text": "Repo has docs and sdk." }
        })
        .to_string();
        let parsed = parse_codex_line(&line);
        assert_eq!(parsed.text.as_deref(), Some("Repo has docs and sdk."));
        assert!(parsed.edits.is_empty());
        assert!(parsed.activity.is_none());
    }

    // Tool-card tests grounded in codex-cli 0.125.0's `--json` schema:
    // tool items (command_execution / web_search / file_change /
    // mcp_tool_call) arrive on `item.completed` carrying `id`, `status`
    // (in_progress|completed|failed) and, for commands, `exit_code`.
    // Example (verbatim from the documented schema):
    //   {"type":"item.completed","item":{"id":"item_2","type":
    //    "command_execution","command":"bash -lc false",
    //    "aggregated_output":"","exit_code":1,"status":"failed"}}

    #[test]
    fn command_execution_completed_becomes_finished_tool_card() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "command_execution",
                "command": "bash -lc 'echo hi'",
                "aggregated_output": "hi\n",
                "exit_code": 0,
                "status": "completed"
            }
        })
        .to_string();
        let parsed = parse_codex_line(&line);
        let start = parsed.tool_start.expect("tool_start");
        let end = parsed.tool_end.expect("tool_end");
        assert_eq!(start.tool_call_id, "item_2");
        assert_eq!(end.tool_call_id, "item_2");
        assert_eq!(start.name, "Running: bash -lc 'echo hi'");
        assert!(end.ok, "exit_code 0 → ok");
        assert!(parsed.activity.is_none());
        assert!(parsed.text.is_none());
    }

    #[test]
    fn command_execution_nonzero_exit_is_error_card() {
        let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"bash -lc false","aggregated_output":"","exit_code":1,"status":"failed"}}"#;
        let end = parse_codex_line(line).tool_end.expect("tool_end");
        assert!(!end.ok, "exit_code 1 / status failed → error");
    }

    #[test]
    fn web_search_completed_becomes_tool_card() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "id": "item_5", "type": "web_search", "status": "completed" }
        })
        .to_string();
        let parsed = parse_codex_line(&line);
        assert_eq!(parsed.tool_start.expect("start").name, "Searching the web");
        assert!(parsed.tool_end.expect("end").ok, "no exit_code, status completed → ok");
    }

    #[test]
    fn started_tool_with_id_becomes_running_card() {
        let line = serde_json::json!({
            "type": "item.started",
            "item": { "id": "item_1", "type": "command_execution", "command": "bash -lc ls", "status": "in_progress" }
        })
        .to_string();
        let parsed = parse_codex_line(&line);
        assert_eq!(parsed.tool_start.expect("start").name, "Running: bash -lc ls");
        assert!(parsed.tool_end.is_none(), "started → running (no end yet)");
        assert!(parsed.activity.is_none());
    }

    #[test]
    fn tool_without_id_degrades_to_activity() {
        // Defensive: an item lacking `id` can't key a card, so it falls
        // back to a plain activity line rather than vanishing.
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "type": "command_execution", "command": "ls -la", "exit_code": 0 }
        })
        .to_string();
        let parsed = parse_codex_line(&line);
        assert_eq!(parsed.activity.as_deref(), Some("Running: ls -la"));
        assert!(parsed.tool_start.is_none() && parsed.tool_end.is_none());
    }

    #[test]
    fn lifecycle_events_are_ignored() {
        for line in [
            r#"{"type":"thread.started","thread_id":"abc"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#,
        ] {
            let parsed = parse_codex_line(line);
            assert!(parsed.text.is_none() && parsed.activity.is_none());
        }
    }

    #[test]
    fn error_event_becomes_activity() {
        let line = r#"{"type":"error","message":"rate limited"}"#;
        assert_eq!(parse_codex_line(line).activity.as_deref(), Some("rate limited"));
    }

    #[test]
    fn non_json_is_ignored() {
        assert!(parse_codex_line("plain text").text.is_none());
    }

    #[test]
    fn codex_info_and_credential() {
        let h = CodexHarness::new();
        assert_eq!(h.info().id, CODEX_HARNESS_ID);
        assert!(h.info().requires_install);
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
    fn codex_args_default_omit_model_and_effort() {
        let args = build_codex_args("hi".to_owned(), RunMode::Ask, &RunTuning::default());
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_owned()));
        assert!(!args.iter().any(|a| a == "--model"));
        assert!(!args.iter().any(|a| a == "-c"));
        assert!(!args.iter().any(|a| a == "--full-auto"));
        // Prompt is the trailing positional arg.
        assert_eq!(args.last().map(String::as_str), Some("hi"));
    }

    #[test]
    fn codex_args_carry_model_and_effort_and_ignore_max_turns() {
        let tuning = RunTuning {
            model: Some("gpt-5-codex".to_owned()),
            effort: Some(ReasoningEffort::High),
            max_turns: Some(5),
        };
        let args = build_codex_args("hi".to_owned(), RunMode::Edit, &tuning);
        assert_eq!(flag_value(&args, "--model"), Some("gpt-5-codex"));
        assert_eq!(flag_value(&args, "-c"), Some("model_reasoning_effort=\"high\""));
        assert!(args.contains(&"--full-auto".to_owned()));
        // Codex has no turn-cap flag — max_turns must not leak.
        assert!(!args.iter().any(|a| a == "--max-turns"));
        // Options precede the prompt; the prompt stays last.
        assert_eq!(args.last().map(String::as_str), Some("hi"));
    }
}
