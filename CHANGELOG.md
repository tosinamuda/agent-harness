# Changelog

Notable changes to this workspace — `cli-stream`, `bob-rs`, `agent-harness` —
recorded together (they're versioned in lockstep). Format loosely follows
[Keep a Changelog](https://keepachangelog.com). All three are on crates.io;
unreleased changes accumulate under **Unreleased** until the next release.

## [Unreleased]

### Added
- **Host-controlled CLI args via `RunTuning.extra_args`.** A host can append raw
  flags after an adapter's own argv — to add a flag (`--settings`, `--add-dir`)
  or override one the adapter already sets (last-wins, for a commander-style CLI
  like Claude Code) — without editing the adapter. The Claude adapter appends
  them at the end of its argv; the Codex adapter before its trailing positional
  prompt; the bob adapter spawns via `spawn_bob` and ignores them (no raw argv).
  The point is to keep run *policy* on the host: a fully-headless host that
  needs Bash/skills to run without an unanswerable permission prompt passes
  `--permission-mode bypassPermissions` through `extra_args` instead of the
  adapter hardcoding a mode. Additive: the field defaults empty, so every
  existing caller is unaffected.

## [0.3.0] - 2026-06-09

### Fixed
- **In-band harness failures now surface as `RunEvent::Error`.** `ParsedLine`
  gained an `error` field, and `run_events_from_parsed` — the single place a
  parsed stdout line becomes a `RunEvent` — emits `RunEvent::Error` when it's
  set. So a failure a harness reports *in its stdout stream* (not just a
  spawn/IO `ProcessEvent::Error`) now reaches the consumer. The codex adapter
  maps `codex exec --json`'s `turn.failed { error: { message } }` and
  `error { message }` lines to it: previously `turn.failed` was ignored outright
  and `error` was downgraded to a transient activity line, so a codex turn that
  failed mid-run (quota, context overflow, model error) produced no answer *and*
  no error — looking like the agent silently did nothing. Additive: parsers that
  don't set `error` are unaffected.

## [0.2.0] - 2026-06-09

### Added
- **Neutral `ToolKind` on `RunEvent::ToolStart`.** A cross-harness behaviour
  class (`Read` / `Write` / `Edit` / `Search` / `Execute` / `Other`) rides
  alongside the raw tool `name`, classified once per adapter where the wire
  format is already parsed, so a consumer can route by what a tool call *does*
  (a read → a context pill, an edit → a file-op card) without re-encoding each
  harness's native tool vocabulary (bob's `read_file`, Claude's `Read`, codex's
  `file_change`). The neutral class rides as `toolKind` on the wire — distinct
  from the `kind` event discriminator. Additive: the raw `name` / `tool_call_id`
  are unchanged, so a consumer that only reads those is unaffected.

## [0.1.0] - 2026-06-03

### Added
- **Typed errors, end to end.** Every crate's public API now returns a typed
  error carrying the real underlying source, not a flattened `String`:
  - `cli-stream` → `StreamError` (`Spawn` carries the spawn `io::Error`,
    `PipeNotCaptured`, `CancelLockPoisoned`).
  - `bob-rs` → `BobError` (`Io { context, source }`, `Keychain(keyring::Error)`,
    `Serialize(serde_json::Error)`, `Invalid`, `NoDataDir`, `Stream(StreamError)`).
  - `agent-harness` → `HarnessError`'s category variants (`Spawn`/`Install`/
    `Login`/`Cancel`) now carry a `BoxError` **source** instead of a `String`,
    so a consumer can `err.source().downcast_ref::<StreamError>()` /
    `::<BobError>()`. `Display` still flattens the source into the message, so a
    consumer that stringifies at a boundary (`.to_string()`) sees the same full
    text as before. `StreamError` and `BobError` are re-exported from
    `agent-harness` for downcasting; `HarnessError::{spawn,install,login,cancel}`
    constructors box any source.
- **`RunEvent` enrichment** — `Session` (id + model), `Usage` (input/output/total
  tokens), and tool `input`/`output`, populated by the bob/claude/codex parsers.
- **Headless auth.** `readiness()` reports authenticated when the CLI's API-key
  env var is set (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `BOBSHELL_API_KEY`) —
  so a container/CI run reports ready without an interactive browser login.
- **Login-shell PATH.** `cli-stream`'s `augmented_node_path()` resolves the
  user's real `PATH` via their login shell (finds nvm / pnpm / volta / asdf /
  Homebrew), cached once, with a hardcoded fallback.
- The harness-agnostic raw tier `parse_raw_line`, the open `Registry`, and the
  `custom_harness` example (compose your own harness from the published pieces).
- **`Harness::run_channel()`** — a provided method that starts a run and returns
  its `RunEvent`s on an `mpsc` receiver, so callers can `for ev in rx { … }`
  instead of hand-writing the `Arc::new(move |ev| tx.send(ev))` callback. The
  receiver hangs up on its own when the run ends. `run()` stays for push
  semantics (forwarding onto a Tauri Channel / SSE sink from the callback).
- A local quality gate, `scripts/check.sh` (clippy `-D warnings` + test + build +
  feature-gate builds + `cargo deny` when installed), and a `deny.toml`.
- **Testable docs + real-I/O coverage.** Runnable/`no_run` doctests on the
  headline APIs (`spawn_streaming`, `Harness::run_channel`, `HarnessError`,
  `Registry`) so the documented code can't drift from the API; a stub-process
  integration test (`tests/stub_run.rs`) that drives a real `sh` child through
  the full spawn → stream → normalize → channel/cancel path; and an
  env-passthrough engine test.

### Changed
- **`RunEvent` and `ProcessEvent` are `#[non_exhaustive]`** — new event kinds are
  additive (downstream matches carry a `_` arm), so future additions don't break
  consumers the way `Session`/`Usage` once did.
- The three adapters are uniform `<harness>/{mod,parser}.rs` modules.

### Fixed
- **`cli-stream::cancel()` now terminates a *running* child.** It previously held
  the child lock across a blocking `wait()`, so `cancel` couldn't send SIGTERM
  until the process exited on its own — "Stop" did nothing mid-run.
- No `unwrap`/`expect`/`panic!` remain in library (non-test) code; poisoned
  mutexes are recovered rather than panicked on.
- `augmented_node_path()` keeps only absolute `PATH` entries — a relative/empty
  entry (e.g. a direnv `node_modules/.bin`) can no longer run a planted binary
  from the spawn cwd.
- **`bob-rs` keychain now persists on Linux and Windows, not just macOS.** The
  `keyring` dependency was built with `apple-native` only, so on other platforms
  it fell back to a no-op store and silently dropped saved keys. It now selects a
  native backend per OS — Keychain (macOS), Credential Manager (Windows), Secret
  Service over D-Bus with a pure-Rust encrypted session (Linux). Headless Linux
  (no Secret Service daemon) is unaffected: the key comes from `BOBSHELL_API_KEY`.
