//! Turning answers, questions and errors into styled transcript lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use typesafe::{Answer, Error, Question};

use crate::cost::{Estimate, Rates, Thread, format_rates, price, price_estimate, usd};
use crate::session::Turn;

pub const NOUL: Color = Color::Cyan;
pub const CHOICE: Color = Color::Magenta;
pub const SCORE: Color = Color::Green;
pub const DIM: Color = Color::DarkGray;
pub const WARN: Color = Color::Yellow;
pub const BAD: Color = Color::Red;
pub const ACCENT: Color = Color::LightBlue;

pub fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(DIM))
}

pub fn plain(text: impl Into<String>) -> Line<'static> {
    Line::from(text.into())
}

pub fn styled(text: impl Into<String>, color: Color) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::new().fg(color)))
}

pub fn bold(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().add_modifier(Modifier::BOLD))
}

/// A probability meter. Eighteen columns is enough to read a distribution at a glance.
pub fn bar(p: f64, width: usize) -> String {
    let filled = ((p.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

pub fn color_for(kind: &str) -> Color {
    match kind {
        "noul" => NOUL,
        "choice" => CHOICE,
        "score" => SCORE,
        _ => DIM,
    }
}

/// Render one answer: the number, the distribution it came from, and what it means.
pub fn answer_lines(name: &str, answer: &Answer, threshold: f64) -> Vec<Line<'static>> {
    let kind = answer.kind();
    let mut out = vec![Line::from(vec![
        Span::raw("  "),
        bold(name.to_owned()),
        Span::raw("  "),
        Span::styled(kind.to_owned(), Style::new().fg(color_for(kind))),
    ])];

    match answer {
        Answer::Noul(a) => {
            let yes = a.is_yes(threshold);
            out.push(Line::from(vec![
                Span::raw("    "),
                bold(format!("{:.2}", a.noul)),
                Span::raw("  "),
                Span::styled(bar(a.noul, 18), Style::new().fg(NOUL)),
                Span::raw("  "),
                Span::styled(
                    if yes { "yes" } else { "no" }.to_owned(),
                    Style::new()
                        .fg(if yes { SCORE } else { DIM })
                        .add_modifier(Modifier::BOLD),
                ),
                dim(format!(" at threshold {threshold:.2}")),
            ]));
        }
        Answer::Choice(a) => {
            out.push(Line::from(vec![
                Span::raw("    → "),
                Span::styled(
                    a.choice.clone(),
                    Style::new().fg(CHOICE).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                dim("confidence "),
                confidence_span(a.confidence),
            ]));
            let ranked = a.ranked();
            let pad = ranked.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
            for (label, p) in ranked {
                out.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(
                        format!("{label:pad$}"),
                        Style::new().fg(if label == a.choice { Color::Reset } else { DIM }),
                    ),
                    Span::raw("  "),
                    Span::raw(format!("{p:.2}")),
                    Span::raw("  "),
                    Span::styled(bar(p, 18), Style::new().fg(CHOICE)),
                ]));
            }
        }
        Answer::Score(a) => {
            let top = a.legend.keys().next_back().copied().unwrap_or(0);
            out.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    format!("{:.2}", a.score),
                    Style::new().fg(SCORE).add_modifier(Modifier::BOLD),
                ),
                dim(format!(" of {top}")),
                Span::raw("   "),
                dim("confidence "),
                confidence_span(a.confidence),
                dim(format!(
                    "   most likely level {}",
                    a.most_likely_level()
                        .map(|l| l.to_string())
                        .unwrap_or_else(|| "-".into())
                )),
            ]));
            let labels: Vec<(u32, String)> =
                a.legend.iter().map(|(i, v)| (*i, text_of(v))).collect();
            let pad = labels
                .iter()
                .map(|(_, l)| l.len())
                .max()
                .unwrap_or(0)
                .min(40);
            for (level, label) in labels {
                let p = a.probabilities.get(&level).copied().unwrap_or(0.0);
                let marker = if a.rounded_level() == level {
                    "▸"
                } else {
                    " "
                };
                out.push(Line::from(vec![
                    Span::raw(format!("     {marker} ")),
                    dim(format!("{level} ")),
                    Span::styled(format!("{label:pad$}"), Style::new().fg(Color::Reset)),
                    Span::raw("  "),
                    Span::raw(format!("{p:.2}")),
                    Span::raw("  "),
                    Span::styled(bar(p, 18), Style::new().fg(SCORE)),
                ]));
            }
        }
        _ => out.push(styled(
            format!("    (this SDK version does not model {kind} answers; see :last)"),
            WARN,
        )),
    }
    out
}

