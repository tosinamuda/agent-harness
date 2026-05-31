//! Generic streaming subprocess engine — the shared core behind every
//! process-backed harness (bob, Claude Code, Codex, …).
//!
//! Spawns a child, pipes stdout/stderr line-by-line through a callback
//! as [`ProcessEvent`]s, augments PATH so Node-based CLIs resolve even
//! from a Finder-launched `.app`, and hands back a [`ProcessHandle`] for
//! cancellation (SIGTERM → SIGKILL). No harness-trait or bob knowledge —
//! purely subprocess streaming.
//!
//! Cancellation is the wrinkle: a run needs to be stoppable mid-stream
//! when the user closes the tab or hits "stop". `ProcessHandle::cancel()`
//! sends SIGTERM (with a SIGKILL fallback) and flips an atomic
//! `cancelled` flag the reader threads use to short-circuit.

use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Raw events emitted to the caller's callback during a streaming run.
/// JSON-tagged so axum SSE and Tauri Channel render identical payloads
/// on the wire. Harness-neutral: a process-backed adapter parses the
/// `Stdout` lines into a normalized event vocabulary (e.g. `agent-harness`'s `RunEvent`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProcessEvent {
    /// First event. Sent before the child has produced any output so the
    /// UI can show a "thinking…" state.
    Started { run_id: String },
    /// Raw stdout line. Process-backed CLIs emit one JSON object per line
    /// in their streaming mode. The caller parses.
    Stdout { run_id: String, line: String },
    /// Raw stderr line. Warnings + the occasional error.
    Stderr { run_id: String, line: String },
    /// Spawn / IO failure. Terminal — followed by `Exited`.
    Error { run_id: String, message: String },
    /// Process exited. Always sent exactly once at the end.
    Exited {
        run_id: String,
        exit_code: Option<i32>,
        /// True iff `cancel()` was called before exit.
        cancelled: bool,
    },
}

/// Handle to an in-flight streaming run. Caller stores it (e.g. in a
/// runId-keyed map) so a later `cancel()` can find it.
///
/// Dropping the handle does NOT cancel the run — the reader threads +
/// wait thread continue independently. Use `cancel()` explicitly when
/// the user closes the connection.
#[derive(Clone)]
pub struct ProcessHandle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    child: Mutex<Option<Child>>,
    cancelled: AtomicBool,
}

impl ProcessHandle {
    /// SIGTERM the process, then SIGKILL after 1.5s if it's still alive.
    /// The CLI is supposed to flush a final result on SIGTERM but we
    /// don't trust it to do so forever.
    pub fn cancel(&self) -> Result<(), String> {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        let mut guard = self
            .inner
            .child
            .lock()
            .map_err(|e| format!("cancel lock: {e}"))?;
        let Some(child) = guard.as_mut() else {
            // Already exited.
            return Ok(());
        };
        // Best-effort SIGTERM. On Unix, kill() sends SIGKILL by default;
        // we use libc::kill for SIGTERM, falling back to child.kill() if
        // the libc call fails. On Windows there's only TerminateProcess
        // via .kill().
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            // SAFETY: pid is the child's PID owned by this Child; sending
            // SIGTERM is well-defined.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            // Spawn the SIGKILL fallback inline to avoid holding the mutex
            // while sleeping.
            let inner = Arc::clone(&self.inner);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(1500));
                if let Ok(mut guard) = inner.child.lock() {
                    if let Some(child) = guard.as_mut() {
                        let _ = child.kill();
                    }
                }
            });
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }
        Ok(())
    }

    /// Whether `cancel()` was called. Tagged on the final `Exited` event.
    pub fn was_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }
}

