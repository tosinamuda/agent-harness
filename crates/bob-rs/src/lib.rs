//! Shared bob CLI integration logic.
//!
//! Two consumers:
//!   * `src-tauri` (production desktop runtime, via Tauri commands)
//!   * `bob-api`   (axum HTTP server used by browser-preview dev)
//!
//! Both call into this crate's public API directly. Streaming
//! operations (install, run_bob) take callback closures so each
//! consumer can adapt to its own transport — Tauri `Channel<T>`
//! on one side, axum SSE on the other.
//!
//! Wire shapes (`InstallEvent`, `BobReadinessSnapshot`, etc.) are
//! `#[derive(Serialize)]` so both transports emit identical JSON.
//! Keep their field names stable — the TypeScript front-end
//! consumes them verbatim and doesn't reach back through TS-→Rust
//! type generation today.

pub mod check;
pub mod install;
pub mod keychain;
pub mod run;

pub use check::{get_readiness, BobReadinessSnapshot};
pub use install::install_bob;
pub use keychain::{
    auth_source, delete_api_key, read_api_key, resolve_api_key, write_api_key, KeySource,
};
pub use run::{spawn_bob, spawn_bob_raw, BobApprovalMode, BobChatMode, RunBobOptions};
// The generic subprocess engine + install/process event shapes live in the
// `cli-stream` leaf; re-export them here so existing `bob_rs::…` paths in
// the hosts keep compiling unchanged. (bob-rs depends only on cli-stream —
// not on the harness framework — so it stays a standalone SDK.)
pub use cli_stream::{
    augmented_node_path, spawn_streaming, InstallEvent, ProcessEvent, ProcessHandle,
};

/// Bob's documented minimum Node.js version. Mirrored in
/// `scripts/install-bob.sh`'s `REQUIRED_NODE_MAJOR` default.
/// Bumping this string also requires updating the bob installer.
pub const BOB_MIN_NODE_VERSION: &str = "22.15.0";

/// Service + account keys used to identify the bob API-key entry in
/// the OS keychain. The service is `"bob"` (this is the bob SDK; the
/// slot belongs to the bob tool, not to any host product), the
/// account is bob's documented env var name. Both transports hit the
/// exact same slot — switching either string orphans stored keys.
pub const KEYCHAIN_SERVICE: &str = "bob";
pub const KEYCHAIN_ACCOUNT: &str = "BOBSHELL_API_KEY";
