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

## License

Licensed under either of MIT or Apache-2.0 at your option.