/// Spawn an arbitrary streaming child process — the generic engine behind
/// every process-backed harness (bob, Claude Code, Codex).
///
/// Pipes stdout/stderr line-by-line through `callback` using the raw
/// [`ProcessEvent`] vocabulary (Started / Stdout / Stderr / Error /
/// Exited). `env` supplies per-harness secrets (each harness's API-key
/// var, or none for self-authenticating CLIs). PATH is augmented so
/// Node-based CLIs find `node`. Returns a [`ProcessHandle`] for
/// cancellation.
///
/// `callback` is invoked from three threads (stdout reader, stderr
/// reader, exit watcher); the `Clone` bound lets us hand a copy to each.
/// `run_id` is opaque — the caller chooses it and uses it to correlate
/// events with the handle.
pub fn spawn_streaming<F>(
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: PathBuf,
    run_id: String,
    callback: F,
) -> Result<ProcessHandle, String>
where
    F: FnMut(ProcessEvent) + Send + Sync + Clone + 'static,
{
    // PATH augmentation: Node-based CLIs (bob, claude, codex) expect
    // `node` (and often `npm`, `git`) on PATH. A desktop app launched
    // from Finder/Launchpad inherits only the minimal launchd PATH
    // (`/usr/bin:/bin:/usr/sbin:/sbin`), so an nvm-installed node is
    // invisible and the child exits 127 ("command not found").
    //
    // Fix: prepend the program's parent dir (where node also lives in an
    // nvm install) to the child's PATH. Added, not replaced, so a PATH
    // the user explicitly set still wins on later lookups.
    let augmented_path = augment_path_for_node(&program);

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&cwd)
        .env("PATH", augmented_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", program.display()))?;

    let stdout = child.stdout.take().ok_or("child stdout was not captured")?;
    let stderr = child.stderr.take().ok_or("child stderr was not captured")?;

    let inner = Arc::new(HandleInner {
        child: Mutex::new(Some(child)),
        cancelled: AtomicBool::new(false),
    });
    let handle = ProcessHandle { inner: Arc::clone(&inner) };

    // Emit Started immediately so the caller doesn't wait on the first
    // output line for a UI signal.
    let mut started_cb = callback.clone();
    started_cb(ProcessEvent::Started { run_id: run_id.clone() });

    // Reader threads. Each owns its own callback clone — the Clone bound
    // is the whole point.
    let stdout_cb = callback.clone();
    let stdout_run_id = run_id.clone();
    let stdout_handle = thread::spawn(move || {
        pump_lines(stdout, stdout_run_id, true, stdout_cb);
    });

    let stderr_cb = callback.clone();
    let stderr_run_id = run_id.clone();
    let stderr_handle = thread::spawn(move || {
        pump_lines(stderr, stderr_run_id, false, stderr_cb);
    });

    // Exit watcher — waits on the child, joins the reader threads, then
    // emits the terminal Exited event with the cancellation flag.
    let exit_inner = Arc::clone(&inner);
    let mut exit_cb = callback;
    let exit_run_id = run_id;
    thread::spawn(move || {
        // Hold the lock only long enough to call wait(). Drop before
        // joining threads so cancel() can still acquire the lock.
        let wait_result = {
            let mut guard = exit_inner.child.lock().ok();
            guard.as_mut().and_then(|g| g.as_mut().map(|c| c.wait()))
        };
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        let cancelled = exit_inner.cancelled.load(Ordering::SeqCst);

        match wait_result {
            Some(Ok(status)) => exit_cb(ProcessEvent::Exited {
                run_id: exit_run_id.clone(),
                exit_code: status.code(),
                cancelled,
            }),
            Some(Err(err)) => exit_cb(ProcessEvent::Error {
                run_id: exit_run_id.clone(),
                message: format!("wait failed: {err}"),
            }),
            None => {}
        }

        // Drop the child handle so subsequent cancel() calls
        // short-circuit cleanly.
        if let Ok(mut guard) = exit_inner.child.lock() {
            *guard = None;
        }
    });

    Ok(handle)
}

fn pump_lines<R, F>(reader: R, run_id: String, is_stdout: bool, mut callback: F)
where
    R: Read,
    F: FnMut(ProcessEvent),
{
    let buffered = BufReader::new(reader);
    for line in buffered.lines() {
        match line {
            Ok(text) => {
                let event = if is_stdout {
                    ProcessEvent::Stdout { run_id: run_id.clone(), line: text }
                } else {
                    ProcessEvent::Stderr { run_id: run_id.clone(), line: text }
                };
                callback(event);
            }
            Err(err) => {
                callback(ProcessEvent::Error {
                    run_id: run_id.clone(),
                    message: format!("stream read failed: {err}"),
                });
                return;
            }
        }
    }
}

