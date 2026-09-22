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

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use typesafe::{Answer, Choice, Question, Usage};

use crate::cost::{self, Cost, Rates};
use crate::format::{BAD, CHOICE, DIM, SCORE, bold, color_for, dim, text_of};
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
    read_cases(text, |one| {
        let mut expect = Vec::with_capacity(one.wanted.len());
        for (name, value) in one.wanted {
            let question = question_of(session, name)
                .ok_or_else(|| format!("no question named {name:?} on the page."))?;
            expect.push((name.clone(), expected(name, question, value)?));
        }
        cases.push(case_of(&one, expect));
        Ok(())
    })?;
    Ok(cases)
}

/// What the two pages of a comparison are called in its messages and its report.
#[derive(Debug, Clone, Copy)]
pub struct Labels<'a> {
    pub a: &'a str,
    pub b: &'a str,
}

/// Parse one cases file for two pages at once: a case per page, each holding the expectations for
/// that page's questions.
///
/// A label may name a question on either page, which is what lets a page that adds a question be
/// compared with one that does not. It is still checked against every page that has the question —
/// a case one page cannot even express is not a paired observation, it is a typo.
pub fn parse_compare_cases(
    text: &str,
    a: &Session,
    b: &Session,
    labels: Labels<'_>,
) -> Result<(Vec<Case>, Vec<Case>), String> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    read_cases(text, |one| {
        let mut expect_a = Vec::new();
        let mut expect_b = Vec::new();
        for (name, value) in one.wanted {
            let on_a = question_of(a, name);
            let on_b = question_of(b, name);
            if on_a.is_none() && on_b.is_none() {
                return Err(format!("no question named {name:?} on either page."));
            }
            for (question, into, label) in [
                (on_a, &mut expect_a, labels.a),
                (on_b, &mut expect_b, labels.b),
            ] {
                let Some(question) = question else { continue };
                let expectation =
                    expected(name, question, value).map_err(|e| format!("{label}: {e}"))?;
                into.push((name.clone(), expectation));
            }
        }
        if !expect_a.is_empty() {
            left.push(case_of(&one, expect_a));
        }
        if !expect_b.is_empty() {
            right.push(case_of(&one, expect_b));
        }
        Ok(())
    })?;
    Ok((left, right))
}

/// A line of the cases file that is a case in shape, before its labels meet a page.
struct RawCase<'a> {
    line: usize,
    id: Option<String>,
    state: &'a Value,
    wanted: &'a serde_json::Map<String, Value>,
}

/// Hand every non-blank line to `visit` as a case, stopping at the first line that is not one or
/// that `visit` refuses; the message that comes back already names the line.
fn read_cases(
    text: &str,
    mut visit: impl FnMut(RawCase<'_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut seen = 0usize;
    for (i, raw) in text.split('\n').enumerate() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let line = i + 1;
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| format!("cases line {line}: not valid JSON: {e}"))?;
        let one = read_case(&value, line).map_err(|e| format!("cases line {line}: {e}"))?;
        visit(one).map_err(|e| format!("cases line {line}: {e}"))?;
        seen += 1;
    }
    if seen == 0 {
        return Err("the cases file holds no cases.".to_owned());
    }
    Ok(())
}

fn read_case(value: &Value, line: usize) -> Result<RawCase<'_>, String> {
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
    Ok(RawCase {
        line,
        id,
        state,
        wanted,
    })
}

fn case_of(one: &RawCase<'_>, expect: Vec<(String, Expectation)>) -> Case {
    Case {
        line: one.line,
        id: one.id.clone(),
        state: one.state.clone(),
        expect,
    }
}

fn question_of<'a>(session: &'a Session, name: &str) -> Option<&'a Question> {
    session
        .questions
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, q)| q)
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
/// A slow case holds up nothing but itself, and each result is written to its own slot: a file of
/// a thousand labels keeps its order however the calls come back.
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
    spawn_leg(&mut workers, &permits, 0, session, cases, ask).await;
    let [outcomes] = collect(workers, [cases.len()]).await;
    outcomes
}

/// One page's share of a comparison: its session, its cases, and how to ask it.
pub struct Leg<'a, F> {
    pub session: &'a Session,
    pub cases: &'a [Case],
    pub ask: F,
}

