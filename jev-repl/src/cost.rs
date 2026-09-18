//! What a session costs: the tokens a request carries, the tokens its answers bring back, and the
//! money that is at rates you supply.
//!
//! Nothing here calls the API. Token counts are an estimate — roughly four characters to a token,
//! JSON punctuation in pairs — so treat them as an order of magnitude, not an invoice. The answer
//! side is not guesswork about length: a System One answer has the shape the question asks for, so
//! the estimate prices the real shape, which is why a choice over eight labels costs more to
//! answer than a noul.
//!
//! Rates are yours to set, in dollars per million tokens, because the price of a model is not
//! something an SDK should hardcode: `:cost 0.20/1.00` in the REPL, or `JEV_PRICE=0.20/1.00` in
//! the environment.

use serde_json::{Value, json};
use typesafe::Usage;

use crate::mock;
use crate::session::Session;

/// Environment variable holding `<input>/<output>` dollars per million tokens.
pub const PRICE_ENV: &str = "JEV_PRICE";

/// A question shape this crate does not model: assume an answer the size of a noul's.
const ASSUMED_ANSWER: &str = r#"{"name":{"type":"noul","noul":0.123}}"#;

/// Dollars per million tokens, one rate for what goes up and one for what comes back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Dollars per million input tokens.
    pub input: f64,
    /// Dollars per million output tokens.
    pub output: f64,
}

/// What one question adds to a request and to the answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionEstimate {
    pub name: String,
    /// `noul`, `choice`, `score`, or whatever a hand-built question object calls itself.
    pub kind: String,
    /// Tokens the question itself contributes to the request.
    pub input_tokens: usize,
    /// Tokens its answer is expected to contribute to the response.
    pub output_tokens: usize,
    /// True when the answer shape could not be derived and a noul-sized answer was assumed.
    pub assumed: bool,
}

/// The token side of a call: where they go, and how many there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimate {
    pub model: String,
    /// Tokens in the state being judged.
    pub state_tokens: usize,
    /// Tokens in the keys, braces and model name wrapped around the request.
    pub envelope_tokens: usize,
    /// Tokens in the braces wrapped around the answers.
    pub answer_envelope_tokens: usize,
    pub questions: Vec<QuestionEstimate>,
    /// State, questions and envelope together.
    pub input_tokens: usize,
    /// Every expected answer, plus its envelope.
    pub output_tokens: usize,
}

/// Dollars, split the way the rates are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub total: f64,
}

/// Tokens in a piece of text, estimated: a run of letters is a token per four characters, digits
/// run denser, a run of punctuation pairs up the way `":"` and `"},` do in a real vocabulary, and
/// a CJK character is a token on its own. Whitespace rides along with the token beside it. No
/// tokenizer is shipped or downloaded to do this; the API reports the real counts in `usage` once
/// a call has been made.
pub fn estimate_tokens(text: &str) -> usize {
    /// The run of like characters being counted; flushing it turns the run into tokens.
    #[derive(Default)]
    struct Runs {
        letters: usize,
        digits: usize,
        marks: usize,
    }
    impl Runs {
        fn flush(&mut self) -> usize {
            let tokens =
                self.letters.div_ceil(4) + self.digits.div_ceil(3) + self.marks.div_ceil(2);
            *self = Runs::default();
            tokens
        }
    }

    let mut tokens = 0;
    let mut runs = Runs::default();
    for ch in text.chars() {
        if ch.is_whitespace() {
            // Whitespace rides along with the token beside it, so it only ends a run.
            tokens += runs.flush();
        } else if ch as u32 >= 0x2e80 {
            // Anything above the CJK block is a character per token or worse; Latin is far denser.
            tokens += runs.flush() + 1;
        } else if ch.is_ascii_digit() {
            if runs.letters > 0 || runs.marks > 0 {
                tokens += runs.flush();
            }
            runs.digits += 1;
        } else if ch.is_alphabetic() {
            if runs.digits > 0 || runs.marks > 0 {
                tokens += runs.flush();
            }
            runs.letters += 1;
        } else {
            if runs.letters > 0 || runs.digits > 0 {
                tokens += runs.flush();
            }
            runs.marks += 1;
        }
    }
    tokens + runs.flush()
}

/// Tokens in a JSON value, as it goes on the wire.
pub fn estimate_json_tokens(value: &Value) -> usize {
    estimate_tokens(&value.to_string())
}