/// Compose a PATH for the spawned process that always includes the
/// directory containing the program — where `node`, `npm`, and friends
/// usually live in an nvm install. The user's existing PATH stays as a
/// fallback after our prepended directory.
fn augment_path_for_node(program: &Path) -> String {
    prepend_program_dir(program, &augmented_node_path())
}

/// Prepend the directory containing `program` (where `node` also lives in an
/// nvm install) to `base_path`, so the resolved binary's own dir is searched
/// first. Pure (no env / no spawn) so it's unit-tested directly.
fn prepend_program_dir(program: &Path, base_path: &str) -> String {
    match program
        .parent()
        .map(|p| p.display().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(dir) => format!("{dir}:{base_path}"),
        None => base_path.to_owned(),
    }
}

/// A PATH that resolves Node-based CLIs (bob, claude, codex) even from a
/// process launched by Finder/Launchpad, which inherits only the minimal
/// launchd PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) rather than the user's
/// shell PATH.
///
/// Strategy: keep the process's own PATH first (an explicit PATH still wins),
/// then append the user's **real** PATH as resolved by their login shell —
/// which sources their rc, so it knows where nvm / pnpm / volta / asdf / fnm /
/// Homebrew put `node`, with no guessing. If the shell query is unavailable
/// (no `$SHELL`, a timeout, a sandboxed app that can't spawn, …) we fall back
/// to a hardcoded best-effort list, so we're never worse than before.
///
/// Used by the run path (which prepends the resolved binary's own dir on top
/// of this) and by readiness probes that locate `claude`/`codex` via a bare
/// `Command::new(name)`. Computed once and cached for the process — the
/// (bounded) shell spawn happens at most once per launch, lazily on the first
/// readiness/run/login, never at construction.
pub fn augmented_node_path() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(compute_augmented_node_path).clone()
}

fn compute_augmented_node_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    // The process's own PATH first — anything explicitly set still wins.
    if let Ok(existing) = std::env::var("PATH") {
        if !existing.is_empty() {
            parts.push(existing);
        }
    }
    // The user's real PATH (nvm/pnpm/volta/asdf/Homebrew) via their login
    // shell; a hardcoded best-effort list if that's unavailable.
    parts.push(login_shell_path().unwrap_or_else(hardcoded_node_dirs));
    parts.join(":")
}

/// Resolve PATH by asking the user's login + interactive shell — it sources
/// their rc, so it knows wherever any node manager (nvm / pnpm / volta / asdf /
/// fnm / Homebrew) put `node`, without us guessing. Bounded by a timeout so a
/// slow or interactive rc can't hang us; returns `None` (→ hardcoded fallback)
/// on any failure: no `$SHELL`, spawn refused (e.g. a sandboxed app), timeout,
/// or no PATH in the output. Reads PATH from `env` (OS colon format,
/// shell-agnostic — works for fish too) rather than expanding `$PATH`.
///
/// This *executes the user's shell rc*, exactly as opening a terminal does —
/// their own shell, on their own machine. It is not a privilege/auth step: no
/// "login session" is created; `-l`/`-i` only select which startup files are
/// sourced (login profiles + the interactive rc where nvm usually lives).
/// Printed on its own line right before `env`, so the parser can skip any
/// shell-init chatter / terminal escape sequences (e.g. iTerm2 shell
/// integration's `]1337;…` OSC codes) the interactive shell emits before our
/// command runs — which would otherwise prepend to the `PATH=` line.
const PATH_SENTINEL: &str = "__CLI_STREAM_PATH__";

#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;
    // Print a sentinel line, then dump the environment. Reading PATH from `env`
    // (not by expanding `$PATH`) keeps it OS colon format and shell-agnostic
    // (fish stores PATH as a list); the sentinel lets the parser ignore
    // anything the interactive shell prints at startup before `env` runs.
    let script = format!("printf '\\n{PATH_SENTINEL}\\n'; env");
    let mut child = Command::new(&shell)
        .arg("-lic") // -l: login profiles, -i: interactive rc (nvm), -c: command
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on a worker thread so the whole query can be bounded by a timeout —
    // a misbehaving rc must not hang the app.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let output = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(buf) => buf,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let _ = child.wait();
    parse_path_from_shell_output(&output)
}

#[cfg(not(unix))]
fn login_shell_path() -> Option<String> {
    None
}