/// Both pages of a comparison through one pool of workers: page `a`'s cases first, then `b`'s.
///
/// One pool rather than one per page, so `--concurrency` still means what it says — that many
/// requests in the air, whichever page they are for.
pub async fn run_compare<FA, FutA, FB, FutB>(
    a: Leg<'_, FA>,
    b: Leg<'_, FB>,
    concurrency: usize,
) -> (Vec<Outcome>, Vec<Outcome>)
where
    FA: Fn(Session) -> FutA + Send + Sync + Clone + 'static,
    FutA: Future<Output = Outcome> + Send + 'static,
    FB: Fn(Session) -> FutB + Send + Sync + Clone + 'static,
    FutB: Future<Output = Outcome> + Send + 'static,
{
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut workers = JoinSet::new();
    let sizes = [a.cases.len(), b.cases.len()];
    spawn_leg(&mut workers, &permits, 0, a.session, a.cases, a.ask).await;
    spawn_leg(&mut workers, &permits, 1, b.session, b.cases, b.ask).await;
    let [left, right] = collect(workers, sizes).await;
    (left, right)
}

/// Queue one leg's cases on the pool, in order: a case is spawned once a permit is free, so the
/// cases go out in file order and never more than the pool allows are in the air.
async fn spawn_leg<F, Fut>(
    workers: &mut JoinSet<(usize, usize, Outcome)>,
    permits: &Arc<Semaphore>,
    leg: usize,
    session: &Session,
    cases: &[Case],
    ask: F,
) where
    F: Fn(Session) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    for (at, one) in cases.iter().enumerate() {
        let session = with_state(session, one.state.clone());
        let ask = ask.clone();
        let permit = Arc::clone(permits).acquire_owned().await;
        workers.spawn(async move {
            let _permit = permit;
            (leg, at, ask(session).await)
        });
    }
}

/// Wait for every worker and put each outcome in its leg's slot for its case.
async fn collect<const N: usize>(
    mut workers: JoinSet<(usize, usize, Outcome)>,
    sizes: [usize; N],
) -> [Vec<Outcome>; N] {
    let mut slots: [Vec<Option<Outcome>>; N] = sizes.map(|n| vec![None; n]);
    while let Some(joined) = workers.join_next().await {
        // A worker that panicked leaves its slot empty; the run is not lost to one case.
        if let Ok((leg, at, outcome)) = joined {
            slots[leg][at] = Some(outcome);
        }
    }
    slots.map(|outcomes| {
        outcomes
            .into_iter()
            .map(|outcome| {
                outcome.unwrap_or_else(|| Outcome::Failed {
                    error: "nothing was sent for this case.".to_owned(),
                })
            })
            .collect()
    })
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
    line: usize,
    id: Option<&'a str>,
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
    let (errors, scored) = scored_of(cases, outcomes);
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

/// Split the outcomes into the cases that can be scored and the ones that are errors.
fn scored_of<'a>(cases: &'a [Case], outcomes: &[Outcome]) -> (Vec<CaseError>, Vec<Scored<'a>>) {
    let mut errors: Vec<CaseError> = Vec::new();
    let mut scored: Vec<Scored<'a>> = Vec::new();
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
            line: one.line,
            id: one.id.as_deref(),
            expect: &one.expect,
            answers,
            usage: usage.clone(),
        });
    }
    (errors, scored)
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

/// Two decimals, the way the TypeScript port's `toFixed(2)` writes them.
///
/// Rust rounds an exact tie to even, so `0.125` would print `0.12` here and `0.13` there; every
/// rate and probability in a report goes through this so the two ports' reports can be compared
/// byte for byte.
pub fn two(x: f64) -> String {
    to_fixed(x, 2)
}

/// Three decimals, the way `toFixed(3)` writes them; see [`two`].
pub fn three(x: f64) -> String {
    to_fixed(x, 3)
}

/// `Number.prototype.toFixed`: the double's exact decimal value, rounded to `digits` places with an
/// exact tie going away from zero.
///
/// Scaling by a power of ten and rounding is not the same thing — `0.475 * 100` lands on `47.5`
/// although `0.475` is a hair below it, so it would print `0.48` where `toFixed` prints `0.47`.
/// Rust's `{:.N}` is already exact and differs only on a true tie, which is the one case handled
/// by hand.
fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() {
        return format!("{x}");
    }
    // Wide enough to hold every digit of any double's exact expansion.
    let exact = format!("{:.1100}", x.abs());
    let point = exact.find('.').unwrap_or(exact.len());
    let tail = exact.get(point + 1 + digits..).unwrap_or("");
    let tie = tail.starts_with('5') && tail[1..].bytes().all(|b| b == b'0');
    let magnitude = if tie {
        let mut kept: Vec<u8> = exact[..point + 1 + digits].bytes().collect();
        let mut at = kept.len();
        loop {
            if at == 0 {
                kept.insert(0, b'1');
                break;
            }
            at -= 1;
            match kept[at] {
                b'.' => continue,
                b'9' => kept[at] = b'0',
                digit => {
                    kept[at] = digit + 1;
                    break;
                }
            }
        }
        let mut text = String::from_utf8(kept).unwrap_or_default();
        if digits == 0 {
            text.pop();
        }
        text
    } else {
        format!("{:.digits$}", x.abs())
    };
    if x < 0.0 {
        format!("-{magnitude}")
    } else {
        magnitude
    }
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
            out.extend(error_case_lines(failed, ""));
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

