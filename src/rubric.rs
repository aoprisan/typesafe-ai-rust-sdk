//! Rubrics as types: a struct describes the questions, and the answers come back into it.
//!
//! [`Rubric`] is implemented by `#[derive(Rubric)]` (feature `derive`) on a struct with one field
//! per question. The field name is the question name, the attribute is its type, and the field
//! type is what the answer decodes into, so a misspelled name or a wrong answer type is a compile
//! error instead of a `None` at runtime:
//!
//! ```
//! # #[cfg(feature = "derive")] {
//! use typesafe::{ChoiceOf, NoulAnswer, Rubric, RubricChoice, ScoreAnswer};
//!
//! #[derive(Rubric)]
//! struct Triage {
//!     #[noul("The message conveys urgency", yes = "A deadline or ASAP", no = "Routine")]
//!     is_urgent: NoulAnswer,
//!     #[choice("Which team should handle this")]
//!     department: ChoiceOf<Department>,
//!     #[score("How frustrated", levels = ["Calm", "Frustrated but civil", "Very angry"])]
//!     frustration: ScoreAnswer,
//! }
//!
//! #[derive(Debug, PartialEq, RubricChoice)]
//! enum Department {
//!     #[option("Payment or subscription issues")]
//!     Billing,
//!     /// Bugs or integration problems
//!     Technical,
//! }
//!
//! let questions = Triage::questions(); // what `Client::ask::<Triage>` sends
//! assert_eq!(questions.len(), 3);
//! # }
//! ```
//!
//! `client.ask::<Triage>(state).await?` sends the questions and returns a `Triage`; see
//! [`Client::ask`]. Without the derive, the same traits can be implemented by hand, and
//! `Rubric::from_response` decodes any [`SystemOneResponse`] you already have.
//!
//! # Attributes
//!
//! On the struct's fields — exactly one of the first three per field:
//!
//! | Attribute | Field type | Asks |
//! | --- | --- | --- |
//! | `#[noul("…", yes = "…", no = "…")]` | [`NoulAnswer`], or `f64` for the probability | a [`Noul`]; `yes`/`no` are optional |
//! | `#[choice("…")]` | a `RubricChoice` enum, [`ChoiceOf<E>`], [`ChoiceAnswer`] or `String` | a [`Choice`] |
//! | `#[score("…", levels = ["…", …])]` | [`ScoreAnswer`], or `f64` for the score | a [`Score`], levels lowest first |
//! | `#[rubric(rename = "…")]` | | a question name other than the field's |
//!
//! The enum types bring their options with them. [`ChoiceAnswer`] and `String` do not, so they
//! take theirs as `#[choice("…", labels = ["a", "b"])]`. Instructions left out of an attribute are
//! read from the field's doc comment.
//!
//! On an enum's variants, for `#[derive(RubricChoice)]`: the label is the variant name in
//! snake_case (`NeedsHuman` → `needs_human`) unless `#[rubric(rename = "…")]` says otherwise, and
//! the description is `#[option("…")]` or else the doc comment; a variant with neither is an
//! undescribed option. The derive also implements `FromStr`, so
//! `res.choice("department").unwrap().parse::<Department>()` keeps working.
//!
//! # Errors
//!
//! A response that does not fit the struct — an answer missing, of another type, or a label the
//! enum does not have — is an [`Error::ResponseValidation`] whose `field_path` names it
//! (`answers.department` or `answers.department.choice`).

use std::fmt;
use std::future::IntoFuture;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Duration;

