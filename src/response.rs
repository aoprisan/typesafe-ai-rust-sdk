//! Answers and response metadata.

use std::collections::BTreeMap;
use std::str::FromStr;

use http::header::HeaderMap;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::constants::REQUEST_ID_HEADER;

/// A yes/no answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct NoulAnswer {
    /// Probability of "yes", from 0 to 1.
    pub noul: f64,
}

impl NoulAnswer {
    /// `noul >= threshold`.
    pub fn is_yes(&self, threshold: f64) -> bool {
        self.noul >= threshold
    }
}

/// A selected option with its distribution.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ChoiceAnswer {
    /// The highest-probability option.
    pub choice: String,
    /// Every option mapped to its probability, in server order.
    pub probabilities: IndexMap<String, f64>,
    /// Certainty derived from the distribution, 0 to 1.
    pub confidence: f64,
}

impl ChoiceAnswer {
    /// Parse the selected label into your own type (e.g. an enum implementing `FromStr`).
    pub fn parse<T: FromStr>(&self) -> Result<T, T::Err> {
        self.choice.parse()
    }

    /// Probability of a given label.
    pub fn probability(&self, label: &str) -> Option<f64> {
        self.probabilities.get(label).copied()
    }

    /// Labels ordered by descending probability.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut v: Vec<_> = self
            .probabilities
            .iter()
            .map(|(k, p)| (k.as_str(), *p))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }
}

/// An expected score with its rubric and distribution.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScoreAnswer {
    /// Probability-weighted level; may fall between levels.
    pub score: f64,
    /// Certainty derived from the distribution, 0 to 1.
    pub confidence: f64,
    /// Level index → the description you supplied.
    pub legend: BTreeMap<u32, Value>,
    /// Level index → probability.
    pub probabilities: BTreeMap<u32, f64>,
}

impl ScoreAnswer {
    /// The single most likely level (argmax of `probabilities`), as opposed to the weighted `score`.
    pub fn most_likely_level(&self) -> Option<u32> {
        self.probabilities
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(level, _)| *level)
    }

    /// `score` rounded to the nearest level.
    pub fn rounded_level(&self) -> u32 {
        self.score.round().max(0.0) as u32
    }
}

/// An answer to one question.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    /// See [`NoulAnswer`].
    Noul(NoulAnswer),
    /// See [`ChoiceAnswer`].
    Choice(ChoiceAnswer),
    /// See [`ScoreAnswer`].
    Score(ScoreAnswer),
}

impl Answer {
    /// The wire `type` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            Answer::Noul(_) => "noul",
            Answer::Choice(_) => "choice",
            Answer::Score(_) => "score",
        }
    }

    /// Confidence, for answer types that report it.
    pub fn confidence(&self) -> Option<f64> {
        match self {
            Answer::Noul(_) => None,
            Answer::Choice(a) => Some(a.confidence),
            Answer::Score(a) => Some(a.confidence),
        }
    }
}

/// Token usage. The API reports these when available; absent counts are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Usage {
    /// Input tokens, when reported.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output tokens, when reported.
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

/// Metadata of the HTTP exchange that produced a response.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResponseMeta {
    /// HTTP status.
    pub status: u16,
    /// Response headers.
    pub headers: HeaderMap,
    /// Number of attempts made, including the successful one.
    pub attempts: u32,
}

impl ResponseMeta {
    /// The `x-typesafe-request-id` header.
    pub fn request_id(&self) -> Option<&str> {
        self.headers
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
    }
}

/// The result of a System One call.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SystemOneResponse {
    /// The model that answered.
    pub model: String,
    /// Token usage.
    pub usage: Usage,
    /// Answers keyed by question name, in server order. Answer types unknown to this SDK version
    /// are skipped (with a `tracing` warning) and remain visible in [`SystemOneResponse::raw`].
    pub answers: IndexMap<String, Answer>,
    /// The full decoded body. Object keys follow `serde_json::Map` ordering (sorted unless your
    /// build enables serde_json's `preserve_order`); the typed fields above keep server order.
    pub raw: Value,
    /// HTTP metadata.
    pub meta: ResponseMeta,
}

impl SystemOneResponse {
    /// The `x-typesafe-request-id` header.
    pub fn request_id(&self) -> Option<&str> {
        self.meta.request_id()
    }

    /// The answer to `name`, if it is a Noul.
    pub fn noul(&self, name: &str) -> Option<&NoulAnswer> {
        match self.answers.get(name)? {
            Answer::Noul(a) => Some(a),
            _ => None,
        }
    }