fn error_case_lines(failed: &CaseError, prefix: &str) -> Vec<Line<'static>> {
    let name = format!("{prefix}{}", case_name(failed.case, failed.id.as_deref()));
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
    lines_text(report_lines(report))
}

fn lines_text(lines: Vec<Line<'static>>) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            out.push_str(span.content.as_ref());
        }
        out.push('\n');
    }
    out
}

// ---- two pages over the same cases ------------------------------------------------------------

/// One page's run, as a comparison needs it.
#[derive(Debug, Clone, Copy)]
pub struct Side<'a> {
    /// What the page is called: the path it was read from.
    pub label: &'a str,
    pub session: &'a Session,
    pub cases: &'a [Case],
    pub outcomes: &'a [Outcome],
    pub model: &'a str,
}

/// What the exact McNemar test made of the discordant pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    TooFew,
    Better,
    Worse,
    Same,
}

impl Verdict {
    /// The word the report and the JSON use.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::TooFew => "too few",
            Verdict::Better => "better",
            Verdict::Worse => "worse",
            Verdict::Same => "same",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McNemar {
    /// Cases one page got right and the other wrong: `fixed + broke`.
    pub discordant: usize,
    /// Two-sided exact p-value; 1 when there is nothing to test.
    pub p: f64,
    pub verdict: Verdict,
}

/// A case the two pages answered differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Flip {
    pub case: usize,
    pub id: Option<String>,
    /// What the case expects, and what each page predicted, in the question's own terms.
    pub expected: Value,
    pub a: Value,
    pub b: Value,
    /// `fixed`: `b` put right what `a` got wrong. `broke`: the reverse. `changed`: both wrong.
    pub status: &'static str,
}

/// One metric on both sides, and what moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    /// The JSON key.
    pub key: &'static str,
    /// The row name in the text report.
    pub label: &'static str,
    pub a: f64,
    pub b: f64,
    /// Whether a delta means anything: a threshold is a setting, not a result.
    pub delta: bool,
}

/// A question both pages ask the same way, measured on the cases both pages scored.
#[derive(Debug, Clone, PartialEq)]
pub struct Shared {
    pub name: String,
    pub kind: &'static str,
    pub paired: usize,
    pub metrics: Vec<Metric>,
    pub fixed: usize,
    pub broke: usize,
    pub changed: usize,
    pub mcnemar: McNemar,
    pub flips: Vec<Flip>,
}

/// A name both pages use for questions of different kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub name: String,
    pub a: &'static str,
    pub b: &'static str,
}

/// One page's report, with the name it goes by.
#[derive(Debug, Clone, PartialEq)]
pub struct Labelled {
    pub label: String,
    pub report: Report,
}

/// Everything a comparison found, with the numbers unrounded.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub a: Labelled,
    pub b: Labelled,
    pub questions: Vec<Shared>,
    pub only_a: Vec<String>,
    pub only_b: Vec<String>,
    pub mismatched: Vec<Mismatch>,
    pub unpaired: Vec<String>,
    /// Distinct cases across both pages.
    pub cases: usize,
    pub usage: ReportUsage,
}

/// What a comparison is read at, which both reports repeat back.
#[derive(Debug, Clone, Copy)]
pub struct CompareOptions {
    pub threshold: f64,
    pub rates: Option<Rates>,
}

/// The level McNemar's test is read at. Not a flag: a comparison should mean the same everywhere.
pub const ALPHA: f64 = 0.05;

/// Below this many discordant pairs no two-sided exact p can reach [`ALPHA`]: 2 / 2^5 > 0.05.
pub const MIN_DISCORDANT: usize = 6;

