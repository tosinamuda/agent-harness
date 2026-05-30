# bob-rs

> **Unofficial.** A community Rust SDK for the `bob` agent CLI. Not
> affiliated with, sponsored by, or endorsed by the maintainers of bob.

A standalone Rust SDK for the **bob** agent CLI: detection
and readiness probing, streaming install, OS-keychain credential storage,
and spawning a `bob` run with its `--output-format stream-json` stream
piped back line-by-line.

No Tauri, no HTTP server, no harness abstraction — just the bob
integration logic, so it can be reused by any host. Exposing bob as a
`Harness` lives in the [`agent-harness`](../agent-harness) crate's `bob`
module (which wraps this SDK); this crate stays a clean, standalone SDK.

Key surface:

- `get_readiness()` → a `BobReadinessSnapshot` (installed? version? Node?
  auth configured?).
- `install_bob(cb)` → streams the bundled install script's progress.
- `spawn_bob(opts, run_id, cb)` / `spawn_bob_raw(...)` → spawn a run,
  streaming `ProcessEvent`s (from the [`cli-stream`](../cli-stream) engine)
  until exit; returns a `ProcessHandle` for cancellation.
- `read_api_key` / `write_api_key` / `resolve_api_key` → the bob API key
  in the OS keychain.

## Example — run bob with a prompt

```rust
use std::sync::mpsc::sync_channel;
use bob_rs::{spawn_bob, BobApprovalMode, BobChatMode, ProcessEvent, RunBobOptions};

fn main() -> Result<(), String> {
    // bob authenticates with an API key this SDK reads from the OS keychain;
    // store it once with `bob_rs::write_api_key(..)`. `bob` must be on PATH.
    let (tx, rx) = sync_channel::<ProcessEvent>(256);

    let _handle = spawn_bob(
        RunBobOptions {
            prompt: "List the files in this directory.".into(),
            chat_mode: BobChatMode::Ask,
            approval_mode: BobApprovalMode::Default,
            max_coins: 30,
            cwd: None,             // defaults to the current directory
            bob_executable: None,  // defaults to `bob` on PATH
        },
        "demo".into(),
        move |ev| { let _ = tx.send(ev); }, // FnMut + Send + Sync + Clone
    )?;

    // bob-rs streams bob's stdout RAW — one JSON object per line from its
    // `--output-format stream-json`. For a *normalized* event stream (text /
    // thinking / tool calls), use the `agent-harness` crate, whose `bob`
    // adapter parses these for you.
    for ev in rx {
        match ev {
            ProcessEvent::Stdout { line, .. }      => println!("{line}"),
            ProcessEvent::Stderr { line, .. }      => eprintln!("{line}"),
            ProcessEvent::Error  { message, .. }   => eprintln!("error: {message}"),
            ProcessEvent::Exited { exit_code, .. } => { eprintln!("(exit {exit_code:?})"); break; }
            ProcessEvent::Started { .. }           => {}
        }
    }
    Ok(())
}
```

## License

Licensed under either of MIT or Apache-2.0 at your option.