    /// The answer to `name`, if it is a Choice.
    pub fn choice(&self, name: &str) -> Option<&ChoiceAnswer> {
        match self.answers.get(name)? {
            Answer::Choice(a) => Some(a),
            _ => None,
        }
    }

    /// The answer to `name`, if it is a Score.
    pub fn score(&self, name: &str) -> Option<&ScoreAnswer> {
        match self.answers.get(name)? {
            Answer::Score(a) => Some(a),
            _ => None,
        }
    }

    /// All Noul answers.
    pub fn nouls(&self) -> impl Iterator<Item = (&str, &NoulAnswer)> {
        self.answers.iter().filter_map(|(k, a)| match a {
            Answer::Noul(n) => Some((k.as_str(), n)),
            _ => None,
        })
    }

    /// All Choice answers.
    pub fn choices(&self) -> impl Iterator<Item = (&str, &ChoiceAnswer)> {
        self.answers.iter().filter_map(|(k, a)| match a {
            Answer::Choice(c) => Some((k.as_str(), c)),
            _ => None,
        })
    }

    /// All Score answers.
    pub fn scores(&self) -> impl Iterator<Item = (&str, &ScoreAnswer)> {
        self.answers.iter().filter_map(|(k, a)| match a {
            Answer::Score(s) => Some((k.as_str(), s)),
            _ => None,
        })
    }
}

/// One available model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ModelMetadata {
    /// Model name, e.g. `jev-latest`.
    pub name: String,
    /// Description.
    pub description: String,
    /// Release date as reported by the API.
    pub release_date: String,
}

/// The models available to the account.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ListModelsResponse {
    /// Available models.
    pub models: Vec<ModelMetadata>,
    /// The full decoded body.
    pub raw: Value,
    /// HTTP metadata.
    pub meta: ResponseMeta,
}

// ---- decoding ------------------------------------------------------------------------------

/// A decode failure with a dotted path to the offending field.
pub(crate) struct DecodeFailure {
    pub path: String,
    pub detail: String,
}

/// Deserialize `T` from JSON text, reporting the dotted path of the failing field under `prefix`.
///
/// Decoding from text (not from a [`Value`]) keeps object key order for `IndexMap` fields
/// regardless of serde_json's `preserve_order` feature.
fn typed<T: DeserializeOwned>(prefix: &str, json: &[u8]) -> Result<T, DecodeFailure> {
    let mut de = serde_json::Deserializer::from_slice(json);
    let value = serde_path_to_error::deserialize(&mut de).map_err(|e| {
        let inner = e.path().to_string();
        let path = match (prefix.is_empty(), inner.as_str()) {
            (true, ".") => String::new(),
            (true, _) => inner.clone(),
            (false, ".") => prefix.to_owned(),
            (false, _) => format!("{prefix}.{inner}"),
        };
        // serde_json reports a missing field on the parent path; append it for precision.
        let msg = e.inner().to_string();
        let path = match msg
            .strip_prefix("missing field `")
            .and_then(|r| r.split('`').next())
        {
            Some(field) if path.is_empty() => field.to_owned(),
            Some(field) => format!("{path}.{field}"),
            None => path,
        };
        DecodeFailure { path, detail: msg }
    })?;
    de.end().map_err(|e| DecodeFailure {
        path: prefix.to_owned(),
        detail: e.to_string(),
    })?;
    Ok(value)
}

#[derive(Deserialize)]
struct Envelope {
    model: String,
    #[serde(default)]
    usage: Usage,
    answers: IndexMap<String, Box<RawValue>>,
}

/// A successfully decoded System One body.
pub(crate) struct DecodedSystemOne {
    pub model: String,
    pub usage: Usage,
    pub answers: IndexMap<String, Answer>,
    pub raw: Value,
}

pub(crate) fn decode_system_one(body: &[u8]) -> Result<DecodedSystemOne, DecodeFailure> {
    let env: Envelope = typed("", body)?;
    let mut answers = IndexMap::with_capacity(env.answers.len());
    for (name, value) in env.answers {
        let prefix = format!("answers.{name}");
        let json = value.get();
        let tag = serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned));
        let Some(tag) = tag else {
            return Err(DecodeFailure {
                path: format!("{prefix}.type"),
                detail: "missing or non-string answer type".into(),
            });
        };
        let answer = match tag.as_str() {
            "noul" => Answer::Noul(typed(&prefix, json.as_bytes())?),
            "choice" => Answer::Choice(typed(&prefix, json.as_bytes())?),
            "score" => Answer::Score(typed(&prefix, json.as_bytes())?),
            other => {
                tracing::warn!(answer = %name, r#type = %other, "ignoring answer with unrecognized type");
                continue;
            }
        };
        answers.insert(name, answer);
    }
    // The envelope parsed, so the body is valid JSON.
    let raw = serde_json::from_slice(body).unwrap_or(Value::Null);
    Ok(DecodedSystemOne {
        model: env.model,
        usage: env.usage,
        answers,
        raw,
    })
}