/// The exact McNemar test on the discordant pairs of a paired comparison.
///
/// Under "no difference" each discordant pair is a fair coin, so the p-value is a binomial tail.
/// It is summed in log space because `2^n` stops being a number long before a cases file stops
/// being a reasonable size.
pub fn mcnemar(fixed: usize, broke: usize) -> McNemar {
    let n = fixed + broke;
    let mut p = 1.0;
    if n > 0 {
        let low = fixed.min(broke);
        let ln2n = n as f64 * std::f64::consts::LN_2;
        let mut ln_choose = 0.0;
        let mut tail = (-ln2n).exp();
        for k in 1..=low {
            ln_choose += ((n - k + 1) as f64).ln() - (k as f64).ln();
            tail += (ln_choose - ln2n).exp();
        }
        p = (2.0 * tail).min(1.0);
    }
    let verdict = if n < MIN_DISCORDANT {
        Verdict::TooFew
    } else if p < ALPHA && fixed > broke {
        Verdict::Better
    } else if p < ALPHA && broke > fixed {
        Verdict::Worse
    } else {
        Verdict::Same
    };
    McNemar {
        discordant: n,
        p,
        verdict,
    }
}

/// The wire `type` of a question, with anything that is not a noul, a choice or a score as `raw`.
fn kind_of(question: &Question) -> &'static str {
    match question {
        Question::Noul(_) => "noul",
        Question::Choice(_) => "choice",
        Question::Score(_) => "score",
        _ => "raw",
    }
}

/// Put two runs of the same cases side by side.
///
/// Only the cases both pages scored count, so each delta is measured on the same states: a page
/// that errored on the hard cases must not look better for it. The full reports, over everything
/// each page scored, travel along for the JSON.
pub fn compare(a: Side<'_>, b: Side<'_>, options: CompareOptions) -> Comparison {
    let report_of = |side: &Side<'_>| {
        report(
            side.session,
            side.cases,
            side.outcomes,
            ReportOptions {
                model: side.model,
                threshold: options.threshold,
                rates: options.rates,
            },
        )
    };
    let (_, left) = scored_of(a.cases, a.outcomes);
    let (_, right) = scored_of(b.cases, b.outcomes);
    let right: HashMap<(usize, Option<usize>), &Scored<'_>> =
        right.iter().map(|one| (key_of(one), one)).collect();
    let twin_of = |one: &Scored<'_>| right.get(&key_of(one)).copied();

    let kind_in_b = |name: &str| question_of(b.session, name).map(kind_of);
    let mut questions = Vec::new();
    let mut mismatched = Vec::new();
    let mut unpaired = Vec::new();
    for (name, question) in &a.session.questions {
        let Some(other) = kind_in_b(name) else {
            continue;
        };
        let kind = kind_of(question);
        if other != kind || kind == "raw" {
            if other != kind {
                mismatched.push(Mismatch {
                    name: name.clone(),
                    a: kind,
                    b: other,
                });
            }
            continue;
        }
        let pairs: Vec<(&Scored<'_>, &Scored<'_>)> = left
            .iter()
            .filter_map(|one| {
                let twin = twin_of(one)?;
                (one.expects(name).is_some() && twin.expects(name).is_some()).then_some((one, twin))
            })
            .collect();
        if pairs.is_empty() {
            // A question nobody labelled is left out, as eval leaves it out; one that was labelled
            // and still has no pair is worth saying so about.
            let labelled = a
                .cases
                .iter()
                .chain(b.cases)
                .any(|one| one.expect.iter().any(|(n, _)| n == name));
            if labelled {
                unpaired.push(name.clone());
            }
            continue;
        }
        questions.push(shared(
            name,
            kind,
            &pairs,
            options.threshold,
            options.threshold,
        ));
    }

    let mut keys: Vec<(usize, Option<usize>)> =
        a.cases.iter().chain(b.cases).map(case_key).collect();
    keys.sort_unstable();
    keys.dedup();
    let report_a = report_of(&a);
    let report_b = report_of(&b);
    let usage = sum_usage(&report_a.usage, &report_b.usage, options.rates);
    Comparison {
        a: Labelled {
            label: a.label.to_owned(),
            report: report_a,
        },
        b: Labelled {
            label: b.label.to_owned(),
            report: report_b,
        },
        questions,
        only_a: a
            .session
            .questions
            .iter()
            .filter(|(name, _)| question_of(b.session, name).is_none())
            .map(|(name, _)| name.clone())
            .collect(),
        only_b: b
            .session
            .questions
            .iter()
            .filter(|(name, _)| question_of(a.session, name).is_none())
            .map(|(name, _)| name.clone())
            .collect(),
        mismatched,
        unpaired,
        cases: keys.len(),
        usage,
    }
}

