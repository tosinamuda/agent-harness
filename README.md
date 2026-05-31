# agent-harness

**Use existing agent CLIs — the Claude Code CLI, Codex, bob — programmatically
from Rust, behind one interface.**

Instead of shelling out to `claude -p …` / `codex exec …` yourself and
hand-parsing each tool's bespoke stream format, you drive them through one
`Harness` trait and consume a single normalized `RunEvent` stream — text,
reasoning ("thinking"), tool start/end (with input/output), session + token
usage, suggested edits, lifecycle — no matter which agent CLI is running
underneath.

> "Harness" as in: you put a harness on an existing thing to *drive* it.
> This doesn't build an agent; it gives you one uniform, **programmatic**
> way to run the agent CLIs you already have installed and stream their
> output.

## Crates

| crate | what it is |
|---|---|
| [`agent-harness`](crates/agent-harness) | the framework — the `Harness` trait, the normalized `RunEvent` stream, an open `Registry`, and feature-gated `bob` / `claude` / `codex` adapters. Imported as **`harness`**. |
| [`bob-rs`](crates/bob-rs) | an **unofficial** Rust SDK for the `bob` agent CLI (detection, install, OS keychain, spawn). Not affiliated with bob. |
| [`cli-stream`](crates/cli-stream) | a generic streaming-subprocess engine — spawn a CLI, stream its stdout/stderr line-by-line, cancel it, augment `PATH` for packaged apps. Useful on its own. |

The dependency arrow points up: `cli-stream` ← `bob-rs` ← `agent-harness`
(the bob adapter wraps `bob-rs`; claude/codex use `cli-stream` directly).

## Quick start

### 1. Add the dependency

```toml
[dependencies]
# Not yet on crates.io — track the repo directly for now:
agent-harness = { git = "https://github.com/tosinamuda/agent-harness" }
# (the library is imported as `harness`)
```

### 2. Install & sign in the CLI you'll drive

A harness drives an agent CLI that must be on `PATH` and authenticated — but
you don't have to do that by hand. `readiness()` reports `installed` /
`auth_configured`; `install()` installs the CLI (npm for claude/codex, a
bundled script for bob); `login()` runs the CLI's own OAuth. Full,
compile-checked version in
[`examples/setup.rs`](crates/agent-harness/examples/setup.rs):

```rust
let h = harness::Claude::new();
let r = h.readiness();                                 // installed? signed in?
if !r.installed       { h.install(log.clone())?; }     // npm i -g @anthropic-ai/claude-code
if !r.auth_configured { h.login(log)?; }               // `claude auth login` (opens browser)
```

> **Auth is per-CLI.** `claude` / `codex` manage their own login; **bob** has
> no `login()` — it reads `BOBSHELL_API_KEY` from the environment, else the OS
> keychain (see [`bob-rs`](crates/bob-rs)).

### 3. Run a prompt

Give it a prompt and stream the answer — same code whichever CLI runs underneath:

```rust
use std::sync::{mpsc::sync_channel, Arc};
use harness::{Claude, Harness, RunEvent, RunMode, RunRequest, RunTuning};

fn main() -> Result<(), String> {
    // Pick a harness. `Claude` drives the `claude` CLI (must be installed +
    // signed in). Swap for `harness::Bob::new()` or `harness::Codex::new()`.
    let claude = Claude::new();

    // `run()` returns immediately; events arrive on background threads, so
    // collect them over a channel. (The callback must be Send + Sync — a
    // `sync_channel` sender is.)
    let (tx, rx) = sync_channel::<RunEvent>(256);
    let on_event: harness::RunCallback = Arc::new(move |ev| { let _ = tx.send(ev); });

    let _handle = claude.run(
        RunRequest {
            run_id: "demo".into(),
            prompt: "In one sentence, what is a Markdown heading?".into(),
            cwd: None,                     // working dir for the agent's tool calls
            mode: RunMode::Ask,            // Ask = answer only; Edit = may edit files
            tuning: RunTuning::default(),  // optional: model / effort / max_turns
        },
        on_event,
    )?; // keep `_handle` to `.cancel()`; dropping it does NOT stop the run

    // ONE normalized event stream, regardless of the backing CLI:
    for ev in rx {
        match ev {
            RunEvent::Text { delta, .. }     => print!("{delta}"),        // the answer
            RunEvent::Thinking { delta, .. } => eprint!("{delta}"),       // model reasoning
            RunEvent::ToolStart { name, .. } => eprintln!("\n[tool] {name}"),
            RunEvent::Error { message, .. }  => eprintln!("\n[error] {message}"),
            RunEvent::Exited { .. }          => break,
            _ => {}
        }
    }
    Ok(())
}
```

Prefer to pick a harness by string id (e.g. from a config field)? Use the registry:

```rust
let reg = harness::default_registry();          // the built-ins (per enabled features)
let h = reg.by_id("claude").expect("enabled");  // "bob" / "codex" / your own
let info = h.info();
```

### Bring your own provider

The point of the framework: implement `Harness` in **your own crate** and
register it — no fork required.

```rust
use harness::{Harness, Registry};

struct Acme;
impl Harness for Acme { /* info / readiness / install / run / credential */ }

let reg = Registry::new().register(Acme);
```

`Harness::run()` just emits a normalized `RunEvent` stream, so a provider
can spawn a CLI (via `cli-stream`) **or** call an HTTP API — there is no
CLI requirement in the trait.

## Features

Each built-in adapter is gated behind a Cargo feature
(`default = ["bob", "claude", "codex"]`). Want only the framework plus one
adapter — and none of bob's keychain/install weight?

```toml
agent-harness = { version = "0.1", default-features = false, features = ["claude"] }
```

## Status

Early, pre-1.0 — the API may change. Built for (and used by) the Compose
writing app, but designed to be usable standalone.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option. Contributions are dual-licensed under the same terms.

`bob-rs` is an **unofficial** community SDK and is not affiliated with,
sponsored by, or endorsed by the maintainers of bob.
