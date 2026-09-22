//! Rust client for the [TypeSafe AI](https://typesafe.ai) System One API.
//!
//! Send a `state` and a map of typed questions — [`Noul`] (yes/no probability), [`Choice`]
//! (one of N labels) and [`Score`] (ordered levels) — and get typed answers back.
//!
//! Behaviour follows the official Python SDK (`typesafe-sdk`): the same environment variables,
//! defaults, retry semantics, error classification and forward-compatible decoding.
//!
//! Enable the `blocking` feature for a synchronous client in [`blocking`], `reqwest-client`
//! to supply your own `reqwest::Client`, and `derive` for `#[derive(Rubric)]`: a struct that
//! describes the questions and receives the answers (see [`rubric`]).
#![warn(missing_docs)]

mod client;
pub mod constants;
pub mod error;
pub mod question;
pub mod response;
pub mod retry;
pub mod rubric;

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
pub use rubric::{AskRequest, ChoiceOf, Rubric, RubricChoice};
/// `#[derive(Rubric)]` and `#[derive(RubricChoice)]` (feature `derive`); see [`rubric`].
#[cfg(feature = "derive")]
pub use typesafe_derive::{Rubric, RubricChoice};

/// Re-exported so callers can build headers without adding a dependency.
pub use http;
/// Re-exported so callers can build structured instructions/state without adding a dependency.
pub use serde_json::{self, json};

/// Compiles the README's code samples as doctests (they are not part of the rendered docs).
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