/// Estimate one call: what the session sends, and what its answers come back as.
pub fn estimate(session: &Session, model: &str) -> Estimate {
    let state_tokens = estimate_json_tokens(&session.state);
    let envelope_tokens =
        estimate_json_tokens(&json!({"state": "", "model": model, "questions": {}}));
    let answer_envelope_tokens = estimate_json_tokens(&json!({"model": model, "answers": {}}));

    let questions: Vec<QuestionEstimate> = session
        .questions
        .iter()
        .map(|(name, question)| {
            let json = serde_json::to_value(question).unwrap_or(Value::Null);
            let kind = json
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("raw")
                .to_owned();
            let shape = mock::answer(&session.state, name, &json).map(|answer| {
                let mut map = serde_json::Map::new();
                map.insert(name.clone(), mock::answer_json(&answer));
                Value::Object(map)
            });
            QuestionEstimate {
                name: name.clone(),
                kind,
                input_tokens: estimate_tokens(&format!("{}:{}", json!(name), json)),
                output_tokens: match &shape {
                    Some(value) => estimate_json_tokens(value),
                    None => estimate_tokens(ASSUMED_ANSWER),
                },
                assumed: shape.is_none(),
            }
        })
        .collect();

    let input_tokens =
        state_tokens + envelope_tokens + questions.iter().map(|q| q.input_tokens).sum::<usize>();
    let output_tokens =
        answer_envelope_tokens + questions.iter().map(|q| q.output_tokens).sum::<usize>();
    Estimate {
        model: model.to_owned(),
        state_tokens,
        envelope_tokens,
        answer_envelope_tokens,
        questions,
        input_tokens,
        output_tokens,
    }
}

/// Price a pair of token counts.
pub fn price(input_tokens: u64, output_tokens: u64, rates: Rates) -> Cost {
    let input = input_tokens as f64 / 1_000_000.0 * rates.input;
    let output = output_tokens as f64 / 1_000_000.0 * rates.output;
    Cost {
        input,
        output,
        total: input + output,
    }
}

/// Price an estimate.
pub fn price_estimate(estimate: &Estimate, rates: Rates) -> Cost {
    price(
        estimate.input_tokens as u64,
        estimate.output_tokens as u64,
        rates,
    )
}

/// Price what a call actually used. `None` when the API reported no counts, because a made-up
/// number is worse than none.
pub fn price_usage(usage: &Usage, rates: Rates) -> Option<Cost> {
    Some(price(usage.input_tokens?, usage.output_tokens?, rates))
}

/// `0.20/1.00`, `0.20 1.00`, `$0.20, $1.00` — input first, output second, per million tokens.
pub fn parse_rates(text: &str) -> Result<Rates, String> {
    let parts: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '/' || c == ',')
        .map(|p| p.trim().trim_start_matches('$'))
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 2 {
        return Err(
            "Two rates, input then output, in dollars per million tokens: :cost 0.20/1.00"
                .to_owned(),
        );
    }
    let mut values = [0.0f64; 2];
    for (slot, part) in values.iter_mut().zip(parts) {
        match part.parse::<f64>() {
            Ok(n) if n.is_finite() && n >= 0.0 => *slot = n,
            _ => return Err(format!("{text:?} is not a pair of dollar amounts.")),
        }
    }
    Ok(Rates {
        input: values[0],
        output: values[1],
    })
}

/// Rates from `JEV_PRICE`; `None` when it is unset or malformed.
pub fn rates_from_env() -> Option<Rates> {
    rates_from_str(&std::env::var(PRICE_ENV).ok()?)
}

/// Rates from a stored string; `None` when it is empty or malformed.
pub fn rates_from_str(value: &str) -> Option<Rates> {
    if value.trim().is_empty() {
        return None;
    }
    parse_rates(value).ok()
}

/// `$0.20/$1.00 per Mtok`.
pub fn format_rates(rates: Rates) -> String {
    format!(
        "${}/${} per Mtok",
        amount(rates.input),
        amount(rates.output)
    )
}

/// The same pair as `JEV_PRICE` takes: `0.20/1.00`.
pub fn rates_value(rates: Rates) -> String {
    format!("{}/{}", amount(rates.input), amount(rates.output))
}

/// Dollars, with enough decimals to be readable at the size a single call costs.
pub fn usd(amount: f64) -> String {
    if amount == 0.0 {
        return "$0".to_owned();
    }
    if amount < 0.000_001 {
        return "<$0.000001".to_owned();
    }
    let digits: usize = if amount >= 1.0 {
        2
    } else if amount >= 0.01 {
        4
    } else {
        6
    };
    format!("${amount:.digits$}")
}

/// Rates are dollars: two decimals unless the rate is finer than a cent.
fn amount(n: f64) -> String {
    if (n * 100.0).fract() == 0.0 {
        format!("{n:.2}")
    } else {
        format!("{n}")
    }
}
