//! Scoring a rubric against states someone has already labelled.
//!
//! One call tells you what the model said about one state; that is what `jev run` is for. It does
//! not tell you where to put the threshold, or how confident a choice has to be before a script
//! may act on it — the README leaves both calls to you, and this is the module that turns them
//! into a table. Feed it a page and a file of labelled states and it reports what the rubric got
//! right, at every threshold worth trying.
//!
//! Everything here is pure: cases come in as text, the answers come from a function you pass, and
//! the report goes out as lines and JSON. Reading files, hashing request bodies and talking to the
//! API belong to the caller.

use serde_json::Value;
use typesafe::Question;

use crate::format::text_of;
use crate::session::{self, Session};

/// One labelled state: what to judge, and what the rubric should say about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// 1-based line in the cases file, for messages.
    pub line: usize,
    pub id: Option<String>,
    pub state: Value,
    /// Question name → expectation, in the order the file gave them, already checked against the
    /// session's questions. A `Vec` and not a map, because that is the shape `Session` uses for
    /// questions and it saves a dependency.
    pub expect: Vec<(String, Expectation)>,
}

/// What one question is expected to answer, in the shape its kind is scored in.
#[derive(Debug, Clone, PartialEq)]
pub enum Expectation {
    Noul { yes: bool },
    Choice { label: String },
    Score { level: usize },
}

impl Expectation {
    /// The wire `type` of the answer this expectation can be compared with.
    pub fn kind(&self) -> &'static str {
        match self {
            Expectation::Noul { .. } => "noul",
            Expectation::Choice { .. } => "choice",
            Expectation::Score { .. } => "score",
        }
    }
}

/// Parse JSON Lines into cases, checking every expectation against `session`.
///
/// Blank lines are skipped and everything else has to be a case, because a file of labels is worth
/// nothing if a typo silently drops a row. The line number travels with the case: it is what the
/// report names a case by, so a bad row is found by the same number that reported it.
pub fn parse_cases(text: &str, session: &Session) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let line = i + 1;
        let one = parse_case(raw, line, session).map_err(|e| format!("cases line {line}: {e}"))?;
        cases.push(one);
    }
    if cases.is_empty() {
        return Err("the cases file holds no cases.".to_owned());
    }
    Ok(cases)
}

fn parse_case(text: &str, line: usize, session: &Session) -> Result<Case, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or("expected a JSON object with `state` and `expect`.")?;

    let id = match object.get("id") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("`id` must be a string.".to_owned()),
    };

    let state = object
        .get("state")
        .ok_or("missing `state`: a case has to say what to judge.")?;
    if session::is_empty_value(state) {
        return Err("the `state` is empty: there is nothing to judge.".to_owned());
    }

    let wanted = object
        .get("expect")
        .ok_or("missing `expect`: a case has to say what the answer is.")?;
    let wanted = wanted
        .as_object()
        .filter(|map| !map.is_empty())
        .ok_or("`expect` has to name at least one question.")?;

    let mut expect = Vec::with_capacity(wanted.len());
    for (name, value) in wanted {
        let question = session
            .questions
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, q)| q)
            .ok_or_else(|| format!("no question named {name:?} on the page."))?;
        expect.push((name.clone(), expected(name, question, value)?));
    }
    Ok(Case {
        line,
        id,
        state: state.clone(),
        expect,
    })
}

/// Check one expected value against the question it names, and store it the way it is scored.
fn expected(name: &str, question: &Question, value: &Value) -> Result<Expectation, String> {
    match question {
        Question::Noul(_) => match value {
            Value::Bool(yes) => Ok(Expectation::Noul { yes: *yes }),
            other => Err(format!(
                "{name} is a noul: expected true or false, got {other}."
            )),
        },
        Question::Choice(q) => {
            let labels: Vec<&str> = q.criteria.keys().map(String::as_str).collect();
            match value.as_str() {
                Some(label) if labels.contains(&label) => Ok(Expectation::Choice {
                    label: label.to_owned(),
                }),
                _ => Err(format!(
                    "{name} is a choice between {}; got {value}.",
                    labels.join(", ")
                )),
            }
        }
        Question::Score(q) => {
            let top = q.criteria.len().saturating_sub(1);
            if let Value::Number(number) = value {
                let n = number.as_f64().unwrap_or(f64::NAN);
                if n.fract() == 0.0 && (0.0..=top as f64).contains(&n) {
                    return Ok(Expectation::Score { level: n as usize });
                }
                return Err(format!(
                    "{name} is a score: expected a level from 0 to {top}, got {number}."
                ));
            }
            // A level's own text reads better in a cases file than its index does; the first wins.
            let wanted = text_of(value);
            match q.criteria.iter().position(|level| text_of(level) == wanted) {
                Some(at) => Ok(Expectation::Score { level: at }),
                None => Err(format!(
                    "{name} is a score: expected a level from 0 to {top}, or one of its levels; got {value}."
                )),
            }
        }
        // A hand-built question object has no shape to score against, and neither has a kind this
        // build does not know.
        _ => Err(format!(
            "{name} is a raw question: raw questions cannot be scored."
        )),
    }
}

/// The session as one case sends it: the page's questions, the case's state.
pub fn with_state(session: &Session, state: Value) -> Session {
    Session {
        state,
        questions: session.questions.clone(),
        model: session.model.clone(),
    }
}
