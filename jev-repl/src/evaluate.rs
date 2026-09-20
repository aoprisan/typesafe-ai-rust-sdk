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

use std::future::Future;
use std::sync::Arc;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use typesafe::{Answer, Choice, Question, Usage};

use crate::cost::{self, Cost, Rates};
use crate::format::{BAD, CHOICE, bold, color_for, dim, text_of};
use crate::headless::Answered;
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

/// What one case's request came back as.
#[derive(Debug, Clone)]
pub enum Outcome {
    Ok {
        answers: Vec<Answered>,
        usage: Option<Usage>,
    },
    Failed {
        error: String,
    },
}

/// Send every case through `ask`, at most `concurrency` at a time; results are in case order.
///
/// Every case is spawned at once and a semaphore decides how many are in the air, so a slow case
/// holds up nothing but itself, and each result is written to its own slot: a file of a thousand
/// labels keeps its order however the calls come back.
pub async fn run<F, Fut>(
    session: &Session,
    cases: &[Case],
    ask: F,
    concurrency: usize,
) -> Vec<Outcome>
where
    F: Fn(Session) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut workers = JoinSet::new();
    for (at, one) in cases.iter().enumerate() {
        let session = with_state(session, one.state.clone());
        let ask = ask.clone();
        let permits = Arc::clone(&permits);
        workers.spawn(async move {
            let _permit = permits.acquire_owned().await;
            (at, ask(session).await)
        });
    }

    let mut outcomes: Vec<Option<Outcome>> = vec![None; cases.len()];
    while let Some(joined) = workers.join_next().await {
        // A worker that panicked leaves its slot empty; the run is not lost to one case.
        if let Ok((at, outcome)) = joined {
            outcomes[at] = Some(outcome);
        }
    }
    outcomes
        .into_iter()
        .map(|outcome| {
            outcome.unwrap_or_else(|| Outcome::Failed {
                error: "nothing was sent for this case.".to_owned(),
            })
        })
        .collect()
}

/// One row of a noul's threshold sweep: the confusion counts, and what they come to.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepRow {
    pub threshold: f64,
    pub tp: usize,
    pub fp: usize,
    pub r#fn: usize,
    pub tn: usize,
    pub accuracy: f64,
    /// `None` when nothing was predicted a yes, because a rate over nothing is not zero.
    pub precision: Option<f64>,
    /// `None` when nothing was expected to be a yes.
    pub recall: Option<f64>,
    pub f1: f64,
}

/// One cut of the confidence gate: how much of the set survives it, and how right it is.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRow {
    pub confidence: f64,
    pub coverage: f64,
    /// `None` when no case is confident enough to be counted.
    pub accuracy: Option<f64>,
}

/// The threshold that scored best, and what it scored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Best {
    pub threshold: f64,
    pub f1: f64,
}

/// What one question scored, in the numbers its kind is judged by.
#[derive(Debug, Clone, PartialEq)]
pub enum QuestionReport {
    Noul {
        name: String,
        cases: usize,
        /// Mean squared error of the probability itself, threshold or no threshold.
        brier: f64,
        /// Accuracy at the threshold this run was asked to use.
        accuracy: f64,
        best: Best,
        sweep: Vec<SweepRow>,
    },
    Choice {
        name: String,
        cases: usize,
        accuracy: f64,
        /// The page's options, plus `other` when the model answered something else.
        labels: Vec<String>,
        /// Rows expected, columns predicted.
        confusion: Vec<Vec<usize>>,
        gate: Vec<GateRow>,
    },
    Score {
        name: String,
        cases: usize,
        exact: f64,
        within_one: f64,
        mae: f64,
        gate: Vec<GateRow>,
    },
}

impl QuestionReport {
    pub fn name(&self) -> &str {
        match self {
            QuestionReport::Noul { name, .. }
            | QuestionReport::Choice { name, .. }
            | QuestionReport::Score { name, .. } => name,
        }
    }

