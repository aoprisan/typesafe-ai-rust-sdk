//! A rubric followed across a conversation: every question asked again after each turn, and what
//! it said drawn as one line per question.
//!
//! A single call over a thread says where the answer ended up. It does not say when it got there —
//! whether urgency was clear from the first message or only arrived with the third — and that is
//! usually the thing worth knowing about a rubric meant to run on a live conversation. This module
//! is the pure half: the prefixes to ask, and the lines to draw from what came back.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use typesafe::{Answer, Question};

use crate::evaluate::two;
use crate::format::{bold, color_for, dim};
use crate::headless::Answered;
use crate::session::{Session, turns_to_json};

/// The session once per turn: the first turn, the first two, and so on up to the whole thread.
/// Empty when the state is not a conversation.
///
/// Each prefix is written with `turns_to_json`, the same way `cost::thread` prices it and a cases
/// file labelled `by_turn` sends it, so the three agree on what "the conversation after turn 2" is.
pub fn prefixes(session: &Session) -> Vec<Session> {
    let Some(turns) = session.turns() else {
        return Vec::new();
    };
    (1..=turns.len())
        .map(|n| {
            let mut so_far = session.clone();
            so_far.state = turns_to_json(&turns[..n]);
            so_far
        })
        .collect()
}

/// One question across the turns.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub kind: &'static str,
    /// The number at each turn: a noul's probability, the probability of the label a choice ended
    /// on, a score's weighted level. Empty when some turn came back without an answer.
    pub values: Vec<f64>,
    /// What a full bar means: 1 for a probability, the highest level for a score.
    pub top: f64,
    /// What the question said at each turn, in words: `yes`, a label, `level 2`.
    pub readings: Vec<String>,
    /// For a choice, the label it chose at the last turn — the one `values` follows.
    pub label: Option<String>,
}

/// The wire `type` of a question, with a hand-built object as `raw`.
fn kind_of(question: &Question) -> &'static str {
    match question {
        Question::Noul(_) => "noul",
        Question::Choice(_) => "choice",
        Question::Score(_) => "score",
        _ => "raw",
    }
}

/// Line each question up across the turns.
///
/// A choice is followed through the label it ended on, so the line shows that label gaining ground
/// (or not) rather than jumping between whichever label led at each turn; the readings still name
/// the leader turn by turn, which is where a change of mind shows.
pub fn series(session: &Session, per_turn: &[Vec<Answered>], threshold: f64) -> Vec<Series> {
    session
        .questions
        .iter()
        .map(|(name, question)| {
            let kind = kind_of(question);
            let missing = Series {
                name: name.clone(),
                kind,
                values: Vec::new(),
                top: 1.0,
                readings: Vec::new(),
                label: None,
            };
            let answers: Option<Vec<&Answer>> = per_turn
                .iter()
                .map(|answered| {
                    answered
                        .iter()
                        .find(|(n, _)| n == name)
                        .and_then(|(_, a)| a.as_ref())
                        .filter(|a| a.kind().as_str() == kind)
                })
                .collect();
            let Some(all) = answers.filter(|all| !all.is_empty()) else {
                return missing;
            };
            match all[all.len() - 1] {
                Answer::Noul(_) => {
                    let at = session.threshold_of(name, threshold);
                    let values: Vec<f64> = all
                        .iter()
                        .map(|a| match a {
                            Answer::Noul(a) => a.noul,
                            _ => 0.0,
                        })
                        .collect();
                    let readings = values
                        .iter()
                        .map(|p| if *p >= at { "yes" } else { "no" }.to_owned())
                        .collect();
                    Series {
                        values,
                        readings,
                        ..missing
                    }
                }
                Answer::Choice(last) => {
                    let label = last.choice.clone();
                    Series {
                        values: all
                            .iter()
                            .map(|a| match a {
                                Answer::Choice(a) => a.probability(&label).unwrap_or(0.0),
                                _ => 0.0,
                            })
                            .collect(),
                        readings: all
                            .iter()
                            .map(|a| match a {
                                Answer::Choice(a) => a.choice.clone(),
                                _ => String::new(),
                            })
                            .collect(),
                        label: Some(label),
                        ..missing
                    }
                }
                Answer::Score(last) => Series {
                    values: all
                        .iter()
                        .map(|a| match a {
                            Answer::Score(a) => a.score,
                            _ => 0.0,
                        })
                        .collect(),
                    top: last.legend.len().saturating_sub(1) as f64,
                    readings: all
                        .iter()
                        .map(|a| match a {
                            Answer::Score(a) => format!("level {}", a.rounded_level()),
                            _ => String::new(),
                        })
                        .collect(),
                    ..missing
                },
                _ => missing,
            }
        })
        .collect()
}

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// One block per value, as tall as the value is of `top`. The scale is fixed rather than fitted to
/// the values, so a noul that sits at 0.9 all the way through reads as high and flat, not as noise.
pub fn sparkline(values: &[f64], top: f64) -> String {
    values
        .iter()
        .map(|v| {
            let share = if top <= 0.0 {
                0.0
            } else {
                (v / top).clamp(0.0, 1.0)
            };
            BLOCKS[(share * 7.0).round() as usize]
        })
        .collect()
}

/// `turn 3 yes · turn 4 no`, or `no throughout` when it never changed its mind.
pub fn changes(readings: &[String]) -> String {
    let out: Vec<String> = readings
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] != pair[1])
        .map(|(i, pair)| format!("turn {} {}", i + 2, pair[1]))
        .collect();
    if out.is_empty() {
        format!(
            "{} throughout",
            readings.first().map(String::as_str).unwrap_or("")
        )
    } else {
        out.join(" · ")
    }
}

/// `0.12 → 0.91`, with a choice's label in front and a score's scale behind.
fn summary(one: &Series) -> String {
    let first = two(one.values.first().copied().unwrap_or(0.0));
    let last = two(one.values.last().copied().unwrap_or(0.0));
    match &one.label {
        Some(label) => format!("{label} {first} → {last}"),
        None if one.kind == "score" => format!("{first} → {last} of {}", one.top),
        None => format!("{first} → {last}"),
    }
}

/// One line per question: the spark, where it started and ended, and every turn it changed.
pub fn trend_lines(all: &[Series]) -> Vec<Line<'static>> {
    let width = all
        .iter()
        .map(|one| one.name.chars().count())
        .max()
        .unwrap_or(0);
    let summaries: Vec<String> = all
        .iter()
        .map(|one| {
            if one.values.is_empty() {
                String::new()
            } else {
                summary(one)
            }
        })
        .collect();
    let summary_width = summaries
        .iter()
        .map(|text| text.chars().count())
        .max()
        .unwrap_or(0);
    all.iter()
        .zip(&summaries)
        .map(|(one, summary)| {
            let color = Style::new().fg(color_for(one.kind));
            let mut spans = vec![
                Span::raw("  "),
                bold(pad_end(&one.name, width)),
                Span::raw("  "),
                Span::styled(pad_end(one.kind, 8), color),
            ];
            if one.values.is_empty() {
                spans.push(dim("no answer to follow"));
                return Line::from(spans);
            }
            spans.extend([
                Span::styled(sparkline(&one.values, one.top), color),
                Span::raw("  "),
                Span::raw(pad_end(summary, summary_width)),
                Span::raw("  "),
                dim(changes(&one.readings)),
            ]);
            Line::from(spans)
        })
        .collect()
}

fn pad_end(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - length))
    }
}
