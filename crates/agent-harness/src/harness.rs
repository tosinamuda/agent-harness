//! The neutral harness contract: the [`Harness`] trait, the run-control
//! handle, the neutral request/metadata types, and the shared
//! interactive-login helper.
//!
//! A *harness* is whatever actually answers the user's prompt — a CLI
//! agent (bob / Claude Code / Codex today), a direct LLM API tomorrow,
//! some other runner after that. A consumer only needs to: probe whether
//! a harness is ready, run a one-time install if required, stream a run,
//! and know which credential to ask for. This module is that seam.
//!
//! ## Design rules
//!
//! - **Object-safe trait.** Consumers hold `Box<dyn Harness>`; no
//!   generics leak across the seam.
//! - **Arc callbacks, not generic closures.** Streaming methods take
//!   `Arc<dyn Fn(..) + Send + Sync>` so they stay object-safe and can be
//!   cloned onto the reader threads the subprocess engine uses.
//! - **Normalize at the adapter, not the UI.** The event enums in
//!   [`crate::events`] are harness-neutral by intent; each adapter
//!   translates its CLI's wire format into them so the front-end consumes
//!   one shape regardless of which harness produced it.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};

use crate::events::RunEvent;
use cli_stream::{spawn_streaming, InstallEvent, ProcessEvent, ProcessHandle};

// --- Streaming callbacks --------------------------------------------

/// Callback a harness invokes for each run event. `Arc<dyn Fn>` is
/// `Clone + Send + Sync`, so it can be handed to the multiple reader
/// threads a process-backed harness uses without the trait method
/// needing to be generic.
pub type RunCallback = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// Callback a harness invokes for each install event.
pub type InstallCallback = Arc<dyn Fn(InstallEvent) + Send + Sync>;

// --- Run control (cancellation) -------------------------------------

/// Object-safe handle to an in-flight run. A process-backed harness
/// cancels by signalling its child; a request-backed harness (a hosted
/// LLM API) cancels by aborting its HTTP stream. The consumer only needs
/// these two operations, so the concrete mechanism stays behind the trait.
pub trait RunControl: Send + Sync {
    /// Stop the run. Best-effort; idempotent.
    fn cancel(&self) -> Result<(), String>;
    /// Whether [`cancel`](RunControl::cancel) was called.
    fn was_cancelled(&self) -> bool;
}

/// Boxed [`RunControl`] returned by [`Harness::run`].
pub type RunHandle = Box<dyn RunControl>;

// The engine's run handle is the canonical process-backed `RunControl`.
// Both the trait and the handle live in this crate, so this impl is here
// (orphan rule) rather than in any adapter crate.
impl RunControl for ProcessHandle {
    fn cancel(&self) -> Result<(), String> {
        ProcessHandle::cancel(self)
    }
    fn was_cancelled(&self) -> bool {
        ProcessHandle::was_cancelled(self)
    }
}

// --- Neutral request / metadata shapes ------------------------------

/// What the user wants the harness to do with the prompt. Mirrors
/// the Ask / Edit split the comment bubble already exposes; adapters
/// map it onto their own mode vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Answer / discuss. No file edits expected.
    Ask,
    /// Propose edits to the workspace.
    Edit,
}