    /// The wire `type` of the question this reports on.
    pub fn kind(&self) -> &'static str {
        match self {
            QuestionReport::Noul { .. } => "noul",
            QuestionReport::Choice { .. } => "choice",
            QuestionReport::Score { .. } => "score",
        }
    }

    pub fn cases(&self) -> usize {
        match self {
            QuestionReport::Noul { cases, .. }
            | QuestionReport::Choice { cases, .. }
            | QuestionReport::Score { cases, .. } => *cases,
        }
    }

    /// The accuracy `--min-accuracy` holds a question to: exact agreement at the chosen threshold.
    pub fn accuracy_of(&self) -> f64 {
        match self {
            QuestionReport::Noul { accuracy, .. } | QuestionReport::Choice { accuracy, .. } => {
                *accuracy
            }
            QuestionReport::Score { exact, .. } => *exact,
        }
    }
}

/// A case that never produced a full set of answers, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseError {
    pub case: usize,
    pub id: Option<String>,
    pub message: String,
}

/// The tokens the run spent, counted when the API counted them and estimated when it did not.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated: bool,
    pub cost: Option<Cost>,
}

/// Everything the run found out, with the numbers unrounded.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub model: String,
    pub threshold: f64,
    pub cases: usize,
    pub answered: usize,
    pub errors: Vec<CaseError>,
    pub questions: Vec<QuestionReport>,
    pub usage: ReportUsage,
}

/// What the run was asked for, which the report repeats back.
#[derive(Debug, Clone, Copy)]
pub struct ReportOptions<'a> {
    pub model: &'a str,
    pub threshold: f64,
    pub rates: Option<Rates>,
}

/// The thresholds a sweep always covers; the chosen one joins them when it is not one of these.
const SWEEP: [f64; 9] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];

/// The cuts the confidence gate is read at.
const CUTS: [f64; 5] = [0.0, 0.2, 0.4, 0.6, 0.8];

/// One case that answered everything it was labelled for.
struct Scored<'a> {
    expect: &'a [(String, Expectation)],
    answers: Vec<(String, Answer)>,
    usage: Option<Usage>,
}

impl Scored<'_> {
    fn answer(&self, name: &str) -> Option<&Answer> {
        self.answers.iter().find(|(n, _)| n == name).map(|(_, a)| a)
    }

    fn expects(&self, name: &str) -> Option<&Expectation> {
        self.expect.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }
}

/// Score the outcomes against the cases.
///
/// A case either answered everything it was labelled for or it counts as an error: a half-answered
/// case would quietly skew whichever question it did answer, and a rubric is being judged here.
pub fn report(
    session: &Session,
    cases: &[Case],
    outcomes: &[Outcome],
    options: ReportOptions<'_>,
) -> Report {
    let mut errors: Vec<CaseError> = Vec::new();
    let mut scored: Vec<Scored<'_>> = Vec::new();
    for (at, one) in cases.iter().enumerate() {
        let mut failed = |message: String| {
            errors.push(CaseError {
                case: one.line,
                id: one.id.clone(),
                message,
            });
        };
        let (answers, usage) = match outcomes.get(at) {
            None => {
                failed("nothing was sent for this case.".to_owned());
                continue;
            }
            Some(Outcome::Failed { error }) => {
                failed(error.clone());
                continue;
            }
            Some(Outcome::Ok { answers, usage }) => (answers, usage),
        };
        let answers: Vec<(String, Answer)> = answers
            .iter()
            .filter_map(|(name, answer)| answer.clone().map(|a| (name.clone(), a)))
            .collect();
        if let Some(message) = unscorable(&one.expect, &answers) {
            failed(message);
            continue;
        }
        scored.push(Scored {
            expect: &one.expect,
            answers,
            usage: usage.clone(),
        });
    }

    let mut questions = Vec::new();
    for (name, question) in &session.questions {
        let rows: Vec<&Scored<'_>> = scored
            .iter()
            .filter(|one| one.expects(name).is_some())
            .collect();
        if rows.is_empty() {
            continue;
        }
        match question {
            Question::Noul(_) => questions.push(noul_report(name, &rows, options.threshold)),
            Question::Choice(q) => questions.push(choice_report(name, q, &rows)),
            Question::Score(_) => questions.push(score_report(name, &rows)),
            _ => {}
        }
    }

    let usage = usage_of(session, cases, &scored, options.model, options.rates);
    Report {
        model: options.model.to_owned(),
        threshold: options.threshold,
        cases: cases.len(),
        answered: scored.len(),
        errors,
        questions,
        usage,
    }
}