use http::header::{HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::Value;

use crate::client::{BoxFuture, Client, SystemOneRequest};
use crate::error::{Error, ResponseValidationError, Result};
use crate::question::{Choice, Questions};
use crate::response::{ChoiceAnswer, NoulAnswer, ScoreAnswer, SystemOneResponse};
use crate::retry::RetryPolicy;

#[cfg(doc)]
use crate::question::{Noul, Score};

/// A struct whose fields are questions and whose values are their answers. Derive it with
/// `#[derive(Rubric)]` (feature `derive`); see the [module docs](self).
pub trait Rubric: Sized {
    /// The questions to send, one per field, in field order.
    fn questions() -> Questions;

    /// Read the answers back out of a response to [`Rubric::questions`].
    ///
    /// # Errors
    ///
    /// [`Error::ResponseValidation`], with `field_path` naming the question, if an answer is
    /// missing, of another type, or a label the field's type does not have.
    fn from_response(response: &SystemOneResponse) -> Result<Self>;
}

/// An enum whose variants are the options of a [`Choice`]. Derive it with
/// `#[derive(RubricChoice)]` (feature `derive`), which also implements [`ChoiceField`] and
/// `FromStr` for the enum.
pub trait RubricChoice: Sized {
    /// Every option as `(label, description)`, in the order they are offered.
    const OPTIONS: &'static [(&'static str, Option<&'static str>)];

    /// The variant for a label, if there is one.
    fn from_label(label: &str) -> Option<Self>;

    /// The label this variant is sent and answered as.
    fn label(&self) -> &'static str;

    /// A [`Choice`] offering every option.
    fn choice(instructions: impl Into<Value>) -> Choice {
        Self::OPTIONS
            .iter()
            .fold(
                Choice::new(instructions),
                |c, (label, description)| match description {
                    Some(d) => c.option(*label, *d),
                    None => c.label(*label),
                },
            )
    }

    /// [`RubricChoice::from_label`], with an error that lists the labels there are.
    ///
    /// # Errors
    ///
    /// [`UnknownLabel`] if no option has this label.
    fn parse_label(label: &str) -> std::result::Result<Self, UnknownLabel> {
        Self::from_label(label).ok_or_else(|| UnknownLabel {
            label: label.to_owned(),
            expected: Self::OPTIONS.iter().map(|(l, _)| *l).collect(),
        })
    }
}

/// A label the options do not include.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnknownLabel {
    /// The label that came back.
    pub label: String,
    /// The labels that were offered.
    pub expected: Vec<&'static str>,
}

impl fmt::Display for UnknownLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown label {:?}; expected one of ", self.label)?;
        for (i, l) in self.expected.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{l:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnknownLabel {}

/// A choice decoded into your enum, with the distribution it was picked from.
///
/// Derefs to the enum, so `match *answer { Department::Billing => … }` works.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ChoiceOf<T> {
    /// The selected option.
    pub value: T,
    /// The answer it came from: per-label probabilities and confidence.
    pub answer: ChoiceAnswer,
}

impl<T> ChoiceOf<T> {
    /// Certainty derived from the distribution, 0 to 1.
    pub fn confidence(&self) -> f64 {
        self.answer.confidence
    }

    /// The selected option.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: RubricChoice> ChoiceOf<T> {
    /// The probability the model gave an option.
    pub fn probability(&self, option: &T) -> Option<f64> {
        self.answer.probability(option.label())
    }
}

impl<T> Deref for ChoiceOf<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

/// A field type a `#[noul]` answer decodes into.
pub trait NoulField {
    /// Convert the answer.
    fn from_noul(answer: &NoulAnswer) -> Self;
}

impl NoulField for NoulAnswer {
    fn from_noul(answer: &NoulAnswer) -> Self {
        answer.clone()
    }
}

/// The probability of "yes".
impl NoulField for f64 {
    fn from_noul(answer: &NoulAnswer) -> Self {
        answer.noul
    }
}

/// A field type a `#[score]` answer decodes into.
pub trait ScoreField {
    /// Convert the answer.
    fn from_score(answer: &ScoreAnswer) -> Self;
}

impl ScoreField for ScoreAnswer {
    fn from_score(answer: &ScoreAnswer) -> Self {
        answer.clone()
    }
}

/// The probability-weighted level.
impl ScoreField for f64 {
    fn from_score(answer: &ScoreAnswer) -> Self {
        answer.score
    }
}

/// A field type a `#[choice]` asks with and decodes into. `#[derive(RubricChoice)]` implements
/// it for the enum; it is implemented here for [`ChoiceOf`], [`ChoiceAnswer`] and `String`.
pub trait ChoiceField: Sized {
    /// The question, with whatever options the type knows (none, for a plain label).
    fn question(instructions: Value) -> Choice;

