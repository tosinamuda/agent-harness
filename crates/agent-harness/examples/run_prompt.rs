//! Run a prompt against a harness and stream the normalized events.
//!
//! `cargo run --example run_prompt`
//! (requires the `claude` CLI installed + signed in; swap `Claude` for
//! `Bob` / `Codex` to drive a different agent).

use std::sync::{mpsc::sync_channel, Arc};

use harness::{Claude, Harness, HarnessError, RunEvent, RunMode, RunRequest, RunTuning};

fn main() -> Result<(), HarnessError> {
    // Pick a harness. `Claude` drives the `claude` CLI (must be installed +
    // signed in). Swap for `harness::Bob::new()` or `harness::Codex::new()`.
    let claude = Claude::new();

    // `run()` returns immediately; events arrive on background threads, so
    // collect them over a channel. (The callback must be Send + Sync — a
    // `sync_channel` sender is.)
    let (tx, rx) = sync_channel::<RunEvent>(256);
    let on_event: harness::RunCallback = Arc::new(move |ev| {
        let _ = tx.send(ev);
    });

    let _handle = claude.run(
        RunRequest {
            run_id: "demo".into(),
            prompt: "In one sentence, what is a Markdown heading?".into(),
            cwd: None,                    // working dir for the agent's tool calls
            mode: RunMode::Ask,           // Ask = answer only; Edit = may edit files
            tuning: RunTuning::default(), // optional: model / effort / max_turns
        },
        on_event,
    )?; // keep `_handle` to `.cancel()`; dropping it does NOT stop the run

    // ONE normalized event stream, regardless of the backing CLI:
    for ev in rx {
        match ev {
            RunEvent::Text { delta, .. } => print!("{delta}"), // the answer
            RunEvent::Thinking { delta, .. } => eprint!("{delta}"), // model reasoning
            RunEvent::ToolStart { name, .. } => eprintln!("\n[tool] {name}"),
            RunEvent::Error { message, .. } => eprintln!("\n[error] {message}"),
            RunEvent::Exited { .. } => break,
            _ => {}
        }
    }
    Ok(())
}