/// Which case a scored row came from, so the same case can be found on the other page.
fn key_of(one: &Scored<'_>) -> (usize, Option<usize>) {
    (one.line, None)
}

/// The same key, for a case that has not been scored.
fn case_key(one: &Case) -> (usize, Option<usize>) {
    (one.line, None)
}

fn sum_usage(a: &ReportUsage, b: &ReportUsage, rates: Option<Rates>) -> ReportUsage {
    let input_tokens = a.input_tokens + b.input_tokens;
    let output_tokens = a.output_tokens + b.output_tokens;
    ReportUsage {
        input_tokens,
        output_tokens,
        estimated: a.estimated || b.estimated,
        cost: rates.map(|rates| cost::price(input_tokens, output_tokens, rates)),
    }
}

/// One paired observation: what each side predicted, and whether it was right.
struct Pair<'a> {
    one: &'a Scored<'a>,
    expected: Value,
    a: Value,
    b: Value,
    right_a: bool,
    right_b: bool,
}

fn shared(
    name: &str,
    kind: &'static str,
    pairs: &[(&Scored<'_>, &Scored<'_>)],
    threshold_a: f64,
    threshold_b: f64,
) -> Shared {
    let noul_of = |row: &Scored<'_>| match row.answer(name) {
        Some(Answer::Noul(answer)) => answer.noul,
        _ => 0.0,
    };
    let yes_of =
        |row: &Scored<'_>| matches!(row.expects(name), Some(Expectation::Noul { yes: true }));
    let choice_of = |row: &Scored<'_>| match row.answer(name) {
        Some(Answer::Choice(answer)) => answer.choice.clone(),
        _ => String::new(),
    };
    let level_of = |row: &Scored<'_>| match row.answer(name) {
        Some(Answer::Score(answer)) => answer.rounded_level() as usize,
        _ => 0,
    };
    let (metrics, observed): (Vec<Metric>, Vec<Pair<'_>>) = match kind {
        "noul" => {
            let left: Vec<(f64, bool)> = pairs
                .iter()
                .map(|(one, _)| (noul_of(one), yes_of(one)))
                .collect();
            let right: Vec<(f64, bool)> = pairs
                .iter()
                .map(|(_, twin)| (noul_of(twin), yes_of(twin)))
                .collect();
            let brier = |list: &[(f64, bool)]| {
                mean(
                    list.iter()
                        .map(|(p, yes)| (p - if *yes { 1.0 } else { 0.0 }).powi(2)),
                )
            };
            let row_a = sweep_row(&left, threshold_a);
            let row_b = sweep_row(&right, threshold_b);
            let metrics = vec![
                metric("threshold", "threshold", threshold_a, threshold_b, false),
                metric("brier", "brier", brier(&left), brier(&right), true),
                metric("accuracy", "accuracy", row_a.accuracy, row_b.accuracy, true),
                metric("f1", "f1", row_a.f1, row_b.f1, true),
            ];
            let observed = pairs
                .iter()
                .zip(left.iter().zip(&right))
                .map(|((one, _), ((pa, yes), (pb, _)))| {
                    let (pred_a, pred_b) = (*pa >= threshold_a, *pb >= threshold_b);
                    Pair {
                        one,
                        expected: Value::Bool(*yes),
                        a: Value::Bool(pred_a),
                        b: Value::Bool(pred_b),
                        right_a: pred_a == *yes,
                        right_b: pred_b == *yes,
                    }
                })
                .collect();
            (metrics, observed)
        }
        "choice" => {
            let observed: Vec<Pair<'_>> = pairs
                .iter()
                .map(|(one, twin)| {
                    let expected = match one.expects(name) {
                        Some(Expectation::Choice { label }) => label.clone(),
                        _ => String::new(),
                    };
                    let (a, b) = (choice_of(one), choice_of(twin));
                    Pair {
                        one,
                        right_a: a == expected,
                        right_b: b == expected,
                        expected: Value::String(expected),
                        a: Value::String(a),
                        b: Value::String(b),
                    }
                })
                .collect();
            let metrics = vec![metric(
                "accuracy",
                "accuracy",
                mean(observed.iter().map(|pair| f64::from(pair.right_a))),
                mean(observed.iter().map(|pair| f64::from(pair.right_b))),
                true,
            )];
            (metrics, observed)
        }
        _ => {
            let mut offs: Vec<(usize, usize)> = Vec::new();
            let observed: Vec<Pair<'_>> = pairs
                .iter()
                .map(|(one, twin)| {
                    let expected = match one.expects(name) {
                        Some(Expectation::Score { level }) => *level,
                        _ => 0,
                    };
                    let (a, b) = (level_of(one), level_of(twin));
                    offs.push((a.abs_diff(expected), b.abs_diff(expected)));
                    Pair {
                        one,
                        expected: json!(expected),
                        a: json!(a),
                        b: json!(b),
                        right_a: a == expected,
                        right_b: b == expected,
                    }
                })
                .collect();
            let both = |f: &dyn Fn(usize) -> f64| {
                (
                    mean(offs.iter().map(|(a, _)| f(*a))),
                    mean(offs.iter().map(|(_, b)| f(*b))),
                )
            };
            let (exact_a, exact_b) = both(&|off| f64::from(off == 0));
            let (within_a, within_b) = both(&|off| f64::from(off <= 1));
            let (mae_a, mae_b) = both(&|off| off as f64);
            let metrics = vec![
                metric("exact", "exact", exact_a, exact_b, true),
                metric("withinOne", "within one", within_a, within_b, true),
                metric("mae", "mae", mae_a, mae_b, true),
            ];
            (metrics, observed)
        }
    };

    let mut flips = Vec::new();
    let (mut fixed, mut broke, mut changed) = (0, 0, 0);
    for pair in observed {
        if pair.a == pair.b {
            continue;
        }
        let status = if !pair.right_a && pair.right_b {
            fixed += 1;
            "fixed"
        } else if pair.right_a {
            broke += 1;
            "broke"
        } else {
            changed += 1;
            "changed"
        };
        flips.push(Flip {
            case: pair.one.line,
            id: pair.one.id.map(str::to_owned),
            expected: pair.expected,
            a: pair.a,
            b: pair.b,
            status,
        });
    }
    Shared {
        name: name.to_owned(),
        kind,
        paired: pairs.len(),
        metrics,
        fixed,
        broke,
        changed,
        mcnemar: mcnemar(fixed, broke),
        flips,
    }
}