#[derive(Deserialize)]
struct ModelList {
    models: Vec<ModelMetadata>,
}

pub(crate) fn decode_models(body: &[u8]) -> Result<(Vec<ModelMetadata>, Value), DecodeFailure> {
    let list: ModelList = typed("", body)?;
    let raw = serde_json::from_slice(body).unwrap_or(Value::Null);
    Ok((list.models, raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "model": "jev-latest",
            "answers": {
                "department": {"type": "choice", "choice": "technical",
                    "probabilities": {"billing": 0.159, "technical": 0.84, "sales": 0.001}, "confidence": 0.596},
                "frustration": {"type": "score", "score": 1.6,
                    "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                    "probabilities": {"0": 0.05, "1": 0.3, "2": 0.65}, "confidence": 0.78},
                "is_urgent": {"type": "noul", "noul": 0.999},
                "future": {"type": "span", "start": 3}
            },
            "usage": {"input_tokens": 312, "output_tokens": 48, "extra": true}
        })
    }

    fn decode(v: Value) -> Result<DecodedSystemOne, DecodeFailure> {
        decode_system_one(&serde_json::to_vec(&v).unwrap())
    }

    #[test]
    fn decodes_all_types_and_skips_unknown() {
        let DecodedSystemOne {
            model,
            usage,
            answers,
            raw,
        } = decode(sample()).ok().unwrap();
        assert_eq!(model, "jev-latest");
        assert_eq!(usage.input_tokens, Some(312));
        assert_eq!(answers.len(), 3);
        assert_eq!(raw["answers"]["future"]["start"], json!(3));
        let Answer::Score(s) = &answers["frustration"] else {
            panic!()
        };
        assert_eq!(s.legend[&2], json!("Very angry"));
        assert_eq!(s.most_likely_level(), Some(2));
        assert_eq!(s.rounded_level(), 2);
        let Answer::Choice(c) = &answers["department"] else {
            panic!()
        };
        assert_eq!(c.ranked()[0], ("technical", 0.84));
    }

    #[test]
    fn preserves_server_order_of_probabilities() {
        let body = br#"{"model":"m","usage":{},"answers":{"c":{"type":"choice","choice":"z",
            "probabilities":{"z":0.5,"a":0.3,"m":0.2},"confidence":0.1}}}"#;
        let d = decode_system_one(body).ok().unwrap();
        let Answer::Choice(c) = &d.answers["c"] else {
            panic!()
        };
        let keys: Vec<_> = c.probabilities.keys().map(String::as_str).collect();
        assert_eq!(keys, ["z", "a", "m"]);
    }

    #[test]
    fn usage_may_be_empty_or_absent() {
        let d = decode(json!({"model": "m", "answers": {}, "usage": {}}))
            .ok()
            .unwrap();
        assert_eq!(d.usage, Usage::default());
        let d = decode(json!({"model": "m", "answers": {}})).ok().unwrap();
        assert_eq!(d.usage, Usage::default());
    }

    #[test]
    fn rejects_non_json_and_trailing_content() {
        assert_eq!(decode_system_one(b"<html>").err().unwrap().path, "");
        let mut body = serde_json::to_vec(&sample()).unwrap();
        body.extend_from_slice(b" trailing");
        assert!(decode_system_one(&body).is_err());
    }

    #[test]
    fn reports_precise_paths() {
        let mut v = sample();
        v["answers"]["department"]
            .as_object_mut()
            .unwrap()
            .remove("confidence");
        assert_eq!(
            decode(v).err().unwrap().path,
            "answers.department.confidence"
        );

        let mut v = sample();
        v["answers"]["frustration"]["probabilities"]["1"] = json!("high");
        assert_eq!(
            decode(v).err().unwrap().path,
            "answers.frustration.probabilities.1"
        );

        let mut v = sample();
        v["answers"]["is_urgent"]["type"] = json!(7);
        assert_eq!(decode(v).err().unwrap().path, "answers.is_urgent.type");

        let mut v = sample();
        v.as_object_mut().unwrap().remove("model");
        assert_eq!(decode(v).err().unwrap().path, "model");
    }
}
