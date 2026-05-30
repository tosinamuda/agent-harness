//! OS-keychain storage for the Bob API key.
//!
//! Two-layer design:
//!
//!   1. **The key value** lives in the OS keychain (macOS Keychain
//!      Access / libsecret / Windows Credential Vault). Reads
//!      require the OS to confirm the calling app has permission
//!      — which on macOS triggers a user password prompt the first
//!      time a new binary (or any process that didn't create the
//!      entry) touches the item. That prompt is acceptable when
//!      it's triggered by a deliberate user action (the user just
//!      clicked "Save key" or "Send chat"). It is **not**
//!      acceptable at app boot.
//!
//!   2. **A marker file** at `<app-data-dir>/auth_state.json`
//!      records whether the user has *ever* saved a key. The file
//!      contains no secret material — just `{"hasKey": true,
//!      "source": "keychain"|"env"}`. Reading it is a plain
//!      `fs::read`; no OS prompt. The boot-time readiness check
//!      uses this to decide whether to show "API key not
//!      connected" without touching the keychain.
//!
//! This separation is what keeps the **app launch path
//! prompt-free**. The keychain prompt now fires only when the
//! user takes an explicit action (saving a key, running bob).
//!
//! Lookup precedence for the actual value (`resolve_api_key`):
//!   1. `BOBSHELL_API_KEY` env var (`.env` or shell-exported)
//!   2. OS keychain entry
//!
//! Writes always go to BOTH the keychain (value) and the marker
//! file (existence flag). Deletes clear both.

use crate::{KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    Env,
    Keychain,
}

/// On-disk shape of the marker file. Intentionally minimal — only
/// the fields needed to drive the readiness UI without touching
/// the keychain. Versioned via `schema` so future fields don't
/// break older readers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthState {
    /// Bump if we add fields that older binaries can't ignore.
    #[serde(default = "default_schema")]
    schema: u32,
    /// True iff the user has saved a key via `write_api_key`.
    /// `.env` keys do NOT set this to true (env is the dev
    /// override; we let the env detection happen at read time).
    #[serde(default)]
    has_keychain_key: bool,
}

fn default_schema() -> u32 {
    1
}

/// Filesystem location for the marker file. Uses the platform's
/// standard app-data dir under the Compose bundle id so the
/// file lives next to the Tauri-managed sqlite, logs, etc.
fn auth_state_path() -> Option<PathBuf> {
    // `dirs::data_dir()` returns:
    //   * macOS: ~/Library/Application Support
    //   * Linux: $XDG_DATA_HOME or ~/.local/share
    //   * Windows: %APPDATA%
    // The bundle id matches `tauri.conf.json::identifier`.
    Some(dirs::data_dir()?.join("com.compose.app").join("auth_state.json"))
}

fn read_auth_state() -> AuthState {
    let Some(path) = auth_state_path() else {
        return AuthState::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return AuthState::default();
    };
    serde_json::from_slice::<AuthState>(&bytes).unwrap_or_default()
}

fn write_auth_state(state: &AuthState) -> Result<(), String> {
    let path = auth_state_path().ok_or("could not determine data directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

/// Process-wide cache of the resolved key. Populated on first
/// successful keychain read so subsequent `resolve_api_key()`
/// calls never re-touch the keychain (and therefore never re-
/// trigger the macOS "Allow this app to access ..." prompt when
/// the binary identity isn't yet on the entry's ACL).
///
/// Invariants:
///   * Cleared on `write_api_key` (user saved a new value)
///   * Cleared on `delete_api_key` (user disconnected)
///   * Lives for the duration of the process — restarting the
///     app re-reads the keychain (which is fine: the app's own
///     code-signing identity is then the entry creator, so no
///     prompt).
static KEY_CACHE: Mutex<Option<(String, KeySource)>> = Mutex::new(None);

fn cache_read() -> Option<(String, KeySource)> {
    KEY_CACHE.lock().ok().and_then(|guard| guard.clone())
}

fn cache_write(value: String, source: KeySource) {
    if let Ok(mut guard) = KEY_CACHE.lock() {
        *guard = Some((value, source));
    }
}

fn cache_clear() {
    if let Ok(mut guard) = KEY_CACHE.lock() {
        *guard = None;
    }
}

/// Cheap existence + source probe. Reads the marker file and the
/// env var; never touches the OS keychain. Use this at app boot.
///
/// Returns `None` if no key is configured anywhere.
pub fn auth_source() -> Option<KeySource> {
    // Env wins — that's the dev override and it never prompts.
    if let Ok(value) = std::env::var("BOBSHELL_API_KEY") {
        if !value.is_empty() {
            return Some(KeySource::Env);
        }
    }
    if read_auth_state().has_keychain_key {
        Some(KeySource::Keychain)
    } else {
        None
    }
}

/// Read the API key from the OS keychain. **This triggers the
/// macOS user-confirmation prompt** the first time a non-creator
/// process accesses the entry — only call when you actually need
/// the value (saving, running bob), never at boot.
///
/// Returns `None` if no entry exists or the keychain rejects the
/// read (locked, missing libsecret).
pub fn read_api_key() -> Option<String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).ok()?;
    match entry.get_password() {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Write the API key to the OS keychain AND set the marker file.
/// The keychain write is what triggers a one-time macOS prompt
/// the first time the user saves a key; subsequent overwrites by
/// the same app are silent.
pub fn write_api_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("API key must be non-empty".to_owned());
    }
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| e.to_string())?;
    entry.set_password(key).map_err(|e| e.to_string())?;
    // Mark the existence so future boots don't need to touch the
    // keychain to know the user has a key.
    write_auth_state(&AuthState {
        schema: 1,
        has_keychain_key: true,
    })?;
    // Cache the freshly-written value so the next bob spawn
    // doesn't pay another keychain prompt. The user just gave
    // us this exact byte string — we can trust it.
    cache_write(key.to_owned(), KeySource::Keychain);
    Ok(())
}

/// Remove the keychain entry and clear the marker. No-op for
/// each step if the corresponding state isn't there.
pub fn delete_api_key() -> Result<(), String> {
    if let Ok(entry) = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    write_auth_state(&AuthState {
        schema: 1,
        has_keychain_key: false,
    })
    .ok();
    // Clear the in-memory cache so the next bob spawn realises
    // the user disconnected (and we surface "API key not
    // configured" rather than the now-stale cached value).
    cache_clear();
    Ok(())
}

/// Resolve the effective key with env-then-keychain precedence.
/// Used by `run::spawn_bob` to inject the key into the child env.
///
/// The first successful resolution is **cached for the lifetime
/// of the process** so a flurry of bob runs (or just rapid
/// successive Send clicks) only ever triggers at most one OS
/// keychain prompt per session, no matter how many times this
/// function is called. The cache invalidates whenever the user
/// saves a new key or disconnects — see `write_api_key` /
/// `delete_api_key`.
pub fn resolve_api_key() -> Option<(String, KeySource)> {
    if let Some(cached) = cache_read() {
        return Some(cached);
    }
    // Env wins — never hits the keychain in this branch.
    if let Ok(value) = std::env::var("BOBSHELL_API_KEY") {
        if !value.is_empty() {
            cache_write(value.clone(), KeySource::Env);
            return Some((value, KeySource::Env));
        }
    }
    // Keychain read — this is the path that may prompt the OS
    // user on first access by a new binary identity. We do it
    // at most once per process thanks to the cache above.
    let value = read_api_key()?;
    cache_write(value.clone(), KeySource::Keychain);
    Some((value, KeySource::Keychain))
}