fn metric(key: &'static str, label: &'static str, a: f64, b: f64, delta: bool) -> Metric {
    Metric {
        key,
        label,
        a,
        b,
        delta,
    }
}

/// The questions `b` is significantly worse at, for `--fail-on-regression`.
pub fn regressions(comparison: &Comparison) -> Vec<&Shared> {
    comparison
        .questions
        .iter()
        .filter(|q| q.mcnemar.verdict == Verdict::Worse)
        .collect()
}

/// How many flipped cases the text report lists per question before it points at the JSON.
const FLIPS_SHOWN: usize = 10;

/// The comparison as lines: a legend, a block per shared question, what could not be compared.
pub fn compare_lines(comparison: &Comparison) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let sides = [("a", &comparison.a), ("b", &comparison.b)];
    let label_width = sides
        .iter()
        .map(|(_, side)| side.label.chars().count())
        .max()
        .unwrap_or(0);
    for (letter, side) in sides {
        let n = side.report.cases;
        out.push(Line::from(vec![
            Span::raw("  "),
            bold(letter),
            Span::raw("  "),
            Span::raw(pad_end(&side.label, label_width)),
            Span::raw("  "),
            dim(format!("{} · {n} case{}", side.report.model, plural(n))),
        ]));
    }

    let width = comparison
        .questions
        .iter()
        .map(|q| q.name.chars().count())
        .max()
        .unwrap_or(0);
    for question in &comparison.questions {
        out.push(Line::default());
        out.extend(shared_lines(question, width));
    }

    let mut lists: Vec<Line<'static>> = Vec::new();
    if !comparison.only_a.is_empty() {
        lists.push(Line::from(vec![
            Span::raw("  "),
            dim("only in a: "),
            Span::raw(comparison.only_a.join(", ")),
        ]));
    }
    if !comparison.only_b.is_empty() {
        lists.push(Line::from(vec![
            Span::raw("  "),
            dim("only in b: "),
            Span::raw(comparison.only_b.join(", ")),
        ]));
    }
    for odd in &comparison.mismatched {
        lists.push(Line::from(vec![
            Span::raw("  "),
            dim("mismatched: "),
            Span::raw(format!(
                "{} is a {} in a and a {} in b",
                odd.name, odd.a, odd.b
            )),
        ]));
    }
    for name in &comparison.unpaired {
        lists.push(Line::from(vec![
            Span::raw("  "),
            dim("unpaired: "),
            Span::raw(format!("{name} — no case was scored for it on both pages")),
        ]));
    }
    if !lists.is_empty() {
        out.push(Line::default());
        out.extend(lists);
    }

    let mut failures: Vec<Line<'static>> = Vec::new();
    for (letter, side) in sides {
        for failed in &side.report.errors {
            failures.extend(error_case_lines(failed, &format!("{letter} ")));
        }
    }
    if !failures.is_empty() {
        out.push(Line::default());
        out.extend(failures);
    }

    out.push(Line::default());
    let tally = |report: &Report| {
        let errors = report.errors.len();
        format!(
            "{} answered, {errors} error{}",
            report.answered,
            plural(errors)
        )
    };
    out.push(Line::from(vec![
        Span::raw("  "),
        bold(format!(
            "{} case{}",
            comparison.cases,
            plural(comparison.cases)
        )),
        dim(format!(
            " · a {} · b {}",
            tally(&comparison.a.report),
            tally(&comparison.b.report)
        )),
    ]));
    out.push(usage_line(&comparison.usage));
    out
}

