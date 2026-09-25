//! Typed questions: [`Noul`], [`Choice`] and [`Score`].
//!
//! Instructions, option descriptions and score levels accept any JSON value (`&str`, `String`,
//! or `serde_json::json!({...})` for structured rubrics), per the API's "advanced structure" support.

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::error::{Error, Result};

/// A yes/no question; the answer is the probability of "yes".
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename = "noul")]
pub struct Noul {
    /// The question to evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Optional descriptions of what yes and no mean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

/// Descriptions of the yes (`true`) and no (`false`) outcomes of a [`Noul`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
pub struct NoulCriteria {
    /// What a yes (value near 1) means.
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<Value>,
    /// What a no (value near 0) means.
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<Value>,
}

impl Noul {
    /// A yes/no question with the given instructions.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: None,
        }
    }

    /// Describe what a yes means.
    #[must_use]
    pub fn when_true(mut self, description: impl Into<Value>) -> Self {
        self.criteria.get_or_insert_with(Default::default).yes = Some(description.into());
        self
    }

    /// Describe what a no means.
    #[must_use]
    pub fn when_false(mut self, description: impl Into<Value>) -> Self {
        self.criteria.get_or_insert_with(Default::default).no = Some(description.into());
        self
    }
}

/// Pick one option from a set you define.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename = "choice")]
pub struct Choice {
    /// What the model should decide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Option label → description (`None` is sent as `null`: an undescribed option).
    pub criteria: IndexMap<String, Option<Value>>,
}

impl Choice {
    /// A choice with instructions and no options yet; add them with [`Choice::option`].
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: IndexMap::new(),
        }
    }

    /// A choice between undescribed labels.
    pub fn from_labels<I, S>(instructions: impl Into<Value>, labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut c = Self::new(instructions);
        c.criteria
            .extend(labels.into_iter().map(|l| (l.into(), None)));
        c
    }

    /// Add an option with a description.
    #[must_use]
    pub fn option(mut self, label: impl Into<String>, description: impl Into<Value>) -> Self {
        self.criteria.insert(label.into(), Some(description.into()));
        self
    }

    /// Add an option without a description.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.criteria.insert(label.into(), None);
        self
    }
}

/// Rate the state along ordered levels; the answer is a probability-weighted level index.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename = "score")]
pub struct Score {
    /// What the model should rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Ordered level descriptions; level `i` is index `i`.
    pub criteria: Vec<Value>,
}

impl Score {
    /// A score with instructions and ordered level descriptions.
    pub fn new<I, V>(instructions: impl Into<Value>, levels: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self {
            instructions: Some(instructions.into()),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    /// Append a level.
    #[must_use]
    pub fn level(mut self, description: impl Into<Value>) -> Self {
        self.criteria.push(description.into());
        self
    }
}

/// Any question. `Raw` passes a hand-built JSON object through (after light validation), which is
/// useful for fields this SDK version does not model yet.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Question {
    /// See [`Noul`].
    Noul(Noul),
    /// See [`Choice`].
    Choice(Choice),
    /// See [`Score`].
    Score(Score),
    /// A JSON object with a non-empty string `type`.
    Raw(Value),
}

impl Serialize for Question {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Question::Noul(q) => q.serialize(s),
            Question::Choice(q) => q.serialize(s),
            Question::Score(q) => q.serialize(s),
            Question::Raw(v) => v.serialize(s),
        }
    }
}

impl From<Noul> for Question {
    fn from(q: Noul) -> Self {
        Question::Noul(q)
    }
}
impl From<Choice> for Question {
    fn from(q: Choice) -> Self {
        Question::Choice(q)
    }
}
impl From<Score> for Question {
    fn from(q: Score) -> Self {
        Question::Score(q)
    }
}
impl From<Value> for Question {
    fn from(v: Value) -> Self {
        Question::Raw(v)
    }
}

/// Ordered map of question name → question. Answers come back under the same names.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Questions(IndexMap<String, Question>);

impl Questions {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a question, builder-style.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, question: impl Into<Question>) -> Self {
        self.insert(name, question);
        self
    }

    /// Add a question, or replace the one of the same name (keeping its position). Returns the
    /// question it replaced, as `HashMap::insert` does.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        question: impl Into<Question>,
    ) -> Option<Question> {
        self.0.insert(name.into(), question.into())
    }

    /// The question named `name`.
    pub fn get(&self, name: &str) -> Option<&Question> {
        self.0.get(name)
    }

    /// Number of questions.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no questions.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Question)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Reject what the Python SDK rejects locally; everything else is left to server validation.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.0.is_empty() {
            return Err(Error::invalid_request("at least one question is required"));
        }
        for (name, q) in &self.0 {
            match q {
                Question::Score(s) if s.criteria.is_empty() => return Err(empty_score(name)),
                Question::Choice(c) if c.criteria.is_empty() => return Err(empty_choice(name)),
                Question::Raw(v) => validate_raw(name, v)?,
                _ => {}
            }
        }
        Ok(())
    }
}

fn empty_score(name: &str) -> Error {
    Error::invalid_request(format!(
        "score question \"{name}\" has no criteria; at least one score is required"
    ))
}

fn empty_choice(name: &str) -> Error {
    Error::invalid_request(format!(
        "choice question \"{name}\" has no criteria; at least one option is required"
    ))
}

