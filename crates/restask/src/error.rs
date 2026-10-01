//! Crate-wide error taxonomy (§12.1). Introduced in T04 because the domain spec (§3.4)
//! requires it; every later fallible API returns `Result<_, RestaskError>`.

/// Crate-wide error type (§12.1).
#[derive(Debug, thiserror::Error)]
pub enum RestaskError {
    /// A configuration file is invalid.
    #[error("config invalid: {path}: {reason}")]
    Config {
        /// Path of the offending config file.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
    /// Parsing a vault file failed.
    #[error("parse error in {path} at byte {offset}: {reason}")]
    Parse {
        /// File that failed to parse.
        path: String,
        /// Byte offset of the failure.
        offset: usize,
        /// What failed to parse.
        reason: String,
    },
    /// The same UID is claimed by two routed lines.
    #[error("uid conflict: {uid} claimed by {a} and {b}")]
    UidConflict {
        /// The duplicated UID.
        uid: crate::domain::TaskUid,
        /// First claimant (path:line).
        a: String,
        /// Second claimant (path:line).
        b: String,
    },
    /// Two different list roots were declared for one folder.
    #[error("list conflict in {dir}: {a} vs {b}")]
    ListConflict {
        /// The folder with conflicting roots.
        dir: String,
        /// First declared root.
        a: String,
        /// Second declared root.
        b: String,
    },
    /// A CalDAV operation failed.
    #[error("caldav {kind:?} (status {status:?}): {detail}")]
    Caldav {
        /// Failure classification.
        kind: CaldavErrorKind,
        /// HTTP status, when the server responded.
        status: Option<u16>,
        /// Human-readable detail.
        detail: String,
    },
    /// Filesystem or other I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A value failed validation (e.g. an empty list slug).
    #[error("validation: `{field}`: {reason}")]
    Validation {
        /// Name of the rejected field.
        field: &'static str,
        /// Why it was rejected.
        reason: String,
    },
}

/// Classification of CalDAV failures (§12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaldavErrorKind {
    /// Authentication failed (401/403).
    Auth,
    /// The server could not be reached.
    Network,
    /// The server answered but the response violated the protocol.
    Protocol,
    /// Precondition failed (e.g. etag mismatch).
    Conflict,
    /// TLS handshake failure.
    Tls,
}