fn shared_lines(question: &Shared, width: usize) -> Vec<Line<'static>> {
    let n = question.paired;
    let mut out = vec![
        Line::from(vec![
            Span::raw("  "),
            bold(pad_end(&question.name, width)),
            Span::raw("  "),
            Span::styled(
                pad_end(question.kind, 8),
                Style::new().fg(color_for(question.kind)),
            ),
            dim(format!("{n} paired case{}", plural(n))),
        ]),
        Line::from(vec![
            Span::raw(format!("    {}", " ".repeat(14))),
            dim(format!("{}{}Δ", pad_end("a", 8), pad_end("b", 8))),
        ]),
    ];
    for metric in &question.metrics {
        let mut cells = vec![two(metric.a), two(metric.b)];
        if metric.delta {
            cells.push(signed(metric.b - metric.a));
        }
        let cells: String = cells.iter().map(|cell| pad_end(cell, 8)).collect();
        out.push(Line::from(vec![
            Span::raw("    "),
            Span::raw(pad_end(metric.label, 14)),
            Span::raw(cells.trim_end().to_owned()),
        ]));
    }
    out.push(Line::from(vec![
        Span::raw("    "),
        Span::raw(format!(
            "{} fixed · {} broke · {} changed",
            question.fixed, question.broke, question.changed
        )),
    ]));
    let verdict = question.mcnemar.verdict;
    let text = mcnemar_text(&question.mcnemar);
    out.push(Line::from(vec![
        Span::raw("    "),
        match verdict {
            Verdict::Better => Span::styled(text, Style::new().fg(SCORE)),
            Verdict::Worse => Span::styled(text, Style::new().fg(BAD)),
            _ => dim(text),
        },
    ]));

    let shown = &question.flips[..question.flips.len().min(FLIPS_SHOWN)];
    let names: Vec<String> = shown
        .iter()
        .map(|flip| case_name(flip.case, flip.id.as_deref()))
        .collect();
    let moves: Vec<String> = shown
        .iter()
        .map(|flip| {
            format!(
                "{} → {}",
                reading(question.kind, &flip.a),
                reading(question.kind, &flip.b)
            )
        })
        .collect();
    let name_width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    let move_width = moves.iter().map(|m| m.chars().count()).max().unwrap_or(0);
    for ((flip, name), change) in shown.iter().zip(&names).zip(&moves) {
        let color = match flip.status {
            "fixed" => SCORE,
            "broke" => BAD,
            _ => DIM,
        };
        out.push(Line::from(vec![
            Span::raw("    "),
            Span::raw(pad_end(name, name_width)),
            Span::raw("   "),
            Span::raw(pad_end(change, move_width)),
            Span::raw("   "),
            Span::styled(flip.status, Style::new().fg(color)),
        ]));
    }
    let more = question.flips.len() - shown.len();
    if more > 0 {
        out.push(Line::from(vec![
            Span::raw("    "),
            dim(format!("… {more} more flipped; --json lists them all")),
        ]));
    }
    out
}

/// The significance line, which says in words what the p-value allows and what it does not.
fn mcnemar_text(test: &McNemar) -> String {
    if test.discordant == 0 {
        return "McNemar: no discordant pairs, nothing to test".to_owned();
    }
    if test.verdict == Verdict::TooFew {
        return format!(
            "McNemar: too few discordant pairs to call ({}; {MIN_DISCORDANT} are needed for p < {ALPHA})",
            test.discordant
        );
    }
    let head = format!(
        "McNemar p {} over {} discordant pairs: ",
        three(test.p),
        test.discordant
    );
    match test.verdict {
        Verdict::Better => format!("{head}b is significantly better"),
        Verdict::Worse => format!("{head}b is significantly worse"),
        _ => format!("{head}no significant difference"),
    }
}