fn confidence_span(c: f64) -> Span<'static> {
    let color = if c >= 0.6 {
        SCORE
    } else if c >= 0.35 {
        WARN
    } else {
        BAD
    };
    Span::styled(format!("{c:.2}"), Style::new().fg(color))
}

/// The cost estimate as a small table: which question spends what, and — when rates are set — the
/// money at the bottom. Tokens are estimated, so the numbers are a shape, not a bill. `hint` is how
/// this host sets rates, since the terminal has `:cost` and other hosts have their own. `thread`
/// is what the state costs when it is a conversation, which is more than the last call alone.
pub fn cost_lines(
    estimate: &Estimate,
    rates: Option<Rates>,
    hint: &str,
    thread: Option<&Thread>,
) -> Vec<Line<'static>> {
    let pad = estimate
        .questions
        .iter()
        .map(|q| q.name.chars().count())
        .chain([5, 8, 5])
        .max()
        .unwrap_or(8);
    // `noul` and `choice` fit the seven the table has always been; a long enough thread does not.
    let turns = match thread {
        Some(thread) => {
            let plural = if thread.turns == 1 { "" } else { "s" };
            format!("{} turn{plural}", thread.turns)
        }
        None => String::new(),
    };
    let kind_pad = turns.chars().count().max(7);
    let row = |name: &str, kind: &str, input: String, output: String, color: Option<Color>| {
        Line::from(vec![
            Span::raw("  "),
            match color {
                Some(color) => Span::styled(pad_end(name, pad), Style::new().fg(color)),
                None => Span::raw(pad_end(name, pad)),
            },
            Span::raw("  "),
            dim(pad_end(kind, kind_pad)),
            Span::raw(format!("{input:>6}")),
            Span::raw(format!("{output:>6}")),
        ])
    };

    let mut out = vec![Line::from(vec![
        Span::raw("  "),
        dim(pad_end("", pad)),
        Span::raw("  "),
        dim(pad_end("", kind_pad)),
        dim(format!("{:>6}", "in")),
        dim(format!("{:>6}", "out")),
    ])];
    for q in &estimate.questions {
        out.push(row(
            &q.name,
            &q.kind,
            q.input_tokens.to_string(),
            if q.assumed {
                format!("~{}", q.output_tokens)
            } else {
                q.output_tokens.to_string()
            },
            Some(color_for(&q.kind)),
        ));
    }
    out.push(row(
        "state",
        &turns,
        estimate.state_tokens.to_string(),
        "·".to_owned(),
        None,
    ));
    out.push(row(
        "envelope",
        "",
        estimate.envelope_tokens.to_string(),
        estimate.answer_envelope_tokens.to_string(),
        None,
    ));
    out.push(Line::from(vec![
        Span::raw("  "),
        bold(pad_end("total", pad)),
        Span::raw("  "),
        dim(pad_end("", kind_pad)),
        bold(format!("{:>6}", estimate.input_tokens)),
        bold(format!("{:>6}", estimate.output_tokens)),
        dim(format!(
            "   {} tokens per call",
            estimate.input_tokens + estimate.output_tokens
        )),
    ]));

    let Some(rates) = rates else {
        out.push(Line::from(vec![
            Span::raw("    "),
            dim(format!("no rates set — {hint}")),
        ]));
        out.extend(thread_lines(thread, None));
        return out;
    };
    let cost = price_estimate(estimate, rates);
    out.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(
            usd(cost.total),
            Style::new().fg(SCORE).add_modifier(Modifier::BOLD),
        ),
        dim(" per call   ·   "),
        Span::styled(usd(cost.total * 1000.0), Style::new().fg(SCORE)),
        dim(" per 1,000 calls"),
    ]));
    out.push(Line::from(vec![
        Span::raw("    "),
        dim(format!("at {}", format_rates(rates))),
    ]));
    out.extend(thread_lines(thread, Some(rates)));
    out
}

/// The line under the table when the state is a conversation: a call per turn, all of it added up.
///
/// A thread is sent whole every time it is asked about, so the tokens grow with the square of the
/// turns rather than with the transcript. Saying so once, in numbers, is cheaper than finding out.
fn thread_lines(thread: Option<&Thread>, rates: Option<Rates>) -> Vec<Line<'static>> {
    let Some(thread) = thread else {
        return Vec::new();
    };
    let plural = if thread.turns == 1 { "" } else { "s" };
    let total = thread.input_tokens + thread.output_tokens;
    let mut spans = vec![
        Span::raw("    "),
        dim("asked after every turn: "),
        Span::raw(format!("{} call{plural}", thread.turns)),
        dim(format!(
            ", {} in / {} out",
            thread.input_tokens, thread.output_tokens
        )),
        dim(format!("   {total} tokens for the thread")),
    ];
    if let Some(rates) = rates {
        let spent = price(
            thread.input_tokens as u64,
            thread.output_tokens as u64,
            rates,
        );
        spans.push(dim("   ·   "));
        spans.push(Span::styled(usd(spent.total), Style::new().fg(SCORE)));
    }
    vec![Line::from(spans)]
}

