//! jev with no terminal in the way: a page in, an answer page out.
//!
//! The REPL is the place to shape a request; once it is shaped, the same session is something a
//! script wants — in a pipe, in a Makefile, in CI. Everything here is the pure half of that: text
//! in, text out, no terminal and no process, so a test can drive it without a PTY.
//!
//! ```
//! # use jev_repl::headless;
//! let session = headless::load("A payout failed.\n---\nis_urgent? Conveys urgency").unwrap();
//! println!("{}", headless::request_text(&session, "jev-latest"));
//! ```

use ratatui::text::Line;
use serde_json::Value;
use typesafe::{Answer, Question, SystemOneResponse, Usage};

use crate::cost::Rates;
use crate::format::{answer_lines, cost_lines};
use crate::session::Session;
use crate::{codegen, cost, mock, session, sketch};

/// The subcommands that run without a terminal, and what each one prints.
pub const COMMANDS: &[(&str, &str)] = &[
    ("run", "send the request and print the answers"),
    (
        "eval",
        "run a page over a file of labelled cases and score the answers",
    ),
    ("json", "the exact request body this session POSTs"),
    (
        "cost",
        "what a call costs, per question and on both sides of the wire",
    ),
    ("rust", "the session as a program against typesafe-ai-sdk"),
    ("check", "parse the input and report what is wrong with it"),
];

/// Whether `word` names a subcommand, so `jev <word>` is not mistaken for a flag or a path.
pub fn is_command(word: &str) -> bool {
    COMMANDS.iter().any(|(name, _)| *name == word)
}

/// One question's answer, or `None` when nothing answered it.
pub type Answered = (String, Option<Answer>);

/// Read a session from a sketch page or a request body.
///
/// Which one it is comes from the text, not the file name: stdin has no extension, and a here-doc
/// piped in should behave the same as the file it was copied from. A leading `{` is a request
/// body; anything else is a page.
pub fn load(text: &str) -> Result<Session, String> {
    if text.trim().is_empty() {
        return Err("Nothing to read: the input is empty.".to_owned());
    }
    if text.trim_start().starts_with('{') {
        return session::from_body(text);
    }
    let page = sketch::parse(text);
    match page.problems.first() {
        Some(problem) => Err(format!("line {}: {}", problem.line + 1, problem.message)),
        None => Ok(page.to_session()),
    }
}

/// What `jev check` says about a page: every problem, not just the first.
pub fn check_text(text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("Nothing to read: the input is empty.".to_owned());
    }
    if text.trim_start().starts_with('{') {
        return session::from_body(text).map(|s| describe(&s));
    }
    let page = sketch::parse(text);
    if !page.ok() {
        return Err(page
            .problems
            .iter()
            .map(|p| format!("line {}: {}", p.line + 1, p.message))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    Ok(describe(&page.to_session()))
}

/// `3 questions: is_urgent (noul), department (choice)` — enough to see the parse landed right.
fn describe(session: &Session) -> String {
    let n = session.questions.len();
    let kinds = session
        .questions
        .iter()
        .map(|(name, q)| format!("{name} ({})", kind_of(q)))
        .collect::<Vec<_>>()
        .join(", ");
    let plural = if n == 1 { "" } else { "s" };
    let head = if kinds.is_empty() {
        format!("{n} question{plural}")
    } else {
        format!("{n} question{plural}: {kinds}")
    };
    if session.state_is_empty() {
        return format!("{head}\nno state — pass --state <text> before sending");
    }
    match session.turns() {
        Some(turns) => {
            let plural = if turns.len() == 1 { "" } else { "s" };
            format!(
                "{head}\nthe state is a conversation of {} turn{plural}",
                turns.len()
            )
        }
        None => head,
    }
}

/// The wire `type` of a question, for the one-line summary `check` prints.
fn kind_of(question: &Question) -> String {
    match question {
        Question::Noul(_) => "noul".to_owned(),
        Question::Choice(_) => "choice".to_owned(),
        Question::Score(_) => "score".to_owned(),
        Question::Raw(value) => value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("raw")
            .to_owned(),
        // `Question` is non-exhaustive: a shape this build does not know still has a name.
        _ => "question".to_owned(),
    }
}

/// Why this session cannot be sent yet, if it cannot.
pub fn sendable(session: &Session) -> Option<String> {
    if session.questions.is_empty() {
        return Some("No questions: the request would ask nothing.".to_owned());
    }
    if session.state_is_empty() {
        return Some("No state: pass --state <text>, or put one above the `---`.".to_owned());
    }
    None
}

/// The exact body the SDK would POST.
pub fn request_text(session: &Session, model: &str) -> String {
    format!("{}\n", session.request_json(model))
}

/// Simulated answers, the same deterministic ones the REPL shows offline.
pub fn mock_answers(session: &Session) -> Vec<Answered> {
    session
        .questions
        .iter()
        .map(|(name, q)| {
            let json = serde_json::to_value(q).unwrap_or(Value::Null);
            (name.clone(), mock::answer(&session.state, name, &json))
        })
        .collect()
}

/// The answers a live response carries, lined up with the questions that were asked.
pub fn live_answers(session: &Session, response: &SystemOneResponse) -> Vec<Answered> {
    session
        .questions
        .iter()
        .map(|(name, _)| (name.clone(), response.answers.get(name).cloned()))
        .collect()
}

/// The answers a cached body carries, lined up with the questions that were asked.
///
/// The wire body is all the cache keeps, and `SystemOneResponse` cannot be rebuilt outside the SDK
/// crate, so this is [`live_answers`] for a response that arrived from disk instead of the
/// network. `None` means the file is not a System One body, which is a cache miss and not an error.
pub fn cached_answers(session: &Session, body: &Value) -> Option<(Vec<Answered>, Option<Usage>)> {
    let object = body.as_object()?;
    object.get("model")?.as_str()?;
    let mut decoded: Vec<(String, Answer)> = Vec::new();
    for (name, value) in object.get("answers")?.as_object()? {
        let answer = match value.get("type")?.as_str()? {
            "noul" => Answer::Noul(serde_json::from_value(value.clone()).ok()?),
            "choice" => Answer::Choice(serde_json::from_value(value.clone()).ok()?),
            "score" => Answer::Score(serde_json::from_value(value.clone()).ok()?),
            // An answer this build does not model is skipped, the way the SDK's decoder skips it.
            _ => continue,
        };
        decoded.push((name.clone(), answer));
    }
    let answers = session
        .questions
        .iter()
        .map(|(name, _)| {
            let answer = decoded
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, a)| a.clone());
            (name.clone(), answer)
        })
        .collect();
    let usage = object
        .get("usage")
        .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
    Some((answers, usage))
}