    /// Convert the answer.
    ///
    /// # Errors
    ///
    /// [`UnknownLabel`] if the chosen label is not one the type has.
    fn from_choice(answer: &ChoiceAnswer) -> std::result::Result<Self, UnknownLabel>;
}

impl<T: RubricChoice> ChoiceField for ChoiceOf<T> {
    fn question(instructions: Value) -> Choice {
        T::choice(instructions)
    }

    fn from_choice(answer: &ChoiceAnswer) -> std::result::Result<Self, UnknownLabel> {
        Ok(ChoiceOf {
            value: T::parse_label(&answer.choice)?,
            answer: answer.clone(),
        })
    }
}

impl ChoiceField for ChoiceAnswer {
    fn question(instructions: Value) -> Choice {
        Choice::new(instructions)
    }

    fn from_choice(answer: &ChoiceAnswer) -> std::result::Result<Self, UnknownLabel> {
        Ok(answer.clone())
    }
}

/// The selected label.
impl ChoiceField for String {
    fn question(instructions: Value) -> Choice {
        Choice::new(instructions)
    }

    fn from_choice(answer: &ChoiceAnswer) -> std::result::Result<Self, UnknownLabel> {
        Ok(answer.choice.clone())
    }
}

/// What the derived code calls. Not part of the API.
#[doc(hidden)]
pub mod __private {
    use super::*;
    use crate::response::{Answer, AnswerKind};

    fn mismatch(response: &SystemOneResponse, field_path: String, detail: String) -> Error {
        Error::ResponseValidation(Box::new(ResponseValidationError {
            status: response.meta.status,
            field_path,
            detail,
            body: Some(response.raw.clone()),
            headers: response.meta.headers.clone(),
            endpoint: None,
        }))
    }

    fn answer<'r>(
        response: &'r SystemOneResponse,
        name: &str,
        kind: AnswerKind,
    ) -> Result<&'r Answer> {
        let answer = response.answers.get(name).ok_or_else(|| {
            let detail = if response.raw["answers"].get(name).is_some() {
                format!("the answer is of a type this SDK does not know; expected a {kind}")
            } else {
                format!("no answer; expected a {kind}")
            };
            mismatch(response, format!("answers.{name}"), detail)
        })?;
        if answer.kind() != kind {
            return Err(mismatch(
                response,
                format!("answers.{name}"),
                format!("expected a {kind} answer, got a {}", answer.kind()),
            ));
        }
        Ok(answer)
    }

    pub fn noul<'r>(response: &'r SystemOneResponse, name: &str) -> Result<&'r NoulAnswer> {
        match answer(response, name, AnswerKind::Noul)? {
            Answer::Noul(a) => Ok(a),
            _ => unreachable!("kind checked"),
        }
    }

    pub fn score<'r>(response: &'r SystemOneResponse, name: &str) -> Result<&'r ScoreAnswer> {
        match answer(response, name, AnswerKind::Score)? {
            Answer::Score(a) => Ok(a),
            _ => unreachable!("kind checked"),
        }
    }

    pub fn choice<T: ChoiceField>(response: &SystemOneResponse, name: &str) -> Result<T> {
        let Answer::Choice(a) = answer(response, name, AnswerKind::Choice)? else {
            unreachable!("kind checked")
        };
        T::from_choice(a)
            .map_err(|e| mismatch(response, format!("answers.{name}.choice"), e.to_string()))
    }
}

impl Client {
    /// Ask the questions of a [`Rubric`] about `state` and decode the answers into it.
    ///
    /// ```no_run
    /// # #[cfg(feature = "derive")] {
    /// use typesafe::{Client, NoulAnswer, Rubric};
    ///
    /// #[derive(Rubric)]
    /// struct Urgency {
    ///     #[noul("The message conveys urgency")]
    ///     is_urgent: NoulAnswer,
    /// }
    ///
    /// # async fn run() -> typesafe::Result<()> {
    /// let client = Client::from_env()?;
    /// let answer: Urgency = client.ask("The payout failed again.").await?;
    /// println!("{}", answer.is_urgent.is_yes(0.8));
    /// # Ok(()) }
    /// # }
    /// ```
    pub fn ask<R: Rubric>(&self, state: impl Serialize) -> AskRequest<R> {
        AskRequest {
            req: self.system_one(state, R::questions()),
            rubric: PhantomData,
        }
    }
}

