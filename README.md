# agent-harness

**Use existing agent CLIs — the Claude Code CLI, Codex, bob — programmatically
from Rust, behind one interface.**

Instead of shelling out to `claude -p …` / `codex exec …` yourself and
hand-parsing each tool's bespoke stream format, you drive them through one
`Harness` trait and consume a single normalized `RunEvent` stream — text,
reasoning ("thinking"), tool start/end, suggested edits, lifecycle — no
matter which agent CLI is running underneath.

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

```toml
[dependencies]
# Not yet on crates.io — track the repo directly for now:
agent-harness = { git = "https://github.com/tosinamuda/agent-harness" }
# (the library is imported as `harness`)
```

```rust
use harness::{Harness, Registry};

// built-in adapters (default features = bob, claude, codex)
let reg = Registry::new()
    .register(harness::Bob::new())
    .register(harness::Claude::new())
    .register(harness::Codex::new());

let claude = reg.by_id("claude").expect("registered");
let info = claude.info();
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