/// Why this case cannot be scored, if it cannot: the first label that got no answer, or one whose
/// answer came back as another kind.
fn unscorable(expect: &[(String, Expectation)], answers: &[(String, Answer)]) -> Option<String> {
    for (name, expectation) in expect {
        match answers.iter().find(|(n, _)| n == name).map(|(_, a)| a) {
            None => return Some(format!("no answer came back for {name}")),
            Some(answer) if answer.kind() != expectation.kind() => {
                return Some(format!(
                    "{name} came back as a {}, not a {}",
                    answer.kind(),
                    expectation.kind()
                ));
            }
            Some(_) => {}
        }
    }
    None
}

/// Questions whose accuracy is below `bar`, for --min-accuracy.
pub fn below_bar(report: &Report, bar: f64) -> Vec<(String, f64)> {
    report
        .questions
        .iter()
        .filter(|q| q.accuracy_of() < bar)
        .map(|q| (q.name().to_owned(), q.accuracy_of()))
        .collect()
}

fn noul_report(name: &str, rows: &[&Scored<'_>], threshold: f64) -> QuestionReport {
    let points: Vec<(f64, bool)> = rows
        .iter()
        .map(|row| {
            let p = match row.answer(name) {
                Some(Answer::Noul(a)) => a.noul,
                _ => 0.0,
            };
            let yes = matches!(row.expects(name), Some(Expectation::Noul { yes: true }));
            (p, yes)
        })
        .collect();

    let mut thresholds: Vec<f64> = SWEEP.to_vec();
    if !thresholds.contains(&threshold) {
        thresholds.push(threshold);
        thresholds.sort_by(f64::total_cmp);
    }
    let sweep: Vec<SweepRow> = thresholds
        .iter()
        .map(|at| sweep_row(&points, *at))
        .collect();
    let accuracy = sweep
        .iter()
        .find(|row| row.threshold == threshold)
        .map(|row| row.accuracy)
        .unwrap_or(0.0);
    // The sweep is in ascending order and the comparison is strict, so a tie keeps the lowest.
    let mut best = Best {
        threshold,
        f1: f64::NEG_INFINITY,
    };
    for row in &sweep {
        if row.f1 > best.f1 {
            best = Best {
                threshold: row.threshold,
                f1: row.f1,
            };
        }
    }
    let brier = mean(
        points
            .iter()
            .map(|(p, yes)| (p - if *yes { 1.0 } else { 0.0 }).powi(2)),
    );
    QuestionReport::Noul {
        name: name.to_owned(),
        cases: points.len(),
        brier,
        accuracy,
        best,
        sweep,
    }
}

fn sweep_row(points: &[(f64, bool)], threshold: f64) -> SweepRow {
    let (mut tp, mut fp, mut fneg, mut tn) = (0usize, 0usize, 0usize, 0usize);
    for (p, yes) in points {
        match (*p >= threshold, *yes) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fneg += 1,
            (false, false) => tn += 1,
        }
    }
    let denominator = 2 * tp + fp + fneg;
    SweepRow {
        threshold,
        tp,
        fp,
        r#fn: fneg,
        tn,
        accuracy: (tp + tn) as f64 / points.len() as f64,
        precision: (tp + fp > 0).then(|| tp as f64 / (tp + fp) as f64),
        recall: (tp + fneg > 0).then(|| tp as f64 / (tp + fneg) as f64),
        f1: if denominator == 0 {
            0.0
        } else {
            2.0 * tp as f64 / denominator as f64
        },
    }
}

