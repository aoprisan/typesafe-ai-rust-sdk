//! Rust client for the [TypeSafe AI](https://typesafe.ai) System One API.
//!
//! Send a `state` and a map of typed questions — [`Noul`] (yes/no probability), [`Choice`]
//! (one of N labels) and [`Score`] (ordered levels) — and get typed answers back.
//!
//! Behaviour follows the official Python SDK (`typesafe-sdk`): the same environment variables,
//! defaults, retry semantics, error classification and forward-compatible decoding.
//!
//! Enable the `blocking` feature for a synchronous client in [`blocking`].
#![warn(missing_docs)]

mod client;
pub mod constants;
pub mod error;
pub mod question;
pub mod response;
pub mod retry;

#[cfg(feature = "blocking")]
pub mod blocking;

pub use client::{Client, ClientBuilder, ListModelsRequest, Models, SystemOneRequest};
pub use error::{ApiError, ApiErrorKind, Error, ResponseValidationError, Result};
pub use question::{Choice, Noul, NoulCriteria, Question, Questions, Score};
pub use response::{
    Answer, ChoiceAnswer, ListModelsResponse, ModelMetadata, NoulAnswer, ResponseMeta, ScoreAnswer,
    SystemOneResponse, Usage,
};
pub use retry::RetryPolicy;

/// Re-exported so callers can pass a custom HTTP client or headers.
pub use reqwest;
/// Re-exported so callers can build structured instructions/state without adding a dependency.
pub use serde_json::{self, json};
