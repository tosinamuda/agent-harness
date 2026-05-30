//! Build the bob CLI argv and spawn it via the shared streaming engine.
//!
//! Both `bob-api` (browser preview HTTP) and `src-tauri` (desktop IPC)
//! consume this. The generic subprocess engine (`spawn_streaming`, the
//! process-event type, the run handle) lives in `agent-harness`; this
//! module is the bob-specific layer on top — the chat-mode / approval
//! flags, `RunBobOptions`, and injecting bob's `BOBSHELL_API_KEY`.

use crate::keychain::resolve_api_key;
use cli_stream::{spawn_streaming, ProcessEvent, ProcessHandle};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// --- Wire shapes (bob-specific) -------------------------------------

/// Bob chat mode CLI flag. `--chat-mode <value>` accepts the snake_case
/// forms below.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BobChatMode {
    Plan,
    Code,
    Advanced,
    Ask,
}

impl BobChatMode {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Code => "code",
            Self::Advanced => "advanced",
            Self::Ask => "ask",
        }
    }
}

/// Bob's approval flow. `default` prompts the user via bob's UI; `yolo`
/// skips prompts. We only use `default` and `yolo` today (the legacy
/// `auto_edit` mode kept for back-compat with the existing Tauri command
/// surface).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BobApprovalMode {
    Default,
    AutoEdit,
    Yolo,
}

impl BobApprovalMode {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AutoEdit => "auto_edit",
            Self::Yolo => "yolo",
        }
    }
}

/// Options for a single bob run. Built by both the axum endpoint (from
/// JSON body) and the Tauri command (from invoke args).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunBobOptions {
    pub prompt: String,
    #[serde(default = "default_chat_mode")]
    pub chat_mode: BobChatMode,
    #[serde(default = "default_approval_mode")]
    pub approval_mode: BobApprovalMode,
    #[serde(default = "default_max_coins")]
    pub max_coins: u32,
    /// Working directory the bob process runs in. Defaults to the
    /// caller's cwd. For workspace-scoped runs, pass the workspace path
    /// so bob's tool calls land inside that workspace.
    pub cwd: Option<PathBuf>,
    /// Override the bob executable path. Mainly for tests + when the
    /// caller has already resolved bob (e.g. Tauri's locator). Defaults
    /// to `bob` on PATH.
    #[serde(default)]
    pub bob_executable: Option<PathBuf>,
}

fn default_chat_mode() -> BobChatMode { BobChatMode::Ask }
fn default_approval_mode() -> BobApprovalMode { BobApprovalMode::Default }
fn default_max_coins() -> u32 { 30 }

// --- Spawn ----------------------------------------------------------

/// Spawn bob and stream output through `callback` until the child exits.
/// Returns a [`ProcessHandle`] immediately — the reader + wait threads
/// continue in the background.
///
/// `run_id` is opaque to bob-rs; the caller chooses the identifier and
/// uses it to correlate events with the handle.
pub fn spawn_bob<F>(
    opts: RunBobOptions,
    run_id: String,
    callback: F,
) -> Result<ProcessHandle, String>
where
    F: FnMut(ProcessEvent) + Send + Sync + Clone + 'static,
{
    let args = build_args(&opts);
    let api_key = resolve_api_key().map(|(value, _)| value).unwrap_or_default();
    let program: PathBuf = opts.bob_executable.clone().unwrap_or_else(|| PathBuf::from("bob"));
    let cwd = opts.cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    spawn_bob_raw(program, args, api_key, cwd, run_id, callback)
}

/// Lower-level spawn for callers that have already built the argv,
/// resolved the bob executable path, and loaded the API key themselves
/// (the Tauri runner, which carries its own locator + workspace-aware
/// argv builder). Thin bob-specific wrapper over
/// [`agent_harness::spawn_streaming`]: sets bob's `BOBSHELL_API_KEY` env
/// var, otherwise identical.
pub fn spawn_bob_raw<F>(
    program: PathBuf,
    args: Vec<String>,
    api_key: String,
    cwd: PathBuf,
    run_id: String,
    callback: F,
) -> Result<ProcessHandle, String>
where
    F: FnMut(ProcessEvent) + Send + Sync + Clone + 'static,
{
    spawn_streaming(
        program,
        args,
        vec![("BOBSHELL_API_KEY".to_owned(), api_key)],
        cwd,
        run_id,
        callback,
    )
}

/// Build the bob CLI argv. Mirrors the structure used by both the Vite
/// `bobRunPlugin` and the Tauri `build_bob_command`.
fn build_args(opts: &RunBobOptions) -> Vec<String> {
    vec![
        opts.prompt.clone(),
        "--chat-mode".to_owned(),
        opts.chat_mode.as_cli_value().to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--approval-mode".to_owned(),
        opts.approval_mode.as_cli_value().to_owned(),
        "--accept-license".to_owned(),
        "--max-coins".to_owned(),
        opts.max_coins.to_string(),
    ]
}