/// Extract the `PATH=…` value from the shell's `printf <sentinel>; env` output.
/// Everything up to (and including) the last sentinel is discarded — that's
/// where shell-init chatter and terminal escape sequences live — then the
/// `PATH=` line is read from the clean `env` dump that follows. `None` if the
/// sentinel is missing (query misbehaved) or PATH is absent/empty.
fn parse_path_from_shell_output(output: &str) -> Option<String> {
    output
        .rsplit_once(PATH_SENTINEL)?
        .1
        .lines()
        .find_map(|line| line.strip_prefix("PATH="))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
}

/// Hardcoded best-effort node locations — the fallback when the login-shell
/// query is unavailable. Covers Homebrew (both arches), the system bins, the
/// official-installer dir, and any nvm-managed node. Misses pnpm/volta/asdf —
/// that's what the shell query is for — but never makes things worse than the
/// bare launchd PATH.
fn hardcoded_node_dirs() -> String {
    let mut parts: Vec<String> =
        vec!["/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_owned()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let home_path = Path::new(&home);
            // Official-installer location for several agent CLIs.
            parts.push(home_path.join(".local/bin").display().to_string());
            // nvm: ~/.nvm/versions/node/<version>/bin — where npm-global
            // CLIs (bob, claude, codex) live under an nvm-managed node.
            if let Ok(entries) = std::fs::read_dir(home_path.join(".nvm/versions/node")) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin");
                    if bin.is_dir() {
                        parts.push(bin.display().to_string());
                    }
                }
            }
        }
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardcoded_fallback_includes_macos_defaults() {
        // The fallback (used when the login-shell query is unavailable) must
        // still carry Homebrew + the system bins, so a launchd-spawned `.app`
        // resolves CLIs even without a usable shell — the original
        // "not installed" fix.
        let path = hardcoded_node_dirs();
        assert!(path.contains("/opt/homebrew/bin"), "missing Apple-Silicon Homebrew bin");
        assert!(path.contains("/usr/local/bin"), "missing Intel Homebrew / system bin");
        assert!(path.contains("/usr/bin"), "missing system bin");
    }

    #[test]
    fn parse_path_from_shell_output_skips_chatter_before_the_sentinel() {
        // Real-world shape: iTerm2 OSC escapes + a banner emitted at shell
        // startup, BEFORE our sentinel + `env` dump. Only the post-sentinel
        // PATH= line counts — note the pre-sentinel "PATH=/decoy" is ignored.
        let output = "\u{1b}]1337;RemoteHost=x\u{7}welcome banner\nPATH=/decoy\n__CLI_STREAM_PATH__\nHOME=/Users/x\nPATH=/opt/homebrew/bin:/usr/bin\nLANG=en_US";
        assert_eq!(
            parse_path_from_shell_output(output).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        // No sentinel (query misbehaved) → None, so the caller falls back —
        // even if a bare PATH= is present.
        assert_eq!(parse_path_from_shell_output("PATH=/usr/bin"), None);
        // Sentinel present but PATH absent/empty → None.
        assert_eq!(parse_path_from_shell_output("__CLI_STREAM_PATH__\nFOO=bar"), None);
        assert_eq!(parse_path_from_shell_output("__CLI_STREAM_PATH__\nPATH=\nFOO=bar"), None);
    }

    #[test]
    fn prepend_program_dir_puts_the_binary_dir_first() {
        let combined = prepend_program_dir(
            Path::new("/Users/x/.nvm/versions/node/v22/bin/bob"),
            "/opt/homebrew/bin:/usr/bin",
        );
        assert!(combined.starts_with("/Users/x/.nvm/versions/node/v22/bin:"));
        assert!(combined.contains("/opt/homebrew/bin"));
        // A bare program name has no parent dir → base path unchanged.
        assert_eq!(prepend_program_dir(Path::new("bob"), "/usr/bin"), "/usr/bin");
    }

    #[test]
    fn augmented_node_path_is_nonempty_and_resolves_system_bin() {
        // Exercises the cached public path once. `/usr/bin` is present whether
        // the shell query succeeds (real PATH) or falls back (hardcoded), and
        // is on the bare launchd PATH too — so this holds in any environment.
        let path = augmented_node_path();
        assert!(!path.is_empty());
        assert!(path.contains("/usr/bin"), "system bin must always resolve");
    }
}