/// The answer page: the same bars and labels the REPL draws, minus the colour.
pub fn answers_text(answers: &[Answered], threshold: f64) -> String {
    let mut out = String::new();
    for (name, answer) in answers {
        match answer {
            Some(a) => out.push_str(&plain(answer_lines(name, a, threshold))),
            None => out.push_str(&format!(
                "  {name}: no answer came back for this question.\n"
            )),
        }
    }
    out
}

/// The raw body, for `--json`: what arrived live, or the shape a mock answer would have arrived in.
pub fn answers_json(answers: &[Answered], model: &str, raw: Option<&Value>) -> String {
    match raw {
        Some(value) => format!(
            "{}\n",
            serde_json::to_string_pretty(value).unwrap_or_default()
        ),
        None => format!("{}\n", mock::body(answers, model)),
    }
}

/// The token table, priced when rates were supplied.
pub fn cost_text(session: &Session, model: &str, rates: Option<Rates>) -> String {
    let estimate = cost::estimate(session, model);
    let hint = "--price 0.20/1.00 prices it: dollars per million tokens, input then output";
    plain(cost_lines(
        &estimate,
        rates,
        hint,
        cost::thread(session, model).as_ref(),
    ))
}

/// The one-line footer under an answer page: tokens, and money when the rates are known.
pub fn usage_text(
    session: &Session,
    model: &str,
    rates: Option<Rates>,
    usage: Option<&Usage>,
) -> String {
    if let Some(usage) = usage
        && let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens)
    {
        let money = match rates.and_then(|r| cost::price_usage(usage, r)) {
            Some(cost) => format!(" · {}", cost::usd(cost.total)),
            None => String::new(),
        };
        return format!("  {input} in / {output} out tokens{money}\n");
    }
    let estimate = cost::estimate(session, model);
    let money = match rates {
        Some(rates) => format!(
            " · {}",
            cost::usd(cost::price_estimate(&estimate, rates).total)
        ),
        None => String::new(),
    };
    format!(
        "  ≈ {} in / {} out tokens{money} — estimated, nothing was counted\n",
        estimate.input_tokens, estimate.output_tokens
    )
}

/// The session as code, for `jev rust`.
pub fn code_text(session: &Session, model: &str, threshold: f64) -> String {
    let code = codegen::rust(session, model, threshold);
    if code.ends_with('\n') {
        code
    } else {
        format!("{code}\n")
    }
}

/// Styled lines as the plain text a pipe wants.
fn plain(lines: Vec<Line<'static>>) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            out.push_str(span.content.as_ref());
        }
        out.push('\n');
    }
    out
}
