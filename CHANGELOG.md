# Changelog

Notable changes to this workspace — `cli-stream`, `bob-rs`, `agent-harness` —
recorded together (they're versioned in lockstep). Format loosely follows
[Keep a Changelog](https://keepachangelog.com). All three are on crates.io;
unreleased changes accumulate under **Unreleased** until the next release.

## [Unreleased]

## [0.3.2] - 2026-06-11

### Added
- **`RunRequest.resume` — continue a prior CLI session.** A host can pass the
  session id captured from an earlier run's init (`RunEvent::Session` /
  `SessionInfo`) to **resume that conversation** instead of replaying a
  transcript in the prompt, so the CLI supplies the history (full fidelity,
  fewer tokens). **All three adapters honor it uniformly**, the same way they do
  `extra_args`: Claude maps it to `--resume <id>`; Codex restructures to
  `codex exec resume <id> … <prompt>` (the id is a positional before the prompt,
  `--json`/`--skip-git-repo-check` still apply); bob threads it through
  `RunBobOptions.resume` into `--resume <id>` (bob accepts the session UUID).
  `None` → a fresh session. Additive: the field defaults `None`, so every
  existing caller is unaffected (set `resume: None`).
- **`RunEvent::AskQuestion` — neutral interactive-question event.** When an agent
  asks the user a multiple-choice question (Claude's `AskUserQuestion`), the
  adapter maps it onto `AskQuestion { run_id, request_id, questions }` carrying
  neutral `Question` / `QuestionOption` types, so a host renders chips without
  name-checking a harness's tool — the way `ToolKind` already neutralizes tool
  names. The Claude adapter emits it today; the answer travels back as the user's
  **next chat message** (the host's existing send-path resumes the session), so
  this is one event, no new control channel: `RunControl` is unchanged and no
  stdin write-back is involved. The enum is `#[non_exhaustive]`, so the new
  variant doesn't break consumers with a `_` arm.

### Changed
- **bob runs direct-write (`auto_edit`) — no more previewable-edit proposals.**
  The bob adapter now reports `previews_edits: false` and maps `RunMode::Edit` to
  `BobApprovalMode::AutoEdit`, so bob writes files directly like Claude/Codex and
  the host reviews via its own edit gate (snapshot/clone) rather than an in-stream
  preview. In Edit mode the adapter also **suppresses bob's `SuggestedEdits`** — an
  applied write is not a proposal, so it surfaces as a file-op (the
  `write_to_file` ToolStart/ToolEnd) instead. A host that branched on
  `previews_edits` now treats bob uniformly with the other write-capable harnesses,
  with no id checks. (Ask mode is read-only and unchanged.)

## [0.3.1] - 2026-06-10

### Added
- **Host-controlled CLI args via `RunTuning.extra_args`.** A host can pass raw
  flags, appended after an adapter's own argv — to add a flag (`--settings`,
  `--add-dir`) or set one the adapter otherwise defaults — without editing the
  adapter. Crucially, the adapter's defaults are *defaults, not fixed*: the
  Claude adapter omits its own `--permission-mode acceptEdits` when the host
  sets `--permission-mode` through `extra_args`, so the host fully owns the flag
  (a sensible default exists, but it's cleanly overridable — no duplicate).
  **All three adapters honor it uniformly**, so a client applies a flag the same
  way regardless of harness: Claude appends at the end of its argv; Codex before
  its trailing positional prompt; bob threads them through the new
  `RunBobOptions.extra_args` (bob-rs) into its own argv. Keeps run *policy* on
  the host: a fully-headless host that needs Bash/skills to run without an
  unanswerable permission prompt passes `--permission-mode bypassPermissions`.
  Additive: the field defaults empty, so every existing caller is unaffected.

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