/// A pending [`Client::ask`]: the same per-call options as a [`SystemOneRequest`], and `.await`
/// gives the rubric instead of the response.
#[must_use = "requests do nothing until awaited"]
#[derive(Debug)]
pub struct AskRequest<R> {
    req: SystemOneRequest,
    rubric: PhantomData<fn() -> R>,
}

impl<R: Rubric> AskRequest<R> {
    /// Override the retry policy for this call.
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.req = self.req.retry(policy);
        self
    }

    /// Override the per-attempt timeout for this call.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.req = self.req.timeout(timeout);
        self
    }

    /// Add a header for this call (protected headers still win).
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.req = self.req.header(name, value);
        self
    }

    /// Override the model for this call.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.req = self.req.model(model);
        self
    }

    /// Add a top-level body field; see [`SystemOneRequest::extra_body`].
    pub fn extra_body(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.req = self.req.extra_body(key, value);
        self
    }

    /// Send the request and decode the answers.
    ///
    /// # Errors
    ///
    /// Those of [`SystemOneRequest::send`], plus those of [`Rubric::from_response`].
    pub async fn send(self) -> Result<R> {
        R::from_response(&self.req.send().await?)
    }
}

impl<R: Rubric + 'static> IntoFuture for AskRequest<R> {
    type Output = Result<R>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

/// Mistakes the derive refuses to compile, each checked as a `compile_fail` doctest.
///
/// An answer read as the wrong type:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[noul("Urgent?")]
///     is_urgent: typesafe::ScoreAnswer,
/// }
/// ```
/// A choice whose type has no options to offer:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[choice("Which team")]
///     team: u32,
/// }
/// ```
/// A score without levels:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[score("How angry")]
///     anger: typesafe::ScoreAnswer,
/// }
/// ```
/// A score with one level, or with more than the API's ten:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[score("How angry", levels = ["Furious"])]
///     anger: typesafe::ScoreAnswer,
/// }
/// ```
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[score("How angry", levels = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"])]
///     anger: typesafe::ScoreAnswer,
/// }
/// ```
/// A field that is not a question:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[noul("Urgent?")]
///     is_urgent: typesafe::NoulAnswer,
///     note: String,
/// }
/// ```
/// Two fields asking under one name:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[noul("Urgent?")]
///     is_urgent: typesafe::NoulAnswer,
///     #[noul("Really urgent?")]
///     #[rubric(rename = "is_urgent")]
///     very: typesafe::NoulAnswer,
/// }
/// ```
/// A key the attribute does not take:
/// ```compile_fail
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[noul("Urgent?", levels = ["a", "b"])]
///     is_urgent: typesafe::NoulAnswer,
/// }
/// ```
/// An option that carries data:
/// ```compile_fail
/// #[derive(typesafe::RubricChoice)]
/// enum Team {
///     Billing,
///     Other(String),
/// }
/// ```
/// And the same shapes, spelled correctly, compile:
/// ```
/// #[derive(typesafe::Rubric)]
/// struct R {
///     #[noul("Urgent?", yes = "A deadline")]
///     is_urgent: typesafe::NoulAnswer,
///     #[choice("Which team")]
///     team: Team,
///     #[score("How angry", levels = ["Calm", "Angry"])]
///     anger: typesafe::ScoreAnswer,
///     #[score("How bad", levels = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"])]
///     severity: typesafe::ScoreAnswer,
/// }
/// #[derive(typesafe::RubricChoice)]
/// enum Team {
///     Billing,
///     Other,
/// }
/// ```
#[cfg(all(doctest, feature = "derive"))]
pub struct DeriveCompileFail;