fn choice_report(name: &str, question: &Choice, rows: &[&Scored<'_>]) -> QuestionReport {
    let options: Vec<String> = question.criteria.keys().cloned().collect();
    struct Point {
        predicted: String,
        expected: String,
        confidence: f64,
        right: bool,
    }
    let points: Vec<Point> = rows
        .iter()
        .map(|row| {
            let (predicted, confidence) = match row.answer(name) {
                Some(Answer::Choice(a)) => (a.choice.clone(), a.confidence),
                _ => (String::new(), 0.0),
            };
            let expected = match row.expects(name) {
                Some(Expectation::Choice { label }) => label.clone(),
                _ => String::new(),
            };
            Point {
                right: predicted == expected,
                predicted,
                expected,
                confidence,
            }
        })
        .collect();

    // A label the page never offered still has to land somewhere, or the matrix loses cases.
    let other = points
        .iter()
        .any(|point| !options.contains(&point.predicted));
    let mut labels = options.clone();
    if other {
        labels.push("other".to_owned());
    }
    let confusion: Vec<Vec<usize>> = options
        .iter()
        .map(|expected| {
            labels
                .iter()
                .enumerate()
                .map(|(column, predicted)| {
                    points
                        .iter()
                        .filter(|point| {
                            &point.expected == expected
                                && if other && column == labels.len() - 1 {
                                    !options.contains(&point.predicted)
                                } else {
                                    &point.predicted == predicted
                                }
                        })
                        .count()
                })
                .collect()
        })
        .collect();

    QuestionReport::Choice {
        name: name.to_owned(),
        cases: points.len(),
        accuracy: mean(points.iter().map(|point| f64::from(point.right))),
        labels,
        confusion,
        gate: gate(points.iter().map(|point| (point.confidence, point.right))),
    }
}

fn score_report(name: &str, rows: &[&Scored<'_>]) -> QuestionReport {
    let points: Vec<(f64, i64)> = rows
        .iter()
        .map(|row| {
            let (level, confidence) = match row.answer(name) {
                Some(Answer::Score(a)) => (i64::from(a.rounded_level()), a.confidence),
                _ => (0, 0.0),
            };
            let expected = match row.expects(name) {
                Some(Expectation::Score { level }) => *level as i64,
                _ => 0,
            };
            (confidence, (level - expected).abs())
        })
        .collect();
    QuestionReport::Score {
        name: name.to_owned(),
        cases: points.len(),
        exact: mean(points.iter().map(|(_, off)| f64::from(*off == 0))),
        within_one: mean(points.iter().map(|(_, off)| f64::from(*off <= 1))),
        mae: mean(points.iter().map(|(_, off)| *off as f64)),
        gate: gate(points.iter().map(|(c, off)| (*c, *off == 0))),
    }
}

/// Coverage and accuracy at each cut: what you buy by only acting on confident answers.
fn gate(points: impl Iterator<Item = (f64, bool)>) -> Vec<GateRow> {
    let points: Vec<(f64, bool)> = points.collect();
    CUTS.iter()
        .map(|confidence| {
            let kept: Vec<bool> = points
                .iter()
                .filter(|(c, _)| c >= confidence)
                .map(|(_, right)| *right)
                .collect();
            GateRow {
                confidence: *confidence,
                coverage: if points.is_empty() {
                    0.0
                } else {
                    kept.len() as f64 / points.len() as f64
                },
                accuracy: (!kept.is_empty())
                    .then(|| mean(kept.iter().map(|right| f64::from(*right)))),
            }
        })
        .collect()
}

/// Counted tokens when every answered case carried them; the estimate, marked as one, otherwise.
fn usage_of(
    session: &Session,
    cases: &[Case],
    scored: &[Scored<'_>],
    model: &str,
    rates: Option<Rates>,
) -> ReportUsage {
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut counted = !scored.is_empty();
    for one in scored {
        match one
            .usage
            .as_ref()
            .map(|u| (u.input_tokens, u.output_tokens))
        {
            Some((Some(input), Some(output))) => {
                input_tokens += input;
                output_tokens += output;
            }
            _ => {
                counted = false;
                break;
            }
        }
    }
    if !counted {
        let estimate = preflight(session, cases, model, None);
        input_tokens = estimate.input_tokens as u64;
        output_tokens = estimate.output_tokens as u64;
    }
    ReportUsage {
        input_tokens,
        output_tokens,
        estimated: !counted,
        cost: rates.map(|rates| cost::price(input_tokens, output_tokens, rates)),
    }
}

/// What a whole run would send, before any of it is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Preflight {
    pub cases: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub cost: Option<Cost>,
}

/// The preflight estimate: tokens summed over every case, priced when rates are known.
pub fn preflight(
    session: &Session,
    cases: &[Case],
    model: &str,
    rates: Option<Rates>,
) -> Preflight {
    let mut input_tokens = 0usize;
    let mut output_tokens = 0usize;
    for one in cases {
        let estimate = cost::estimate(&with_state(session, one.state.clone()), model);
        input_tokens += estimate.input_tokens;
        output_tokens += estimate.output_tokens;
    }
    Preflight {
        cases: cases.len(),
        input_tokens,
        output_tokens,
        cost: rates.map(|rates| cost::price(input_tokens as u64, output_tokens as u64, rates)),
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0;
    let mut count = 0usize;
    for value in values {
        sum += value;
        count += 1;
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Two decimals, rounding a tie away from zero the way the TypeScript port's `toFixed(2)` does.
///
/// Rust rounds a tie to even, so `0.125` would print `0.12` here and `0.13` there; every rate and
/// probability in a report goes through this so the two ports' reports can be compared byte for
/// byte.
pub fn two(x: f64) -> String {
    format!("{:.2}", (x * 100.0).round() / 100.0)
}

/// The text report, as lines the terminal draws.
///
/// One block per question, in the page's order: what it scored, the sweep or the gate that says
/// where to set the dial, and — for a choice — the matrix that says what it confuses with what.
pub fn report_lines(report: &Report) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let width = report
        .questions
        .iter()
        .map(|q| q.name().chars().count())
        .max()
        .unwrap_or(0);
    for question in &report.questions {
        if !out.is_empty() {
            out.push(Line::default());
        }
        out.push(header_line(question, width));
        match question {
            QuestionReport::Noul { sweep, best, .. } => {
                out.extend(sweep_lines(sweep, *best, report.threshold));
            }
            QuestionReport::Choice {
                gate,
                labels,
                confusion,
                ..
            } => {
                out.extend(gate_lines(gate, "accuracy"));
                out.extend(confusion_lines(labels, confusion));
            }
            QuestionReport::Score { gate, .. } => out.extend(gate_lines(gate, "exact")),
        }
    }

    if !report.errors.is_empty() {
        if !out.is_empty() {
            out.push(Line::default());
        }
        for failed in &report.errors {
            out.extend(error_case_lines(failed));
        }
    }

    if !out.is_empty() {
        out.push(Line::default());
    }
    let errors = report.errors.len();
    out.push(Line::from(vec![
        Span::raw("  "),
        bold(format!("{} case{}", report.cases, plural(report.cases))),
        dim(format!(
            " · {} answered · {errors} error{}",
            report.answered,
            plural(errors)
        )),
    ]));
    out.push(usage_line(&report.usage));
    out
}

fn header_line(question: &QuestionReport, width: usize) -> Line<'static> {
    let count = format!("{} case{}", question.cases(), plural(question.cases()));
    let summary = match question {
        QuestionReport::Noul { brier, .. } => format!("{count} · Brier {}", two(*brier)),
        QuestionReport::Choice { accuracy, .. } => format!("{count} · accuracy {}", two(*accuracy)),
        QuestionReport::Score {
            exact,
            within_one,
            mae,
            ..
        } => format!(
            "{count} · exact {} · within one {} · mae {}",
            two(*exact),
            two(*within_one),
            two(*mae)
        ),
    };
    Line::from(vec![
        Span::raw("  "),
        bold(pad_end(question.name(), width)),
        Span::raw("  "),
        Span::styled(
            pad_end(question.kind(), 8),
            Style::new().fg(color_for(question.kind())),
        ),
        dim(summary),
    ])
}

/// The sweep: what the threshold buys, row by row, with a `*` on the one this run used.
fn sweep_lines(sweep: &[SweepRow], best: Best, threshold: f64) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::raw("    "),
        dim(pad_end("threshold", 12)),
        dim(pad_end("acc", 6)),
        dim(pad_end("prec", 7)),
        dim(pad_end("rec", 7)),
        dim("f1"),
    ])];
    for row in sweep {
        let chosen = row.threshold == threshold;
        let at = pad_end(
            &format!("{}{}", two(row.threshold), if chosen { " *" } else { "" }),
            12,
        );
        out.push(Line::from(vec![
            Span::raw("    "),
            if chosen { bold(at) } else { Span::raw(at) },
            Span::raw(pad_end(&two(row.accuracy), 6)),
            Span::raw(pad_end(&rate(row.precision), 7)),
            Span::raw(pad_end(&rate(row.recall), 7)),
            Span::raw(two(row.f1)),
        ]));
    }
    out.push(Line::from(vec![
        Span::raw("    "),
        dim(format!("best f1 at {}", two(best.threshold))),
    ]));
    out
}