/// One turn of the conversation held as the state, as `:turn` and `:state` echo it back.
pub fn turn_lines(index: usize, turn: &Turn) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        dim(format!("  {}. ", index + 1)),
        match &turn.who {
            Some(who) => Span::styled(who.clone(), Style::new().fg(ACCENT)),
            None => dim("(unattributed)"),
        },
        Span::raw("  "),
        Span::raw(turn.said.clone()),
    ])]
}

fn pad_end(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

/// One line per question, the way it will go on the wire.
pub fn question_lines(index: usize, name: &str, question: &Question) -> Vec<Line<'static>> {
    let v = serde_json::to_value(question).unwrap_or(Value::Null);
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("raw");
    let instructions = v.get("instructions").map(text_of).unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        dim(format!("  {}. ", index + 1)),
        bold(name.to_owned()),
        Span::raw("  "),
        Span::styled(kind.to_owned(), Style::new().fg(color_for(kind))),
        Span::raw("  "),
        dim(instructions),
    ])];
    match (kind, v.get("criteria")) {
        ("choice", Some(Value::Object(map))) => {
            for (label, desc) in map {
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(label.clone(), Style::new().fg(CHOICE)),
                    dim(match desc {
                        Value::Null => String::new(),
                        v => format!(" — {}", text_of(v)),
                    }),
                ]));
            }
        }
        ("score", Some(Value::Array(levels))) => {
            for (i, level) in levels.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(i.to_string(), Style::new().fg(SCORE)),
                    dim(format!(" {}", text_of(level))),
                ]));
            }
        }
        ("noul", Some(Value::Object(map))) => {
            for (key, v) in map {
                let label = if key == "true" { "yes" } else { "no" };
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(label.to_owned(), Style::new().fg(NOUL)),
                    dim(format!(" — {}", text_of(v))),
                ]));
            }
        }
        _ => {}
    }
    lines
}

/// Errors are part of the lesson: show the variant, what it means, and what to do.
pub fn error_lines(err: &Error) -> Vec<Line<'static>> {
    let (variant, advice) = match err {
        Error::Config(_) => ("Config", "Fix the client settings — :key sets an API key."),
        Error::InvalidRequest(_) => (
            "InvalidRequest",
            "Rejected before anything was sent; nothing reached the API.",
        ),
        Error::Api(e) => (
            "Api",
            match e.status {
                401 => "The API key is missing or wrong.",
                403 => "The key is valid but not allowed to do this.",
                422 => "The server rejected the body — check the question criteria.",
                429 => "Rate limited; the SDK already retried with backoff.",
                s if s >= 500 => "Server-side; the SDK already retried with backoff.",
                _ => "Non-2xx after retries.",
            },
        ),
        Error::Connection(_) => (
            "Connection",
            "No response: DNS, TLS, reset or a dropped body.",
        ),
        Error::Timeout(_) => (
            "Timeout",
            "An attempt ran past its per-attempt timeout — see :timeout.",
        ),
        Error::ResponseValidation(_) => (
            "ResponseValidation",
            "A 2xx body was missing required data; field_path points at it.",
        ),
        _ => ("Error", "Unhandled variant."),
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  {variant}  "),
            Style::new().fg(BAD).add_modifier(Modifier::BOLD),
        ),
        Span::raw(err.to_string()),
    ])];
    if let Error::ResponseValidation(e) = err {
        lines.push(Line::from(vec![
            Span::raw("    "),
            dim(format!("field_path: {}", e.field_path)),
        ]));
    }
    if let Some(api) = err.as_api() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            dim(format!("kind: {:?}", api.kind)),
            dim(match api.retry_after() {
                Some(d) => format!("   retry after {:.1}s", d.as_secs_f64()),
                None => String::new(),
            }),
        ]));
    }
    if let Some(id) = err.request_id() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            dim(format!("request_id: {id}")),
        ]));
    }
    lines.push(Line::from(vec![Span::raw("    "), dim(advice)]));
    lines
}

/// JSON strings read better unquoted; everything else stays JSON.
pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