fn validate_raw(name: &str, v: &Value) -> Result<()> {
    let ty = v
        .as_object()
        .and_then(|o| o.get("type"))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            Error::invalid_request(format!(
                "question \"{name}\" must be a question object or a JSON object with a nonempty string \"type\""
            ))
        })?;
    if matches!(ty, "choice" | "score") {
        let criteria = v.get("criteria").ok_or_else(|| {
            Error::invalid_request(format!("question \"{name}\" requires \"criteria\""))
        })?;
        let empty = match criteria {
            Value::Null => true,
            Value::Bool(b) => !b,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Number(n) => n.as_f64() == Some(0.0),
        };
        if empty {
            return Err(if ty == "score" {
                empty_score(name)
            } else {
                empty_choice(name)
            });
        }
    }
    Ok(())
}

impl<K: Into<String>, Q: Into<Question>> FromIterator<(K, Q)> for Questions {
    fn from_iter<T: IntoIterator<Item = (K, Q)>>(iter: T) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, q)| (k.into(), q.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, Q: Into<Question>> Extend<(K, Q)> for Questions {
    /// Add (or replace) each question, as [`Questions::insert`] does.
    fn extend<T: IntoIterator<Item = (K, Q)>>(&mut self, iter: T) {
        self.0
            .extend(iter.into_iter().map(|(k, q)| (k.into(), q.into())));
    }
}

/// Name and question, in insertion order.
impl IntoIterator for Questions {
    type Item = (String, Question);
    type IntoIter = indexmap::map::IntoIter<String, Question>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// The same items as [`Questions::iter`].
impl<'a> IntoIterator for &'a Questions {
    type Item = (&'a str, &'a Question);
    type IntoIter = std::iter::Map<
        indexmap::map::Iter<'a, String, Question>,
        fn((&'a String, &'a Question)) -> (&'a str, &'a Question),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl<K: Into<String>, Q: Into<Question>, const N: usize> From<[(K, Q); N]> for Questions {
    fn from(arr: [(K, Q); N]) -> Self {
        arr.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_like_the_api_reference() {
        let q = Questions::new()
            .with(
                "department",
                Choice::new("Which team should handle this")
                    .option("billing", "Payment or subscription issues")
                    .label("other"),
            )
            .with(
                "frustration",
                Score::new("How frustrated", ["Calm", "Angry"]),
            )
            .with(
                "is_urgent",
                Noul::new("Urgent?").when_true("Explicitly time-sensitive"),
            )
            .with("bare", Noul::default());
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "department": {"type": "choice", "instructions": "Which team should handle this",
                    "criteria": {"billing": "Payment or subscription issues", "other": null}},
                "frustration": {"type": "score", "instructions": "How frustrated", "criteria": ["Calm", "Angry"]},
                "is_urgent": {"type": "noul", "instructions": "Urgent?", "criteria": {"true": "Explicitly time-sensitive"}},
                "bare": {"type": "noul"}
            })
        );
        // insertion order is preserved on the wire
        let keys: Vec<_> = q.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["department", "frustration", "is_urgent", "bare"]);
    }

    #[test]
    fn lookup_iteration_and_extend() {
        let mut q: Questions = [("a", Noul::new("a"))].into_iter().collect();
        q.extend([("b", Question::from(Noul::new("b")))]);
        q.extend(vec![(String::from("a"), Score::new("a", ["lo", "hi"]))]);
        assert_eq!(q.len(), 2);
        assert!(matches!(q.get("a"), Some(Question::Score(_))));
        assert_eq!(q.get("b"), Some(&Question::Noul(Noul::new("b"))));
        assert_eq!(q.get("c"), None);

        let borrowed: Vec<&str> = (&q).into_iter().map(|(k, _)| k).collect();
        assert_eq!(borrowed, ["a", "b"]);
        let mut names = Vec::new();
        for (name, _) in &q {
            names.push(name);
        }
        assert_eq!(names, borrowed);
        let owned: Vec<String> = q.clone().into_iter().map(|(k, _)| k).collect();
        assert_eq!(owned, ["a", "b"]);
        // Round-trips through the owned iterator without losing order or content.
        assert_eq!(q.clone().into_iter().collect::<Questions>(), q);

        // `insert` hands back what it replaced.
        let mut r = Questions::new();
        assert_eq!(r.insert("c", Noul::new("c")), None);
        assert_eq!(r.insert("c", Noul::new("d")), Some(Noul::new("c").into()));
        assert_eq!(r.get("c"), Some(&Question::Noul(Noul::new("d"))));
    }

    #[test]
    fn structured_instructions() {
        let q = Score::new(
            json!({"task": "rate", "focus": ["tone"]}),
            [json!({"level": "low"}), json!("high")],
        );
        assert_eq!(
            serde_json::to_value(Question::from(q)).unwrap(),
            json!({"type": "score", "instructions": {"task": "rate", "focus": ["tone"]},
                   "criteria": [{"level": "low"}, "high"]})
        );
    }

    #[test]
    fn validation() {
        assert!(Questions::new().validate().is_err());
        assert!(
            Questions::from([("s", Score::new("x", Vec::<Value>::new()))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"instructions": "x"}))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"type": ""}))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("c", Choice::new("x"))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"type": "choice"}))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"type": "choice", "criteria": {}}))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"type": "score", "criteria": []}))])
                .validate()
                .is_err()
        );
        assert!(
            Questions::from([("r", json!({"type": "noul", "future_field": 1}))])
                .validate()
                .is_ok()
        );
        assert!(
            Questions::from([("r", json!({"type": "choice", "criteria": {"a": null}}))])
                .validate()
                .is_ok()
        );
    }
}
