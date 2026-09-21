//! The thing you are building in the REPL: a `state`, some named questions, a model.

use serde::Serialize;
use serde_json::Value;
use typesafe::{Choice, Noul, Question, Questions, Score};

/// A question under construction, kept in the order it was added (answers come back in that order).
pub type Entry = (String, Question);

/// One turn of a conversation held as the state: who spoke, and what they said.
///
/// A conversation is not a new field on the wire — it is the `state`, shaped as an array. The
/// questions stay fixed and the state grows, which is the whole point: the same rubric, re-read
/// after every reply, so a noul can be watched moving rather than sampled once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// The speaker, when the line names one.
    pub who: Option<String>,
    /// What was said.
    pub said: String,
}

/// Keys a turn's speaker may arrive under, so a transcript from elsewhere still reads as one.
const WHO_KEYS: [&str; 4] = ["who", "role", "speaker", "from"];
/// Keys a turn's text may arrive under, for the same reason.
const SAID_KEYS: [&str; 4] = ["said", "text", "content", "message"];

/// Read a state as a conversation, or `None` when it is not one.
///
/// Only an array whose every element carries some text counts, so a string state, a row of
/// numbers or an object of fields is never mistaken for a thread and quietly reshaped. The key
/// names are read loosely because a transcript pasted in from a chat API is still a transcript.
pub fn turns_of(state: &Value) -> Option<Vec<Turn>> {
    let items = state.as_array().filter(|a| !a.is_empty())?;
    let mut turns = Vec::with_capacity(items.len());
    for item in items {
        let object = item.as_object()?;
        let pick = |keys: [&str; 4]| {
            keys.into_iter()
                .find_map(|k| object.get(k).and_then(Value::as_str))
        };
        let said = pick(SAID_KEYS)?;
        turns.push(Turn {
            who: pick(WHO_KEYS).filter(|w| !w.is_empty()).map(str::to_owned),
            said: said.to_owned(),
        });
    }
    Some(turns)
}

/// Turns as they go on the wire: `who` only when there is one, so nothing empty is paid for.
pub fn turns_to_json(turns: &[Turn]) -> Value {
    Value::Array(
        turns
            .iter()
            .map(|t| {
                let mut map = serde_json::Map::new();
                if let Some(who) = &t.who {
                    map.insert("who".to_owned(), Value::String(who.clone()));
                }
                map.insert("said".to_owned(), Value::String(t.said.clone()));
                Value::Object(map)
            })
            .collect(),
    )
}

/// `customer: The payout failed again` — one turn on one line.
pub fn turn_text(turn: &Turn) -> String {
    match &turn.who {
        Some(who) => format!("{who}: {}", turn.said),
        None => turn.said.clone(),
    }
}

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
        if let Some(turns) = self.turns()
            && let Some(last) = turns.last()
        {
            let plural = if turns.len() == 1 { "" } else { "s" };
            return format!("{} turn{plural} · {}", turns.len(), turn_text(last));
        }
        match &self.state {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// The state read as a conversation, or `None` when it is something else.
    pub fn turns(&self) -> Option<Vec<Turn>> {
        turns_of(&self.state)
    }

    /// Append a turn to the state.
    ///
    /// An empty state starts a thread and a thread grows by one. A state that is plain text
    /// becomes the first turn, because that is how a session usually begins — one message, then
    /// the reply to it. Any other JSON is refused rather than reshaped: whatever it is, it is not
    /// a conversation, and guessing at one would lose it.
    pub fn add_turn(&mut self, turn: Turn) -> Result<Vec<Turn>, String> {
        let Some(mut turns) = self.turns().or_else(|| self.seed_turns()) else {
            return Err(
                "The state is JSON that is not a conversation, so there is no thread to add to."
                    .to_owned(),
            );
        };
        turns.push(turn);
        self.state = turns_to_json(&turns);
        Ok(turns)
    }

    /// Take the last turn back. The last one of all leaves the state empty again.
    pub fn drop_turn(&mut self) -> Option<Turn> {
        let mut turns = self.turns()?;
        let last = turns.pop()?;
        self.state = if turns.is_empty() {
            Value::String(String::new())
        } else {
            turns_to_json(&turns)
        };
        Some(last)
    }

    /// What a thread starts from: nothing, or the text that was already there.
    fn seed_turns(&self) -> Option<Vec<Turn>> {
        if self.state_is_empty() {
            return Some(Vec::new());
        }
        match &self.state {
            Value::String(s) => Some(vec![Turn {
                who: None,
                said: s.clone(),
            }]),
            _ => None,
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

/// `who: what they said`, or just what they said.
///
/// The speaker is the first word and only when that word ends in a colon, so a line typed without
/// one keeps all of its words instead of donating the first to a speaker nobody named.
pub fn parse_turn(args: &str) -> Result<Turn, String> {
    let text = args.trim();
    let example = ":turn customer: The payout failed again";
    if text.is_empty() {
        return Err(format!("A turn needs something said. Try: {example}"));
    }
    let (head, rest) = match text.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (text, ""),
    };
    let Some(who) = head.strip_suffix(':').filter(|w| !w.is_empty()) else {
        return Ok(Turn {
            who: None,
            said: text.to_owned(),
        });
    };
    if rest.is_empty() {
        return Err(format!("Nothing said after {head:?}. Try: {example}"));
    }
    Ok(Turn {
        who: Some(who.to_owned()),
        said: rest.to_owned(),
    })
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
