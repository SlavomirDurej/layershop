//! Local AI setup: a private ComfyUI and the recommended model files, installed for the user.
//!
//! Someone who has never heard of ComfyUI gets from "no Python on this PC" to a first generated
//! image without a terminal. The pieces:
//!
//! - [`manifest`]: what can be installed. ComfyUI's portable build, pinned by version and
//!   sha256, and the model files (each listed once, shared between models) with their size,
//!   checksum and licence.
//! - [`probe`]: what the machine has (system, graphics card, free space, an existing ComfyUI).
//! - [`plan`]: a pure function from the manifest, the user's choices and what is on disk to the
//!   ordered list of steps. Every step can tell whether it is already done, so resuming, adding
//!   a model later and repairing are all "plan again and run what is left".
//! - [`fetch`] and [`download`]: a streaming network seam and the downloader on top of it
//!   (resume, checksum, retries); [`unpack`] reads the 7z archive with its path and size guards;
//!   [`runtime`] knows where things live and how the installed engine is started.
//! - [`configure`]: the settings an install leaves behind (address, launch, models).
//! - [`install`]: runs a plan on a worker thread and reports progress; [`state`] is the record
//!   that lets an interrupted setup be continued after a restart.
//! - [`hf`]: Hugging Face, for the gated models: Sign in with Hugging Face (OAuth with PKCE), an
//!   access token as the fallback, and whether the user may download each gated model yet.
//!
//! Everything that touches the network, the file system or another process is native only. The
//! web build keeps the manifest and the plan; its setup commands say they need the desktop app.
//!
//! Nothing here panics on a hostile manifest, header, archive or path: every failure is an
//! [`Error`] with a message the user can act on.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod configure;
#[cfg(not(target_arch = "wasm32"))]
pub mod download;
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod hf;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
pub mod manifest;
pub mod plan;
pub mod probe;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;
pub mod state;
#[cfg(not(target_arch = "wasm32"))]
pub mod unpack;

pub use manifest::Manifest;
pub use plan::{Choices, Plan, Route, Step};
pub use probe::Probe;
pub use state::SetupState;

/// Why a setup step failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The manifest or the choices can't be used (unknown model, bad path, …).
    #[error("{0}")]
    Config(String),
    /// The connection failed or stalled: worth trying again.
    #[error("{0}")]
    Network(String),
    /// The server refused the file (a gated model without a sign-in): trying again won't help.
    #[error("{0}")]
    Access(String),
    /// The server answered with an error, or sent something other than the file.
    #[error("{0}")]
    Server(String),
    /// The disk: full, not writable, a file that vanished.
    #[error("{0}")]
    Disk(String),
    /// What arrived is not what the manifest pins (size or sha256).
    #[error("{0}")]
    Integrity(String),
    /// The archive is damaged, or is not one this app will unpack.
    #[error("{0}")]
    Archive(String),
    /// The installed engine did not start or did not answer.
    #[error("{0}")]
    Engine(String),
    /// The user cancelled or paused.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

/// `bytes` for people: `412 GB`, `18.5 GB`, `675 MB` (decimal units, as download pages and
/// Windows' own "free space" readouts use for sizes this large).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, f64); 4] = [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)];
    let b = bytes as f64;
    for (unit, size) in UNITS {
        if b >= size {
            let v = b / size;
            // Three significant figures at most: 1.62 GB, 18.5 GB, 412 GB.
            return if v >= 100.0 {
                format!("{v:.0} {unit}")
            } else if v >= 10.0 {
                format!("{v:.1} {unit}")
            } else {
                format!("{v:.2} {unit}")
            };
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_a_download_page() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(675_509_688), "676 MB");
        assert_eq!(human_bytes(2_003_629_308), "2.00 GB");
        assert_eq!(human_bytes(18_520_930_264), "18.5 GB");
        assert_eq!(human_bytes(412_000_000_000), "412 GB");
        assert_eq!(human_bytes(u64::MAX), "18446744 TB");
    }
}
