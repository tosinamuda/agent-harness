# agent-harness

A Rust framework for **driving agent CLIs — or building your own** — behind
one interface. Imported as `harness`:

```rust
use harness::{Harness, Registry, Bob, Claude};

let reg = Registry::new()
    .register(Bob::new())
    .register(Claude::new())
    .register(MyCustomHarness::new());   // your own impl Harness — no fork

let claude = reg.by_id("claude").unwrap();
// claude.run(request, on_event)? streams normalized RunEvents
```

## What it gives you

- **The `Harness` trait** — `info` / `readiness` / `install` / `run` /
  `credential` / `login`. Object-safe (`Box<dyn Harness>`). `run()` only
  emits `RunEvent`s, so an implementor can spawn a CLI **or** call an HTTP
  API — CLI-backed and API-backed providers both fit.
- **A normalized event vocabulary** — `RunEvent` (text, thinking, tool
  start/end, suggested edits, activity, lifecycle) + `ParsedLine` +
  `normalize_process_event`, the skeleton every process-backed adapter shares.
- **An open `Registry`** — compose the built-ins and/or your own custom
  providers; nothing to fork to add one.
- The streaming engine (`spawn_streaming` / `ProcessEvent` / `ProcessHandle`
  / PATH augmentation) is re-exported from the [`cli-stream`](../cli-stream) leaf.

## Built-in adapters (feature-gated)

`Bob` / `Claude` / `Codex` wrap the bob / Claude Code / Codex CLIs and live
as the `bob` / `claude` / `codex` modules, enabled by default:

```toml
agent-harness = "0.1"                                                            # all three
agent-harness = { version = "0.1", default-features = false, features = ["claude"] }  # just Claude — no bob/keyring
agent-harness = { version = "0.1", default-features = false }                    # the lean framework only
```

The `bob` feature pulls in the unofficial [`bob-rs`](../bob-rs) SDK (keychain
/ installer); `claude`/`codex` own their CLI's login, so they add no deps.

Wire shapes derive `Serialize` with stable field names, so any transport
(HTTP/SSE, an IPC channel) emits identical JSON.

## License

Licensed under either of MIT or Apache-2.0 at your option.