fn gate_lines(gate: &[GateRow], accuracy: &str) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::raw("    "),
        dim(pad_end("confidence ≥", 15)),
        dim(pad_end("coverage", 10)),
        dim(accuracy.to_owned()),
    ])];
    for row in gate {
        out.push(Line::from(vec![
            Span::raw("    "),
            Span::raw(pad_end(&two(row.confidence), 15)),
            Span::raw(pad_end(&two(row.coverage), 10)),
            Span::raw(rate(row.accuracy)),
        ]));
    }
    out
}

/// The matrix, which is where a rubric's real confusions show: what it calls what.
fn confusion_lines(labels: &[String], confusion: &[Vec<usize>]) -> Vec<Line<'static>> {
    let counts: Vec<usize> = confusion
        .iter()
        .flatten()
        .map(|n| n.to_string().len())
        .collect();
    let column = |label: &str| -> usize {
        counts
            .iter()
            .copied()
            .chain([label.chars().count(), 1])
            .max()
            .unwrap_or(1)
            + 2
    };
    let row_width = confusion
        .iter()
        .enumerate()
        .map(|(at, _)| labels[at].chars().count())
        .max()
        .unwrap_or(0)
        + 3;

    let heading: String = labels
        .iter()
        .map(|label| pad_end(label, column(label)))
        .collect();
    let mut out = vec![
        Line::from(vec![
            Span::raw("    "),
            dim("confusion, rows expected, columns predicted"),
        ]),
        Line::from(vec![
            Span::raw(format!("    {}", " ".repeat(row_width))),
            dim(heading.trim_end().to_owned()),
        ]),
    ];
    for (at, row) in confusion.iter().enumerate() {
        let cells: String = row
            .iter()
            .enumerate()
            .map(|(column2, count)| pad_end(&count.to_string(), column(&labels[column2])))
            .collect();
        out.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(pad_end(&labels[at], row_width), Style::new().fg(CHOICE)),
            Span::raw(cells.trim_end().to_owned()),
        ]));
    }
    out
}