/// How hard the model should think, in harness-neutral terms. Codex
/// maps this onto `model_reasoning_effort`; Claude Code has no
/// equivalent `-p` flag today and ignores it. Kept neutral so a future
/// harness that exposes effort can honor the same field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// The CLI/config token for this level (e.g. codex's
    /// `model_reasoning_effort="high"`).
    pub fn as_cli_value(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

/// User-chosen, harness-neutral run-shaping knobs. Every field is
/// optional; each adapter maps the ones its CLI supports and ignores
/// the rest (Claude has no reasoning-effort flag; Codex has no
/// max-turns flag). Grouped into one struct so the neutral
/// [`RunRequest`] stays open for extension — a new knob is a field
/// here, not a new positional parameter threaded through every caller.
#[derive(Debug, Clone, Default)]
pub struct RunTuning {
    /// Model id or alias passed verbatim to the CLI (`--model` /
    /// `-m`). `None` → let the CLI use its configured default.
    pub model: Option<String>,
    /// Reasoning effort (Codex: `-c model_reasoning_effort`).
    pub effort: Option<ReasoningEffort>,
    /// Cap on agentic turns (Claude: `--max-turns`).
    pub max_turns: Option<u32>,
}

/// A harness-neutral run request. Adapter-specific knobs (bob's
/// approval mode, coin budget, executable override) are filled in by
/// the adapter from its own defaults; the user-facing tuning the
/// picker exposes (model, effort, turn cap) rides on `tuning`.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// Caller-chosen id used to correlate events with the handle.
    pub run_id: String,
    pub prompt: String,
    /// Working directory for the run — the workspace path, so the
    /// harness's tool calls land inside the user's vault.
    pub cwd: Option<PathBuf>,
    pub mode: RunMode,
    /// Optional, harness-neutral run-shaping knobs (model, effort,
    /// turn cap). Adapters honor the subset their CLI supports.
    pub tuning: RunTuning,
}

/// Where a harness's secret lives in the OS keychain, and how to
/// label it in the UI. Lets the front-end ask for the right
/// credential per harness without hard-coding any one harness's slot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSpec {
    /// Human label, e.g. "Bob API key" / "Anthropic API key".
    pub label: String,
    pub keychain_service: String,
    pub keychain_account: String,
    /// Whether the harness can run at all without this credential.
    pub required: bool,
}

/// Harness-neutral readiness snapshot for the UI. `details` carries
/// adapter-specific probes (bob's Node/npm) as free-form JSON so the
/// trait stays generic.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessReadiness {
    pub harness_id: String,
    /// Installed *and* authenticated *and* able to run.
    pub ready: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub auth_configured: bool,
    pub error: Option<String>,
    /// Adapter-specific extra fields (serialized harness snapshot).
    pub details: serde_json::Value,
}

/// A model the harness can be pointed at, for the picker's model
/// selector. `value` is passed verbatim to the CLI (`--model` / `-m`)
/// via [`RunTuning::model`]; `label` is the human-facing name.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModel {
    pub value: String,
    pub label: String,
}

/// What a harness supports, so every consumer (the picker, the options
/// panel, the credential preflight, the chat availability gate) adapts
/// to it *declaratively* instead of branching on the harness id. A new
/// adapter that, say, needs a stored key just sets `credential_required:
/// true` here — no `id == "bob"` checks to hunt down.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    /// Compose stores this harness's credential (bob). When `false`,
    /// the CLI owns its own login (claude/codex) and Compose runs no
    /// credential/install preflight — a missing login surfaces as the
    /// harness's own run error rather than a Compose prompt.
    pub credential_required: bool,
    /// Emits previewable suggested edits the user approves before they
    /// apply (bob). When `false`, edits land on disk directly and the
    /// file watcher reflects them (claude/codex).
    pub previews_edits: bool,
    /// Curated model choices for the picker's selector. Empty → no
    /// curated list (rely on `allows_custom_model`).
    pub models: Vec<HarnessModel>,
    /// Whether a free-text model id is accepted beyond `models` (codex,
    /// whose model names change frequently). Drives a text field vs a
    /// fixed dropdown in the picker.
    pub allows_custom_model: bool,
    /// Honors [`RunTuning::effort`] (codex reasoning effort).
    pub supports_effort: bool,
    /// Honors [`RunTuning::max_turns`] (claude turn cap).
    pub supports_max_turns: bool,
    /// Supports an interactive [`Harness::login`] flow (the CLI's own
    /// OAuth, e.g. `claude auth login` / `codex login`). Drives the
    /// picker's "Sign in" affordance when installed-but-not-signed-in.
    /// `false` for harnesses Compose authenticates itself (bob).
    pub supports_login: bool,
}

