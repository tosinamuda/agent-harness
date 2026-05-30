//! Generic streaming subprocess engine.
//!
//! Spawn a child CLI, stream its stdout/stderr line-by-line through a
//! callback as [`ProcessEvent`]s, cancel it (SIGTERM → SIGKILL), and
//! augment `PATH` so Node-based CLIs resolve even from a Finder-launched
//! `.app`. No agent / harness knowledge — purely subprocess streaming,
//! useful to anyone driving a CLI.
//!
//! [`InstallEvent`] is the sibling shape for streamed install/login output.
//!
//! This is a deliberate *leaf* crate: both `bob-rs` (the bob SDK) and
//! `agent-harness` (the framework) depend on it, which is what lets
//! `bob-rs` stay standalone without a dependency cycle.

pub mod install;
pub mod process;

pub use install::InstallEvent;
pub use process::{augmented_node_path, spawn_streaming, ProcessEvent, ProcessHandle};
