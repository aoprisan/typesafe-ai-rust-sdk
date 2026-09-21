//! The thing you are building in the REPL: a `state`, some named questions, a model.

use serde::Serialize;
use serde_json::Value;
use typesafe::{Choice, Noul, Question, Questions, Score};

/// A question under construction, kept in the order it was added (answers come back in that order).
pub type Entry = (String, Question);

/// Everything the next `:ask` will send.
#[derive(Debug, Default, Clone)]
pub struct Session {
    /// The text or JSON the model reasons about.
    pub state: Value,
    /// Named questions, in insertion order.
    pub questions: Vec<Entry>,
    /// Per-session model override; `None` means the client default.
    pub model: Option<String>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: Value::String(String::new()),
            questions: Vec::new(),
            model: None,
        }
    }

    pub fn state_is_empty(&self) -> bool {
        is_empty_value(&self.state)
    }

    /// One-line preview of the state for the side panel.
    pub fn state_preview(&self) -> String {
        match &self.state {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Add a question, or replace one of the same name in place.
    pub fn insert(&mut self, name: String, question: Question) -> bool {
        if let Some(slot) = self.questions.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = question;
            true
        } else {
            self.questions.push((name, question));
            false
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.questions.len();
        self.questions.retain(|(n, _)| n != name);
        self.questions.len() != before
    }

    pub fn to_questions(&self) -> Questions {
        self.questions.iter().cloned().collect()
    }

    /// The exact JSON body the SDK will POST to `/v1/systemone`.
    ///
    /// Serialized through a struct (not a `Value`) so field and question order survive, which is
    /// the whole point of showing it.
    pub fn request_json(&self, model: &str) -> String {
        self.body_json(model, true)
    }

    /// The same body [`Session::request_json`] shows, with the whitespace taken out: what a cache
    /// key hashes.
    pub fn request_json_compact(&self, model: &str) -> String {
        self.body_json(model, false)
    }

    fn body_json(&self, model: &str, pretty: bool) -> String {
        #[derive(Serialize)]
        struct Body<'a> {
            state: &'a Value,
            model: &'a str,
            questions: &'a Questions,
        }
        let questions = self.to_questions();
        let body = Body {
            state: &self.state,
            model,
            questions: &questions,
        };
        if pretty {
            serde_json::to_string_pretty(&body)
        } else {
            serde_json::to_string(&body)
        }
        .unwrap_or_else(|e| format!("<unencodable: {e}>"))
    }
}

/// Whether a value is empty enough that there is nothing to judge.
///
/// A case's state is arbitrary JSON and is held to the same bar as a session's, so the rule lives
/// here rather than inside [`Session::state_is_empty`].
pub fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// `name instructions [| yes: ...] [| no: ...]`
pub fn parse_noul(args: &str) -> Result<Entry, String> {
    let (name, rest) = split_name(args, ":noul is_urgent The message conveys urgency")?;
    let mut parts = rest.split('|').map(str::trim);
    let instructions = parts.next().unwrap_or("");
    if instructions.is_empty() {
        return Err(
            "A noul needs instructions: :noul is_urgent The message conveys urgency".into(),
        );
    }
    let mut q = Noul::new(value(instructions));
    for part in parts {
        let (tag, text) = part
            .split_once(':')
            .map(|(t, v)| (t.trim().to_ascii_lowercase(), v.trim()))
            .ok_or_else(|| format!("Expected `yes: …` or `no: …`, got {part:?}."))?;
        match tag.as_str() {
            "yes" | "true" => q = q.when_true(value(text)),
            "no" | "false" => q = q.when_false(value(text)),
            _ => return Err(format!("Unknown criterion {tag:?}; use `yes:` or `no:`.")),
        }
    }
    Ok((name, q.into()))
}

/// `name instructions | label=description | bare_label | …`
pub fn parse_choice(args: &str) -> Result<Entry, String> {
    let (name, rest) = split_name(
        args,
        ":choice department Which team handles this | billing=Payments | technical=Bugs",
    )?;
    let mut parts = rest.split('|').map(str::trim);
    let instructions = parts.next().unwrap_or("");
    if instructions.is_empty() {
        return Err("A choice needs instructions before the first `|`.".into());
    }
    let mut q = Choice::new(value(instructions));
    let mut count = 0;
    for part in parts.filter(|p| !p.is_empty()) {
        q = match part.split_once('=') {
            Some((label, desc)) => q.option(label.trim(), value(desc.trim())),
            None => q.label(part),
        };
        count += 1;
    }
    if count < 2 {
        return Err(
            "A choice needs at least two options: … | billing=Payments | technical=Bugs".into(),
        );
    }
    Ok((name, q.into()))
}

