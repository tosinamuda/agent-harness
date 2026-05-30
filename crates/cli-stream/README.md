# cli-stream

A small, generic **streaming subprocess engine** for Rust: spawn a CLI,
stream its stdout/stderr line-by-line through a callback, cancel it
(SIGTERM → SIGKILL), and augment `PATH` so a Node-based CLI (or `node`
itself) resolves even from a Finder-launched macOS `.app`.

No agent / LLM / harness knowledge — just process streaming, useful to
anyone driving a child CLI.

- `spawn_streaming(program, args, env, cwd, run_id, callback)` → returns a
  `ProcessHandle`; emits `ProcessEvent`s (Started / Stdout / Stderr / Error
  / Exited) to the callback from reader threads.
- `ProcessHandle::cancel()` — SIGTERM, then SIGKILL after a grace period.
- `augmented_node_path()` — the PATH-augmentation helper (nvm / Homebrew /
  official installers), so packaged apps find `node`.
- `InstallEvent` — the sibling shape for streamed install/login output.

## License

Licensed under either of MIT or Apache-2.0 at your option.