fn error_case_lines(failed: &CaseError) -> Vec<Line<'static>> {
    let name = match &failed.id {
        Some(id) => format!("case {} ({id})", failed.case),
        None => format!("case {}", failed.case),
    };
    let mut parts = failed.message.split('\n');
    let first = parts.next().unwrap_or("").trim().to_owned();
    let mut out = vec![Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{name}: "), Style::new().fg(BAD)),
        Span::raw(first),
    ])];
    for more in parts {
        out.push(Line::from(vec![
            Span::raw("    "),
            dim(more.trim().to_owned()),
        ]));
    }
    out
}

fn usage_line(usage: &ReportUsage) -> Line<'static> {
    let money = match usage.cost {
        Some(cost) => format!(" · {}", cost::usd(cost.total)),
        None => String::new(),
    };
    let tokens = format!(
        "{} in / {} out tokens{money}",
        usage.input_tokens, usage.output_tokens
    );
    if usage.estimated {
        Line::from(vec![
            Span::raw("  "),
            dim(format!("≈ {tokens} — estimated, nothing was counted")),
        ])
    } else {
        Line::from(vec![Span::raw("  "), dim(tokens)])
    }
}

/// The JSON report, ready for `to_string_pretty`. Numbers keep their precision; what is undefined
/// is null.
pub fn report_json(report: &Report) -> Value {
    let mut questions = serde_json::Map::new();
    for question in &report.questions {
        questions.insert(question.name().to_owned(), question_json(question));
    }
    let mut usage = serde_json::Map::new();
    usage.insert("inputTokens".to_owned(), json!(report.usage.input_tokens));
    usage.insert("outputTokens".to_owned(), json!(report.usage.output_tokens));
    usage.insert("estimated".to_owned(), json!(report.usage.estimated));
    if let Some(cost) = report.usage.cost {
        usage.insert("cost".to_owned(), number(cost.total));
    }
    let errors: Vec<Value> = report
        .errors
        .iter()
        .map(|failed| {
            let mut out = serde_json::Map::new();
            out.insert("case".to_owned(), json!(failed.case));
            if let Some(id) = &failed.id {
                out.insert("id".to_owned(), json!(id));
            }
            out.insert("message".to_owned(), json!(failed.message));
            Value::Object(out)
        })
        .collect();
    json!({
        "model": report.model,
        "threshold": number(report.threshold),
        "cases": report.cases,
        "answered": report.answered,
        "errors": errors,
        "questions": Value::Object(questions),
        "usage": Value::Object(usage),
    })
}