/// `name instructions | level | level | …`
pub fn parse_score(args: &str) -> Result<Entry, String> {
    let (name, rest) = split_name(
        args,
        ":score frustration How frustrated they are | Calm | Annoyed | Furious",
    )?;
    let mut parts = rest.split('|').map(str::trim);
    let instructions = parts.next().unwrap_or("");
    if instructions.is_empty() {
        return Err("A score needs instructions before the first `|`.".into());
    }
    let levels: Vec<Value> = parts.filter(|p| !p.is_empty()).map(value).collect();
    if levels.len() < 2 {
        return Err(
            "A score needs at least two ordered levels: … | Calm | Annoyed | Furious".into(),
        );
    }
    Ok((name, Score::new(value(instructions), levels).into()))
}

/// `name {json}` — a hand-built question object, like `Question::Raw`.
pub fn parse_raw(args: &str) -> Result<Entry, String> {
    let (name, rest) = split_name(
        args,
        r#":raw tone {"type": "noul", "instructions": "Polite?"}"#,
    )?;
    let v: Value = serde_json::from_str(&rest).map_err(|e| format!("Not valid JSON: {e}"))?;
    Ok((name, Question::Raw(v)))
}

fn split_name(args: &str, example: &str) -> Result<(String, String), String> {
    let args = args.trim();
    let (name, rest) = args
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("Missing name or body. Try: {example}"))?;
    if name.is_empty() {
        return Err(format!("Missing a question name. Try: {example}"));
    }
    Ok((name.to_owned(), rest.trim().to_owned()))
}

/// Instructions, descriptions and levels accept any JSON, so `{…}`/`[…]` is parsed as such and
/// anything else is sent as a plain string.
pub fn value(text: &str) -> Value {
    let t = text.trim();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return v;
    }
    Value::String(t.to_owned())
}

/// Rebuild a session from a saved request body (`:open`), keeping typed questions where the
/// `type` is one this SDK models.
pub fn from_body(text: &str) -> Result<Session, String> {
    let body: Value = serde_json::from_str(text).map_err(|e| format!("Not valid JSON: {e}"))?;
    let obj = body.as_object().ok_or("Expected a JSON object.")?;
    let questions = obj
        .get("questions")
        .and_then(Value::as_object)
        .ok_or("Expected a `questions` object.")?;
    Ok(Session {
        state: obj.get("state").cloned().unwrap_or(Value::Null),
        model: obj.get("model").and_then(Value::as_str).map(str::to_owned),
        questions: questions
            .iter()
            .map(|(name, q)| (name.clone(), question_from_json(q)))
            .collect(),
    })
}

/// Map a wire question back to a typed one; anything unfamiliar stays [`Question::Raw`].
pub fn question_from_json(v: &Value) -> Question {
    let instructions = v.get("instructions").cloned();
    let criteria = v.get("criteria");
    match v.get("type").and_then(Value::as_str) {
        Some("noul") => {
            let mut q = match instructions {
                Some(i) => Noul::new(i),
                None => Noul::default(),
            };
            if let Some(yes) = criteria.and_then(|c| c.get("true")) {
                q = q.when_true(yes.clone());
            }
            if let Some(no) = criteria.and_then(|c| c.get("false")) {
                q = q.when_false(no.clone());
            }
            q.into()
        }
        Some("choice") => {
            let mut q = Choice::new(instructions.unwrap_or(Value::Null));
            if let Some(map) = criteria.and_then(Value::as_object) {
                for (label, desc) in map {
                    q = match desc {
                        Value::Null => q.label(label.clone()),
                        d => q.option(label.clone(), d.clone()),
                    };
                }
            }
            q.into()
        }
        Some("score") => {
            let levels = criteria
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Score::new(instructions.unwrap_or(Value::Null), levels).into()
        }
        _ => Question::Raw(v.clone()),
    }
}
