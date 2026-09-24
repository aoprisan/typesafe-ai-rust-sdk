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
//!
//! # Example
//!
//! With the `derive` feature, a struct is the rubric: each field is a question, and the answers
//! come back into it, typed.
//!
//! ```no_run
//! # #[cfg(feature = "derive")] {
//! use typesafe::{ChoiceOf, Client, NoulAnswer, Rubric, RubricChoice, ScoreAnswer};
//!
//! #[derive(Rubric)]
//! struct Triage {
//!     #[noul("The message conveys urgency")]
//!     is_urgent: NoulAnswer,
//!     #[choice("Which team should handle this")]
//!     department: ChoiceOf<Department>,
//!     #[score("How frustrated", levels = ["Calm", "Frustrated but civil", "Very angry"])]
//!     frustration: ScoreAnswer,
//! }
//!
//! #[derive(Debug, RubricChoice)]
//! enum Department {
//!     /// Payment or subscription issues
//!     Billing,
//!     /// Bugs or integration problems
//!     Technical,
//! }
//!
//! # async fn run() -> typesafe::Result<()> {
//! let client = Client::from_env()?; // TYPESAFE_API_KEY
//! let triage = client
//!     .ask::<Triage>("I've been trying to connect Stripe for 3 days. Please help ASAP.")
//!     .await?;
//! println!("route to {:?}", *triage.department);
//! println!("urgent: {}", triage.is_urgent.is_yes(0.8));
//! println!("frustration: {:.2}", triage.frustration.score);
//! # Ok(()) }
//! # }
//! ```
//!
//! Without the derive, [`Client::system_one`] takes a [`Questions`] map built at runtime and
//! returns a [`SystemOneResponse`] to look answers up in by name.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

pub mod cassette;
mod client;
pub mod constants;
pub mod error;
pub mod question;
pub mod response;
pub mod retry;
pub mod rubric;

#[cfg(feature = "blocking")]
#[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
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
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use typesafe_derive::{Rubric, RubricChoice};

/// Re-exported so callers can build headers without adding a dependency.
pub use http;
/// Re-exported so callers can build structured instructions/state without adding a dependency.
pub use serde_json::{self, json};

/// Compiles the README's code samples as doctests (they are not part of the rendered docs). The
/// quick start uses `#[derive(Rubric)]`, so they are checked with the `derive` feature on.
#[cfg(all(doctest, feature = "derive"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