fn question_json(question: &QuestionReport) -> Value {
    match question {
        QuestionReport::Noul {
            cases,
            brier,
            accuracy,
            best,
            sweep,
            ..
        } => json!({
            "kind": question.kind(),
            "cases": cases,
            "brier": number(*brier),
            "accuracy": number(*accuracy),
            "best": {"threshold": number(best.threshold), "f1": number(best.f1)},
            "sweep": sweep.iter().map(|row| json!({
                "threshold": number(row.threshold),
                "tp": row.tp,
                "fp": row.fp,
                "fn": row.r#fn,
                "tn": row.tn,
                "accuracy": number(row.accuracy),
                "precision": maybe(row.precision),
                "recall": maybe(row.recall),
                "f1": number(row.f1),
            })).collect::<Vec<_>>(),
        }),
        QuestionReport::Choice {
            cases,
            accuracy,
            labels,
            confusion,
            gate,
            ..
        } => json!({
            "kind": question.kind(),
            "cases": cases,
            "accuracy": number(*accuracy),
            "labels": labels,
            "confusion": confusion,
            "gate": gate.iter().map(gate_json).collect::<Vec<_>>(),
        }),
        QuestionReport::Score {
            cases,
            exact,
            within_one,
            mae,
            gate,
            ..
        } => json!({
            "kind": question.kind(),
            "cases": cases,
            "exact": number(*exact),
            "withinOne": number(*within_one),
            "mae": number(*mae),
            "gate": gate.iter().map(gate_json).collect::<Vec<_>>(),
        }),
    }
}

fn gate_json(row: &GateRow) -> Value {
    json!({
        "confidence": number(row.confidence),
        "coverage": number(row.coverage),
        "accuracy": maybe(row.accuracy),
    })
}

/// A number written the way `JSON.stringify` writes it: a whole float loses its `.0`, so the two
/// ports' JSON reports can be compared byte for byte the way their tables can.
fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9e15 {
        return json!(x as i64);
    }
    json!(x)
}

/// The same, for a rate that was never defined: `null`, so the key is always there.
fn maybe(x: Option<f64>) -> Value {
    x.map_or(Value::Null, number)
}

/// A rate that was never defined is a dot, not a zero: nothing was measured.
fn rate(n: Option<f64>) -> String {
    match n {
        Some(n) => two(n),
        None => "·".to_owned(),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn pad_end(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - length))
    }
}

/// Styled lines as the plain text a pipe wants.
pub fn report_text(report: &Report) -> String {
    let mut out = String::new();
    for line in report_lines(report) {
        for span in &line.spans {
            out.push_str(span.content.as_ref());
        }
        out.push('\n');
    }
    out
}
