//! Compose's neutral agent-harness core.
//!
//! The library you depend on to drive — or build — an agent harness,
//! independent of any specific backend. It provides:
//!   * the [`Harness`] trait + the neutral request/metadata types
//!     ([`RunRequest`] / [`RunTuning`] / [`HarnessInfo`] / …),
//!   * the normalized [`RunEvent`] vocabulary every adapter parses into
//!     ([`normalize_process_event`] + [`ParsedLine`]),
//!   * the generic streaming subprocess engine ([`spawn_streaming`] +
//!     [`ProcessEvent`] + [`ProcessHandle`]) + the install/login event
//!     shape ([`InstallEvent`]), and
//!   * the shared interactive-login helper ([`run_login_command`]).
//!
//! The built-in per-CLI adapters live here as modules ([`bob`] / [`claude`]
//! / [`codex`]), re-exported as [`Bob`] / [`Claude`] / [`Codex`]. The
//! [`Registry`] is open: a third party adds their own provider by
//! implementing [`Harness`] in their crate and registering it — no fork.
//!
//! Wire shapes derive `Serialize` so every transport emits identical
//! JSON — keep their field names stable; the TypeScript front-end
//! consumes them verbatim.

pub mod events;
pub mod harness;

pub use events::{
    normalize_process_event, ByteRange, ParsedLine, RunEvent, SuggestedEdit, ToolCallEnd,
    ToolCallStart,
};
pub use harness::{
    run_login_command, CredentialSpec, Harness, HarnessCapabilities, HarnessInfo, HarnessModel,
    HarnessReadiness, InstallCallback, ReasoningEffort, RunCallback, RunControl, RunHandle, RunMode,
    RunRequest, RunTuning,
};
// The generic subprocess engine + the install/process event shapes live in
// the `cli-stream` leaf; re-export them so adapters + consumers reach them
// through the framework (e.g. `use harness::spawn_streaming`).
pub use cli_stream::{
    augmented_node_path, spawn_streaming, InstallEvent, ProcessEvent, ProcessHandle,
};

#[cfg(feature = "bob")]
pub mod bob;
#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "codex")]
pub mod codex;
pub mod registry;

// The built-in adapters, re-exported as short names so consumers write
// `use harness::{Bob, Claude, Codex}` — each gated behind its feature.
#[cfg(feature = "bob")]
pub use bob::{normalize_bob_event, BobHarness as Bob, BOB_HARNESS_ID};
#[cfg(feature = "claude")]
pub use claude::{ClaudeHarness as Claude, CLAUDE_HARNESS_ID};
#[cfg(feature = "codex")]
pub use codex::{CodexHarness as Codex, CODEX_HARNESS_ID};
// The open registry + convenience constructors over the built-ins.
pub use registry::{
    default_registry, harness_by_id, harness_catalog, Registry, DEFAULT_HARNESS_ID,
};