/// Static metadata for the harness picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// True if the harness needs a one-time [`Harness::install`].
    pub requires_install: bool,
    /// Declarative capabilities — what the harness supports, so the UI
    /// and run-gating never special-case its id.
    pub capabilities: HarnessCapabilities,
}

// --- The trait ------------------------------------------------------

/// A pluggable agent backend. Implementors are cheap to construct
/// (they hold config, not connections) so a registry can hand out
/// fresh boxes on demand.
pub trait Harness: Send + Sync {
    /// Static metadata for the UI.
    fn info(&self) -> HarnessInfo;

    /// Probe availability / version / auth. May shell out; callers
    /// should treat it as blocking and run it off the UI thread.
    fn readiness(&self) -> HarnessReadiness;

    /// Stream a one-time install. Harnesses that need no install
    /// (e.g. a hosted-API adapter) return `Ok(())` immediately.
    fn install(&self, on_event: InstallCallback) -> Result<(), String>;

    /// Start a run, streaming events through `on_event`. Returns a
    /// handle immediately; work continues on background threads.
    fn run(&self, request: RunRequest, on_event: RunCallback) -> Result<RunHandle, String>;

    /// The credential this harness needs.
    fn credential(&self) -> CredentialSpec;

    /// Trigger the harness's own interactive sign-in (its CLI's OAuth),
    /// streaming progress as [`InstallEvent`]s — the same subprocess
    /// stream shape as [`install`](Harness::install). The flow opens the
    /// user's browser; this blocks until the login process exits, then
    /// `Done { ok }` reports success. Default: unsupported — harnesses
    /// that Compose authenticates itself (bob, via its API key) keep it.
    fn login(&self, _on_event: InstallCallback) -> Result<(), String> {
        Err("This harness does not support interactive sign-in.".to_owned())
    }
}

/// Run a harness's interactive sign-in command, streaming its output as
/// [`InstallEvent`]s and blocking until it exits. Reuses
/// [`spawn_streaming`] (PATH augmentation + reader threads, so a packaged
/// `.app` finds the CLI), mapping its process events onto the
/// install-stream shape (Step / Stdout / Stderr / Done). The login CLI
/// opens the user's browser for OAuth; we surface its output (incl. any
/// device-code URL) so the UI can show progress. Blocks on a condvar
/// until the process exits — the caller is a Tauri `(async)` command on
/// a worker thread, so the UI never blocks.
pub fn run_login_command(
    program: &str,
    args: &[&str],
    on_event: InstallCallback,
) -> Result<(), String> {
    (*on_event)(InstallEvent::Step {
        text: "Opening your browser to sign in…".to_owned(),
    });
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done_cb = Arc::clone(&done);
    let events_cb = Arc::clone(&on_event);
    // Bound, not `_`, so the handle outlives the wait (dropping it could
    // signal the child); by the time we return, the process has exited.
    let _handle = spawn_streaming(
        PathBuf::from(program),
        args.iter().map(|s| (*s).to_owned()).collect(),
        Vec::new(),
        std::env::current_dir().unwrap_or_default(),
        format!("login-{program}"),
        move |event| match event {
            ProcessEvent::Started { .. } => {}
            ProcessEvent::Stdout { line, .. } => {
                (*events_cb)(InstallEvent::Stdout { text: line });
            }
            ProcessEvent::Stderr { line, .. } => {
                (*events_cb)(InstallEvent::Stderr { text: line });
            }
            ProcessEvent::Error { message, .. } => {
                (*events_cb)(InstallEvent::Stderr { text: message });
            }
            ProcessEvent::Exited { exit_code, .. } => {
                (*events_cb)(InstallEvent::Done {
                    exit_code,
                    ok: exit_code == Some(0),
                });
                let (lock, cvar) = &*done_cb;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }
        },
    )?;
    let (lock, cvar) = &*done;
    let mut finished = lock.lock().unwrap();
    while !*finished {
        finished = cvar.wait(finished).unwrap();
    }
    Ok(())
}
