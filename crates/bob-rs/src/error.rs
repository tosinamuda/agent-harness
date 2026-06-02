//! Typed errors for the bob SDK.

/// Why a bob-rs operation (install, keychain I/O, spawn) failed.
///
/// Each variant carries the real underlying error as a source — an
/// [`std::io::Error`], a [`keyring::Error`], a [`serde_json::Error`], or a
/// [`cli_stream::StreamError`] — rather than a pre-formatted string, so a
/// consumer can inspect or downcast it (e.g. branch on `io::ErrorKind`, or
/// recognise a `keyring::Error::NoEntry`). `#[non_exhaustive]` so adding a
/// variant later isn't a breaking change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BobError {
    /// An OS-level I/O failure while running the installer — spooling the
    /// embedded script to a tempfile, setting its mode, spawning `bash`, or
    /// waiting on it. `context` names the step; `source` is the OS error.
    #[error("{context}: {source}")]
    Io {
        /// Which install step failed (e.g. `"spawn install script"`).
        context: &'static str,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A spawned install child didn't expose a piped stdout/stderr. Shouldn't
    /// happen given `Stdio::piped()`, but `Child`'s accessors return `Option`,
    /// so the case is represented rather than `unwrap`ped.
    #[error("install child {stream} pipe was not captured")]
    PipeNotCaptured {
        /// Which stream was missing — `"stdout"` or `"stderr"`.
        stream: &'static str,
    },

    /// The OS keychain rejected a read / write / delete of the API key.
    #[error("keychain access failed: {0}")]
    Keychain(#[from] keyring::Error),

    /// Serializing the on-disk auth-state marker failed.
    #[error("auth-state serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),

    /// A caller-supplied argument was invalid (e.g. an empty API key).
    #[error("{0}")]
    Invalid(String),

    /// The platform application-data directory couldn't be resolved, so the
    /// auth-state marker has nowhere to live.
    #[error("could not determine the application data directory")]
    NoDataDir,

    /// The subprocess engine failed to spawn or cancel bob.
    #[error(transparent)]
    Stream(#[from] cli_stream::StreamError),
}