/// A prediction as the report says it: yes or no, a label, a level.
fn reading(kind: &str, value: &Value) -> String {
    match (kind, value) {
        ("noul", Value::Bool(true)) => "yes".to_owned(),
        ("noul", _) => "no".to_owned(),
        ("score", other) => format!("level {other}"),
        (_, Value::String(label)) => label.clone(),
        (_, other) => other.to_string(),
    }
}

/// `case 7 (t-007)`: the line in the cases file, and the id when the case has one.
fn case_name(line: usize, id: Option<&str>) -> String {
    match id {
        Some(id) => format!("case {line} ({id})"),
        None => format!("case {line}"),
    }
}

/// A change, signed either way, so a regression reads as one; `-0.00` is no change, so `+0.00`.
pub fn signed(n: f64) -> String {
    let text = two(n);
    if text == "-0.00" {
        return "+0.00".to_owned();
    }
    if text.starts_with('-') {
        text
    } else {
        format!("+{text}")
    }
}

/// The comparison as JSON, ready for `to_string_pretty`: both reports whole, and what moved
/// between them.
pub fn compare_json(comparison: &Comparison) -> Value {
    let side = |one: &Labelled| {
        let mut out = serde_json::Map::new();
        out.insert("page".to_owned(), json!(one.label));
        if let Value::Object(report) = report_json(&one.report) {
            out.extend(report);
        }
        Value::Object(out)
    };
    let mut questions = serde_json::Map::new();
    for question in &comparison.questions {
        let pick = |f: &dyn Fn(&Metric) -> Option<f64>| {
            let mut out = serde_json::Map::new();
            for metric in &question.metrics {
                if let Some(value) = f(metric) {
                    out.insert(metric.key.to_owned(), number(value));
                }
            }
            Value::Object(out)
        };
        let flips: Vec<Value> = question
            .flips
            .iter()
            .map(|flip| {
                let mut out = serde_json::Map::new();
                out.insert("case".to_owned(), json!(flip.case));
                if let Some(id) = &flip.id {
                    out.insert("id".to_owned(), json!(id));
                }
                out.insert("expected".to_owned(), flip.expected.clone());
                out.insert("a".to_owned(), flip.a.clone());
                out.insert("b".to_owned(), flip.b.clone());
                out.insert("status".to_owned(), json!(flip.status));
                Value::Object(out)
            })
            .collect();
        questions.insert(
            question.name.clone(),
            json!({
                "kind": question.kind,
                "paired": question.paired,
                "a": pick(&|m| Some(m.a)),
                "b": pick(&|m| Some(m.b)),
                "delta": pick(&|m| m.delta.then_some(m.b - m.a)),
                "fixed": question.fixed,
                "broke": question.broke,
                "changed": question.changed,
                "mcnemar": {
                    "discordant": question.mcnemar.discordant,
                    "p": number(question.mcnemar.p),
                    "verdict": question.mcnemar.verdict.as_str(),
                },
                "flips": flips,
            }),
        );
    }
    let mut usage = serde_json::Map::new();
    usage.insert(
        "inputTokens".to_owned(),
        json!(comparison.usage.input_tokens),
    );
    usage.insert(
        "outputTokens".to_owned(),
        json!(comparison.usage.output_tokens),
    );
    usage.insert("estimated".to_owned(), json!(comparison.usage.estimated));
    if let Some(cost) = comparison.usage.cost {
        usage.insert("cost".to_owned(), number(cost.total));
    }
    json!({
        "a": side(&comparison.a),
        "b": side(&comparison.b),
        "questions": Value::Object(questions),
        "onlyA": comparison.only_a,
        "onlyB": comparison.only_b,
        "mismatched": comparison.mismatched.iter().map(|odd| json!({
            "name": odd.name,
            "a": odd.a,
            "b": odd.b,
        })).collect::<Vec<_>>(),
        "unpaired": comparison.unpaired,
        "regressions": regressions(comparison).iter().map(|q| q.name.clone()).collect::<Vec<_>>(),
        "usage": Value::Object(usage),
    })
}

/// The comparison as the plain text a pipe wants.
pub fn compare_text(comparison: &Comparison) -> String {
    lines_text(compare_lines(comparison))
}
