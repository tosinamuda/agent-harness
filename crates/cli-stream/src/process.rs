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
use std::sync::{Arc, Mutex};
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
    let mut parts: Vec<String> = Vec::new();
    if let Some(parent) = program.parent() {
        let parent_str = parent.display().to_string();
        if !parent_str.is_empty() {
            parts.push(parent_str);
        }
    }
    parts.push(augmented_node_path());
    parts.join(":")
}

/// A PATH that resolves Node-based CLIs (bob, claude, codex) even from a
/// process launched by Finder/Launchpad, which inherits only the minimal
/// launchd PATH (`/usr/bin:/bin:/usr/sbin:/sbin`).
///
/// Used both by the run path (which prepends the resolved binary's own
/// directory on top of this) and — crucially — by readiness probes that
/// locate `claude`/`codex` via a bare `Command::new(name)`. Without this,
/// the packaged `.app` reports installed CLIs as "not installed" because
/// their bin dir (nvm, ~/.local/bin, Homebrew) isn't on the launchd PATH.
/// The caller's existing PATH stays first as a fallback; added
/// directories never replace it.
pub fn augmented_node_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(existing) = std::env::var("PATH") {
        if !existing.is_empty() {
            parts.push(existing);
        }
    }
    // macOS defaults — covers Homebrew (Apple Silicon + Intel) and the
    // system bins a launchd process might otherwise lack entirely.
    parts.push("/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_owned());
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
    fn augmented_node_path_includes_macos_defaults() {
        // These must always be present so a launchd-spawned `.app` can
        // resolve Homebrew-installed CLIs and the system bins — this is the
        // fix for claude/codex being mis-reported as "not installed".
        let path = augmented_node_path();
        assert!(path.contains("/opt/homebrew/bin"), "missing Apple-Silicon Homebrew bin");
        assert!(path.contains("/usr/local/bin"), "missing Intel Homebrew / system bin");
        assert!(path.contains("/usr/bin"), "missing system bin");
    }

    #[test]
    fn augment_path_for_node_prepends_the_program_dir() {
        let combined = augment_path_for_node(Path::new("/Users/x/.nvm/versions/node/v22/bin/bob"));
        assert!(combined.starts_with("/Users/x/.nvm/versions/node/v22/bin:"));
        assert!(combined.contains("/opt/homebrew/bin"));
    }
}
