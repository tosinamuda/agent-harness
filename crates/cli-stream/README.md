# cli-stream

A small, generic **streaming subprocess engine** for Rust: spawn a CLI,
stream its stdout/stderr line-by-line through a callback, cancel it
(SIGTERM → SIGKILL), and augment `PATH` so a Node-based CLI (or `node`
itself) resolves even from a Finder-launched macOS `.app`.

No agent / harness *protocol* knowledge — just process streaming, useful to
anyone driving a child CLI. (The one node-specific concession is the PATH
resolver below; as the shared leaf it's the one place the bob/claude/codex
adapters can reuse it without a dependency cycle.)

- `spawn_streaming(program, args, env, cwd, run_id, callback)` → returns a
  `ProcessHandle` (or a typed `StreamError` — `Spawn` carries the underlying
  `io::Error`, so you can tell "not on PATH" from "permission denied"); emits
  `ProcessEvent`s (Started / Stdout / Stderr / Error / Exited) to the callback
  from reader threads.
- `ProcessHandle::cancel()` — SIGTERM, then SIGKILL after a grace period.
  Polls `try_wait` (not a blocking `wait` under the lock), so it actually
  terminates a *running* child, not just on the next event.
- `augmented_node_path()` — resolves the user's real `PATH` by asking their
  login shell (so it finds `node` wherever nvm / pnpm / volta / asdf / Homebrew
  put it), cached once, with a hardcoded fallback — so a Finder-launched `.app`
  finds `node` instead of mis-reporting installed CLIs as "not installed".
- `InstallEvent` — the sibling shape for streamed install/login output.

## License

Licensed under either of MIT or Apache-2.0 at your option.
