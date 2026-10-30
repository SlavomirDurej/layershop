//! Generative image back ends for PhotoCraft.
//!
//! A [`Model`] describes one image generator the user has set up: a **local** model run by a
//! ComfyUI server (the workflow is built here from the model's family and files, see [`comfy`]),
//! or a **cloud** model behind an API key (OpenAI and Gemini image APIs, see [`cloud`]).
//! [`generate`] turns a prompt and a size into encoded image bytes (PNG, JPEG or WebP).
//!
//! Everything that builds requests and reads responses is pure and tested without a network.
//! The network itself is a [`Transport`], which the app supplies ([`UreqTransport`] on desktop;
//! none in the browser, where generation reports that it is unavailable).
//!
//! Nothing here panics on a hostile response: every failure is an [`Error`] with a message the
//! user can act on.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod activity;
pub mod cloud;
pub mod comfy;
pub mod detect;
mod http;
pub mod launch;
pub mod layout;
mod live;
pub mod model;
pub mod pricing;
mod run;
pub mod size;
mod ws;

#[cfg(not(target_arch = "wasm32"))]
pub use http::UreqTransport;
pub use http::{MAX_RESPONSE_BYTES, Method, Part, Request, Response, Transport};
#[cfg(not(target_arch = "wasm32"))]
pub use launch::ComfyProcess;
pub use launch::{Launch, LocalServer, ServerState};
pub use layout::{LayoutFormat, Region, RegionKind, layout_prompt};
pub use model::{Family, MAX_REFERENCES, Model, Provider, Settings, preferred_edit};
pub use run::{Control, Generated, Job, Source, comfy_models, generate, random_seed, source_size};

/// Why a generation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The model's settings can't produce a request (missing file name, bad workflow JSON, …).
    #[error("{0}")]
    Config(String),
    /// The server could not be reached, or the connection failed.
    #[error("{0}")]
    Network(String),
    /// The server answered with an error.
    #[error("{0}")]
    Server(String),
    /// The answer was not what the API documents.
    #[error("{0}")]
    BadResponse(String),
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
