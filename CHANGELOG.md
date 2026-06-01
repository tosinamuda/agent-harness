# Changelog

Notable changes to this workspace — `cli-stream`, `bob-rs`, `agent-harness` —
recorded together (they're versioned in lockstep at `0.1.0`). Format loosely
follows [Keep a Changelog](https://keepachangelog.com). Nothing is published to
crates.io yet, so everything lives under **Unreleased** until the first release.

## [Unreleased]

### Added
- **Typed errors.** `agent-harness`'s public API (`Harness::{install,run,login}`,
  `RunControl::cancel`, `run_login_command`) returns `HarnessError`
  (`Spawn`/`Install`/`Login`/`Cancel`/`Other`) instead of `String`, so consumers
  can branch on the kind of failure.
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
- A local quality gate, `scripts/check.sh` (fmt + clippy + test + build), and a
  `deny.toml` for `cargo deny`.

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
