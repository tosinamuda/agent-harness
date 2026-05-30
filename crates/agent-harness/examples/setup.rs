//! Make sure a harness is installed + signed in before you run it.
//!
//! `cargo run --example setup`
//!
//! `readiness()` reports whether the CLI is installed and authenticated;
//! `install()` installs it (npm for claude/codex; a bundled script for bob)
//! and `login()` runs the CLI's own OAuth (`claude auth login` / `codex
//! login`, which opens the browser). bob has no `login()` — store its API
//! key instead (see the `bob-rs` crate).

use std::sync::Arc;

use harness::{Claude, Harness, InstallEvent};

fn main() -> Result<(), String> {
    let claude = Claude::new();

    // A logger for the install/login progress stream.
    let log: harness::InstallCallback = Arc::new(|ev| match ev {
        InstallEvent::Step { text } => eprintln!("• {text}"),
        InstallEvent::Stdout { text } | InstallEvent::Stderr { text } => eprintln!("  {text}"),
        InstallEvent::Done { ok, .. } => eprintln!("done (ok={ok})"),
    });

    let r = claude.readiness();
    if !r.installed {
        claude.install(Arc::clone(&log))?; // npm i -g @anthropic-ai/claude-code
    }
    if !r.auth_configured {
        claude.login(log)?; // `claude auth login` — opens the browser
    }

    println!("ready: {}", claude.readiness().ready);
    Ok(())
}
