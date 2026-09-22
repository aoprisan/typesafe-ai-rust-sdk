//! Scoring a rubric: the cases file, the metrics, the runner and the two reports.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jev_repl::cost::Rates;
use jev_repl::evaluate::{
    self, CaseError, Expectation, GateRow, Outcome, QuestionReport, Report, ReportOptions,
};
use jev_repl::headless;
use jev_repl::session::Session;
use serde_json::json;
use typesafe::Question;

const PAGE: &str = "A payout failed for the third time.
---
is_urgent? The message conveys urgency
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales = Pricing and plans
frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry
";

fn session() -> Session {
    headless::load(PAGE).expect("the page parses")
}

/// The cases, or a panic carrying the message that says why there are none.
fn cases(text: &str, session: &Session) -> Vec<evaluate::Case> {
    evaluate::parse_cases(text, session).expect("these cases parse")
}

fn why(text: &str, session: &Session) -> String {
    evaluate::parse_cases(text, session).expect_err("expected these cases to be rejected")
}

#[test]
fn reads_a_labelled_state_per_line() {
    let parsed = cases(
        concat!(
            r#"{"id": "t-001", "state": "Stripe has been down for 3 days", "expect": {"is_urgent": true}}"#,
            "\n",
            r#"{"state": {"subject": "Invoice"}, "expect": {"department": "billing", "frustration": 0}}"#,
        ),
        &session(),
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].line, 1);
    assert_eq!(parsed[0].id.as_deref(), Some("t-001"));
    assert_eq!(
        parsed[0].expect,
        vec![(
            "is_urgent".to_owned(),
            Expectation::Noul {
                yes: true,
                by_turn: None
            }
        )]
    );
    assert_eq!(parsed[1].state, json!({"subject": "Invoice"}));
    assert_eq!(parsed[1].id, None);
    assert_eq!(parsed[1].expect[1].1, Expectation::Score { level: 0 });
}

#[test]
fn numbers_a_case_by_its_line_not_by_its_place_in_the_file() {
    let parsed = cases(
        "\n{\"state\": \"a\", \"expect\": {\"is_urgent\": true}}\n\n  \n\
         {\"state\": \"b\", \"expect\": {\"is_urgent\": false}}\n",
        &session(),
    );
    assert_eq!(
        parsed.iter().map(|c| c.line).collect::<Vec<_>>(),
        vec![2, 5]
    );
}

#[test]
fn takes_a_score_as_the_text_of_one_of_its_levels() {
    let parsed = cases(
        r#"{"state": "a", "expect": {"frustration": "Very angry"}}"#,
        &session(),
    );
    assert_eq!(parsed[0].expect[0].1, Expectation::Score { level: 2 });
}

#[test]
fn says_which_line_is_wrong_and_what_is_wrong_with_it() {
    let s = session();
    let said = |text: &str| why(text, &s);
    assert!(
        said("not json").starts_with("cases line 1: not valid JSON"),
        "{}",
        said("not json")
    );
    assert_eq!(
        said("[1, 2]"),
        "cases line 1: expected a JSON object with `state` and `expect`."
    );
    for (text, wanted) in [
        (r#"{"expect": {"is_urgent": true}}"#, "missing `state`"),
        (
            r#"{"state": "  ", "expect": {"is_urgent": true}}"#,
            "`state` is empty",
        ),
        (r#"{"state": "a"}"#, "missing `expect`"),
        (r#"{"state": "a", "expect": {}}"#, "at least one question"),
        (
            r#"{"state": "a", "id": 7, "expect": {"is_urgent": true}}"#,
            "`id` must be",
        ),
        (
            r#"{"state": "a", "expect": {"nope": true}}"#,
            r#"no question named "nope""#,
        ),
        (
            r#"{"state": "a", "expect": {"is_urgent": "yes"}}"#,
            "expected true or false",
        ),
        (
            r#"{"state": "a", "expect": {"department": "legal"}}"#,
            "billing, technical, sales",
        ),
        (
            r#"{"state": "a", "expect": {"frustration": 3}}"#,
            "from 0 to 2",
        ),
        (
            r#"{"state": "a", "expect": {"frustration": "Livid"}}"#,
            "from 0 to 2",
        ),
    ] {
        let message = said(text);
        assert!(message.contains(wanted), "{message}");
    }
    assert!(said("\n\nnot json").starts_with("cases line 3:"));
}

#[test]
fn refuses_to_score_a_raw_question() {
    let mut s = session();
    s.insert(
        "tone".to_owned(),
        Question::Raw(json!({"type": "sentiment"})),
    );
    let message = why(r#"{"state": "a", "expect": {"tone": true}}"#, &s);
    assert!(message.contains("raw questions cannot"), "{message}");
}

#[test]
fn refuses_a_file_with_nothing_in_it() {
    assert_eq!(why("\n \n", &session()), "the cases file holds no cases.");
}

fn answer(value: serde_json::Value) -> typesafe::Answer {
    // The answer structs are `#[non_exhaustive]`, so a test builds them the way the wire does.
    match value["type"].as_str() {
        Some("noul") => typesafe::Answer::Noul(serde_json::from_value(value).expect("a noul")),
        Some("choice") => {
            typesafe::Answer::Choice(serde_json::from_value(value).expect("a choice"))
        }
        _ => typesafe::Answer::Score(serde_json::from_value(value).expect("a score")),
    }
}

fn noul(p: f64) -> typesafe::Answer {
    answer(json!({"type": "noul", "noul": p}))
}

fn choice(label: &str, confidence: f64) -> typesafe::Answer {
    answer(json!({
        "type": "choice", "choice": label,
        "probabilities": {label: confidence}, "confidence": confidence
    }))
}

fn score(level: f64, confidence: f64) -> typesafe::Answer {
    answer(json!({
        "type": "score", "score": level, "confidence": confidence,
        "legend": {"0": "Calm", "1": "Frustrated but civil", "2": "Very angry"},
        "probabilities": {},
    }))
}

fn usage(input: u64, output: u64) -> typesafe::Usage {
    serde_json::from_value(json!({"input_tokens": input, "output_tokens": output})).expect("usage")
}

/// One answered case: the answers by question name, and the tokens it reported using.
fn answered(answers: &[(&str, typesafe::Answer)], usage: Option<typesafe::Usage>) -> Outcome {
    Outcome::Ok {
        answers: answers
            .iter()
            .map(|(name, a)| ((*name).to_owned(), Some(a.clone())))
            .collect(),
        usage,
    }
}

fn failed(error: &str) -> Outcome {
    Outcome::Failed {
        error: error.to_owned(),
    }
}

fn scored(lines: &[&str], outcomes: Vec<Outcome>, threshold: f64, rates: Option<Rates>) -> Report {
    let s = session();
    let parsed = cases(&lines.join("\n"), &s);
    evaluate::report(
        &s,
        &parsed,
        &outcomes,
        ReportOptions {
            model: "jev-latest",
            threshold,
            rates,
        },
    )
}

/// The report for one question, which the tests always know the kind of.
fn question<'a>(report: &'a Report, name: &str) -> &'a QuestionReport {
    report
        .questions
        .iter()
        .find(|q| q.name() == name)
        .unwrap_or_else(|| panic!("no report for {name}"))
}

const URGENT: [&str; 4] = [
    r#"{"state": "one", "expect": {"is_urgent": true}}"#,
    r#"{"state": "two", "expect": {"is_urgent": true}}"#,
    r#"{"state": "three", "expect": {"is_urgent": false}}"#,
    r#"{"state": "four", "expect": {"is_urgent": true}}"#,
];

fn urgent_report(probabilities: [f64; 4], threshold: f64) -> Report {
    scored(
        &URGENT,
        probabilities
            .iter()
            .map(|p| answered(&[("is_urgent", noul(*p))], None))
            .collect(),
        threshold,
        None,
    )
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

#[test]
fn counts_the_four_corners_at_the_chosen_threshold() {
    let report = urgent_report([0.9, 0.7, 0.3, 0.1], 0.5);
    let QuestionReport::Noul {
        cases,
        accuracy,
        sweep,
        ..
    } = question(&report, "is_urgent")
    else {
        panic!("is_urgent is a noul")
    };
    let row = sweep
        .iter()
        .find(|row| row.threshold == 0.5)
        .expect("the chosen row");
    assert_eq!((row.tp, row.fp, row.r#fn, row.tn), (2, 0, 1, 1));
    assert_eq!(row.accuracy, 0.75);
    assert_eq!(row.precision, Some(1.0));
    assert!(close(row.recall.expect("a recall"), 2.0 / 3.0));
    assert!(close(row.f1, 0.8));
    assert_eq!(*accuracy, 0.75);
    assert_eq!(*cases, 4);
}

#[test]
fn scores_the_probabilities_themselves_threshold_or_no_threshold() {
    let report = urgent_report([0.9, 0.7, 0.3, 0.1], 0.5);
    let QuestionReport::Noul { brier, .. } = question(&report, "is_urgent") else {
        panic!("is_urgent is a noul")
    };
    assert!(close(*brier, (0.01 + 0.09 + 0.09 + 0.81) / 4.0), "{brier}");
}

#[test]
fn sweeps_the_round_thresholds_and_the_chosen_one_exactly_once() {
    let report = urgent_report([0.9, 0.7, 0.3, 0.1], 0.5);
    let QuestionReport::Noul { sweep, .. } = question(&report, "is_urgent") else {
        panic!("is_urgent is a noul")
    };
    assert_eq!(
        sweep.iter().map(|row| row.threshold).collect::<Vec<_>>(),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]
    );

    let odd = urgent_report([0.9, 0.7, 0.3, 0.1], 0.55);
    let QuestionReport::Noul { sweep, .. } = question(&odd, "is_urgent") else {
        panic!("is_urgent is a noul")
    };
    assert_eq!(
        sweep.iter().map(|row| row.threshold).collect::<Vec<_>>(),
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.55, 0.6, 0.7, 0.8, 0.9]
    );
    assert_eq!(sweep.iter().filter(|row| row.threshold == 0.55).count(), 1);
}

#[test]
fn leaves_precision_undefined_when_nothing_was_called_a_yes() {
    let quiet = urgent_report([0.1, 0.1, 0.1, 0.1], 0.5);
    let QuestionReport::Noul { sweep, .. } = question(&quiet, "is_urgent") else {
        panic!("is_urgent is a noul")
    };
    let row = sweep
        .iter()
        .find(|row| row.threshold == 0.5)
        .expect("the chosen row");
    assert_eq!(row.precision, None);
    assert_eq!(row.recall, Some(0.0));
    assert_eq!(row.f1, 0.0);
}

#[test]
fn breaks_a_tie_on_the_best_f1_by_taking_the_lowest_threshold() {
    let flat = scored(
        &[r#"{"state": "one", "expect": {"is_urgent": true}}"#],
        vec![answered(&[("is_urgent", noul(0.95))], None)],
        0.5,
        None,
    );
    let QuestionReport::Noul { best, .. } = question(&flat, "is_urgent") else {
        panic!("is_urgent is a noul")
    };
    assert_eq!(best.threshold, 0.1);
    assert_eq!(best.f1, 1.0);
}

const DEPARTMENTS: [&str; 4] = [
    r#"{"state": "one", "expect": {"department": "billing"}}"#,
    r#"{"state": "two", "expect": {"department": "billing"}}"#,
    r#"{"state": "three", "expect": {"department": "technical"}}"#,
    r#"{"state": "four", "expect": {"department": "sales"}}"#,
];

fn department_report() -> Report {
    scored(
        &DEPARTMENTS,
        vec![
            answered(&[("department", choice("billing", 0.9))], None),
            answered(&[("department", choice("technical", 0.5))], None),
            answered(&[("department", choice("technical", 0.3))], None),
            answered(&[("department", choice("legal", 0.1))], None),
        ],
        0.5,
        None,
    )
}

#[test]
fn orders_the_matrix_the_way_the_page_orders_its_options() {
    let report = department_report();
    let QuestionReport::Choice {
        labels,
        confusion,
        accuracy,
        ..
    } = question(&report, "department")
    else {
        panic!("department is a choice")
    };
    assert_eq!(labels, &["billing", "technical", "sales", "other"]);
    assert_eq!(
        confusion,
        &vec![vec![1, 1, 0, 0], vec![0, 1, 0, 0], vec![0, 0, 0, 1]]
    );
    assert_eq!(*accuracy, 0.5);
}

#[test]
fn shows_what_a_confidence_gate_buys() {
    let report = department_report();
    let QuestionReport::Choice { gate, .. } = question(&report, "department") else {
        panic!("department is a choice")
    };
    assert_eq!(
        gate.iter().map(|row| row.confidence).collect::<Vec<_>>(),
        vec![0.0, 0.2, 0.4, 0.6, 0.8]
    );
    assert_eq!(
        gate.iter().map(|row| row.coverage).collect::<Vec<_>>(),
        vec![1.0, 0.75, 0.5, 0.25, 0.25]
    );
    assert_eq!(gate[0].accuracy, Some(0.5));
    assert_eq!(gate[4].accuracy, Some(1.0));
}

#[test]
fn leaves_the_gates_accuracy_undefined_when_nothing_is_confident_enough() {
    let shy = scored(
        &[r#"{"state": "one", "expect": {"department": "billing"}}"#],
        vec![answered(&[("department", choice("billing", 0.1))], None)],
        0.5,
        None,
    );
    let QuestionReport::Choice { gate, .. } = question(&shy, "department") else {
        panic!("department is a choice")
    };
    assert_eq!(
        gate[4],
        GateRow {
            confidence: 0.8,
            coverage: 0.0,
            accuracy: None
        }
    );
}

fn frustration_report() -> Report {
    scored(
        &[
            r#"{"state": "one", "expect": {"frustration": 0}}"#,
            r#"{"state": "two", "expect": {"frustration": 2}}"#,
            r#"{"state": "three", "expect": {"frustration": 2}}"#,
        ],
        vec![
            answered(&[("frustration", score(0.0, 0.8))], None),
            answered(&[("frustration", score(1.4, 0.5))], None),
            answered(&[("frustration", score(0.0, 0.1))], None),
        ],
        0.5,
        None,
    )
}

#[test]
fn counts_the_exact_levels_the_neighbours_and_the_distance() {
    let report = frustration_report();
    let QuestionReport::Score {
        exact,
        within_one,
        mae,
        ..
    } = question(&report, "frustration")
    else {
        panic!("frustration is a score")
    };
    assert!(close(*exact, 1.0 / 3.0), "{exact}");
    assert!(close(*within_one, 2.0 / 3.0), "{within_one}");
    assert!(close(*mae, 1.0), "{mae}");
}

#[test]
fn gates_on_confidence_with_the_exact_levels() {
    let report = frustration_report();
    let QuestionReport::Score { gate, .. } = question(&report, "frustration") else {
        panic!("frustration is a score")
    };
    assert_eq!(gate[3].confidence, 0.6);
    assert!(close(gate[3].coverage, 1.0 / 3.0));
    assert_eq!(gate[3].accuracy, Some(1.0));
}

#[test]
fn names_cases_that_did_not_answer_and_leaves_them_out_of_every_metric() {
    let report = scored(
        &[
            r#"{"id": "t-001", "state": "one", "expect": {"is_urgent": true}}"#,
            r#"{"state": "two", "expect": {"is_urgent": true, "department": "billing"}}"#,
            r#"{"state": "three", "expect": {"is_urgent": true}}"#,
        ],
        vec![
            failed("Timeout  the request did not complete"),
            answered(&[("is_urgent", noul(0.9))], None),
            answered(&[("is_urgent", noul(0.9))], None),
        ],
        0.5,
        None,
    );
    assert_eq!(
        report.errors,
        vec![
            CaseError {
                turn: None,
                case: 1,
                id: Some("t-001".to_owned()),
                message: "Timeout  the request did not complete".to_owned(),
            },
            CaseError {
                turn: None,
                case: 2,
                id: None,
                message: "no answer came back for department".to_owned(),
            },
        ]
    );
    assert_eq!(report.cases, 3);
    assert_eq!(report.answered, 1);
    assert_eq!(question(&report, "is_urgent").cases(), 1);
    assert_eq!(
        report
            .questions
            .iter()
            .map(QuestionReport::name)
            .collect::<Vec<_>>(),
        vec!["is_urgent"]
    );
}

#[test]
fn sums_the_counts_the_api_reported_and_prices_them() {
    let report = scored(
        &URGENT[..2],
        vec![
            answered(&[("is_urgent", noul(0.9))], Some(usage(100, 20))),
            answered(&[("is_urgent", noul(0.9))], Some(usage(100, 20))),
        ],
        0.5,
        Some(Rates {
            input: 0.2,
            output: 1.0,
        }),
    );
    assert_eq!(report.usage.input_tokens, 200);
    assert_eq!(report.usage.output_tokens, 40);
    assert!(!report.usage.estimated);
    let total = report.usage.cost.expect("a price").total;
    assert!(close(total, 200.0 / 1e6 * 0.2 + 40.0 / 1e6), "{total}");
}

#[test]
fn falls_back_to_the_estimate_and_says_so_when_one_case_counted_nothing() {
    let report = scored(
        &URGENT[..2],
        vec![
            answered(&[("is_urgent", noul(0.9))], Some(usage(100, 20))),
            answered(&[("is_urgent", noul(0.9))], None),
        ],
        0.5,
        None,
    );
    assert!(report.usage.estimated);
    assert!(report.usage.input_tokens > 0);
    assert_eq!(report.usage.cost, None);
}

#[test]
fn names_the_questions_under_the_bar_with_what_they_scored() {
    let report = scored(
        &[
            r#"{"state": "one", "expect": {"is_urgent": true, "frustration": 2}}"#,
            r#"{"state": "two", "expect": {"is_urgent": true, "frustration": 2}}"#,
        ],
        vec![
            answered(
                &[("is_urgent", noul(0.9)), ("frustration", score(0.0, 0.5))],
                None,
            ),
            answered(
                &[("is_urgent", noul(0.9)), ("frustration", score(2.0, 0.5))],
                None,
            ),
        ],
        0.5,
        None,
    );
    assert_eq!(
        evaluate::below_bar(&report, 0.8),
        vec![("frustration".to_owned(), 0.5)]
    );
    assert_eq!(evaluate::below_bar(&report, 0.4), Vec::new());
}

#[test]
fn rounds_a_tie_away_from_zero_the_way_to_fixed_does() {
    assert_eq!(evaluate::two(0.125), "0.13");
    assert_eq!(evaluate::two(0.005), "0.01");
    assert_eq!(evaluate::two(0.0), "0.00");
}

fn five_cases() -> Vec<String> {
    (0..5)
        .map(|i| format!(r#"{{"state": "case {i}", "expect": {{"is_urgent": true}}}}"#))
        .collect()
}

#[tokio::test]
async fn keeps_at_most_concurrency_calls_in_the_air_and_asks_every_case_once() {
    let s = session();
    let lines = five_cases();
    let parsed = cases(&lines.join("\n"), &s);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let flying = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let outcomes = {
        let (asked, flying, most) = (asked.clone(), flying.clone(), most.clone());
        evaluate::run(
            &s,
            &parsed,
            move |one| {
                let (asked, flying, most) = (asked.clone(), flying.clone(), most.clone());
                async move {
                    let now = flying.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    asked.lock().expect("the log").push(one.state_preview());
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    flying.fetch_sub(1, Ordering::SeqCst);
                    answered(&[("is_urgent", noul(0.9))], None)
                }
            },
            2,
        )
        .await
    };
    assert_eq!(most.load(Ordering::SeqCst), 2);
    let mut seen = asked.lock().expect("the log").clone();
    seen.sort();
    assert_eq!(
        seen,
        (0..5).map(|i| format!("case {i}")).collect::<Vec<_>>()
    );
    assert_eq!(outcomes.len(), 5);
}

#[tokio::test]
async fn reports_in_case_order_however_the_calls_finished() {
    let s = session();
    let lines = five_cases();
    let parsed = cases(&lines.join("\n"), &s);
    let total = parsed.len() as u64;
    let outcomes = evaluate::run(
        &s,
        &parsed,
        move |one| async move {
            let at: u64 = one.state_preview()[5..].parse().expect("the case number");
            // The last case answers first, the first case last.
            tokio::time::sleep(Duration::from_millis((total - at) * 4)).await;
            answered(&[("is_urgent", noul(at as f64 / 10.0))], None)
        },
        5,
    )
    .await;
    let probabilities: Vec<f64> = outcomes
        .iter()
        .map(|outcome| match outcome {
            Outcome::Ok { answers, .. } => match &answers[0].1 {
                Some(typesafe::Answer::Noul(a)) => a.noul,
                _ => -1.0,
            },
            Outcome::Failed { .. } => -1.0,
        })
        .collect();
    assert_eq!(probabilities, vec![0.0, 0.1, 0.2, 0.3, 0.4]);
}

#[tokio::test]
async fn turns_a_refused_call_into_an_outcome_and_carries_on() {
    let s = session();
    let lines = five_cases();
    let parsed = cases(&lines.join("\n"), &s);
    let outcomes = evaluate::run(
        &s,
        &parsed,
        |one| async move {
            if one.state_preview() == "case 2" {
                failed("the socket went away")
            } else {
                answered(&[("is_urgent", noul(0.9))], None)
            }
        },
        2,
    )
    .await;
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, Outcome::Ok { .. }))
            .count(),
        4
    );
    let Outcome::Failed { error } = &outcomes[2] else {
        panic!("case 2 failed")
    };
    assert!(error.contains("the socket went away"), "{error}");
}

#[tokio::test]
async fn does_not_mind_more_workers_than_there_are_cases() {
    let s = session();
    let parsed = cases(&five_cases()[0], &s);
    let outcomes = evaluate::run(
        &s,
        &parsed,
        |_| async { answered(&[("is_urgent", noul(0.9))], None) },
        16,
    )
    .await;
    assert_eq!(outcomes.len(), 1);
}

const EVERY: [&str; 3] = [
    r#"{"id": "t-001", "state": "one", "expect": {"is_urgent": true, "department": "billing", "frustration": 2}}"#,
    r#"{"state": "two", "expect": {"is_urgent": false, "department": "technical", "frustration": 0}}"#,
    r#"{"state": "three", "expect": {"is_urgent": true, "department": "sales"}}"#,
];

fn every(p: f64, label: &str, level: f64) -> Vec<(&'static str, typesafe::Answer)> {
    vec![
        ("is_urgent", noul(p)),
        ("department", choice(label, 0.7)),
        ("frustration", score(level, 0.4)),
    ]
}

fn text_report() -> String {
    let report = scored(
        &EVERY,
        vec![
            answered(&every(0.9, "billing", 2.0), Some(usage(100, 20))),
            answered(&every(0.2, "sales", 1.0), Some(usage(100, 20))),
            failed("Timeout  the request did not complete\n    kind: timeout"),
        ],
        0.5,
        None,
    );
    evaluate::report_text(&report)
}

/// The one line of the report that mentions `needle`, for the landmarks that are a whole row.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line mentions {needle} in\n{text}"))
}

#[test]
fn gives_every_question_a_block_that_says_what_it_is_and_what_it_scored() {
    let text = text_report();
    let urgent = line_with(&text, "is_urgent");
    assert!(urgent.contains("noul"), "{urgent}");
    assert!(urgent.contains("2 cases · Brier"), "{urgent}");
    let department = line_with(&text, "department");
    assert!(department.contains("choice"), "{department}");
    assert!(department.contains("2 cases · accuracy"), "{department}");
    let frustration = line_with(&text, "frustration");
    assert!(frustration.contains("score"), "{frustration}");
    assert!(
        frustration.contains("2 cases · exact 0.50 · within one 1.00 · mae 0.50"),
        "{frustration}"
    );
}

#[test]
fn marks_the_threshold_this_run_used_and_names_the_best_one() {
    let text = text_report();
    assert!(text.contains("0.50 *"), "{text}");
    assert!(text.contains("best f1 at 0."), "{text}");
}

#[test]
fn draws_the_gate_and_the_matrix() {
    let text = text_report();
    assert!(text.contains("confidence ≥"), "{text}");
    assert!(
        text.contains("confusion, rows expected, columns predicted"),
        "{text}"
    );
    assert!(text.contains("technical"), "{text}");
}

#[test]
fn names_the_cases_that_did_not_answer_and_totals_the_run() {
    let text = text_report();
    assert!(text.contains("case 3: Timeout"), "{text}");
    assert!(text.contains("3 cases · 2 answered · 1 error"), "{text}");
    assert!(text.contains("200 in / 40 out tokens"), "{text}");
    // A dot stands where a rate was never defined.
    assert!(text.contains('·'), "{text}");
}

#[test]
fn marks_an_estimate_as_one() {
    let report = scored(
        &EVERY[..1],
        vec![answered(&every(0.9, "billing", 2.0), None)],
        0.5,
        None,
    );
    let text = evaluate::report_text(&report);
    assert!(text.contains('≈'), "{text}");
    assert!(text.contains("estimated, nothing was counted"), "{text}");
}

fn json_report() -> serde_json::Value {
    let report = scored(
        &[
            r#"{"id": "t-001", "state": "one", "expect": {"is_urgent": true, "department": "billing"}}"#,
            r#"{"state": "two", "expect": {"is_urgent": false, "department": "technical"}}"#,
        ],
        vec![
            answered(
                &[
                    ("is_urgent", noul(0.85)),
                    ("department", choice("billing", 0.9)),
                ],
                Some(usage(100, 20)),
            ),
            answered(
                &[
                    ("is_urgent", noul(0.45)),
                    ("department", choice("legal", 0.1)),
                ],
                Some(usage(100, 20)),
            ),
        ],
        0.5,
        Some(Rates {
            input: 0.2,
            output: 1.0,
        }),
    );
    evaluate::report_json(&report)
}

#[test]
fn has_the_shape_a_script_can_read() {
    let json = json_report();
    assert_eq!(json["model"], "jev-latest");
    assert_eq!(json["threshold"], 0.5);
    assert_eq!(json["cases"], 2);
    assert_eq!(json["answered"], 2);
    assert_eq!(json["errors"], json!([]));
    let urgent = &json["questions"]["is_urgent"];
    assert_eq!(urgent["kind"], "noul");
    assert_eq!(urgent["cases"], 2);
    assert_eq!(urgent["accuracy"], 1.0);
    assert_eq!(urgent["best"], json!({"threshold": 0.5, "f1": 1}));
    assert_eq!(
        json["questions"]["department"]["labels"],
        json!(["billing", "technical", "sales", "other"])
    );
    assert_eq!(
        json["usage"],
        json!({"inputTokens": 200, "outputTokens": 40, "estimated": false, "cost": 0.00008})
    );
    // A whole number loses its `.0`, the way `JSON.stringify` writes it.
    assert_eq!(
        serde_json::to_string(&urgent["best"]).expect("json"),
        r#"{"threshold":0.5,"f1":1}"#
    );
    let first = &urgent["sweep"][0];
    assert_eq!(first["threshold"], 0.1);
    assert_eq!((&first["tp"], &first["fp"]), (&json!(1), &json!(1)));
    assert_eq!((&first["fn"], &first["tn"]), (&json!(0), &json!(0)));
}

#[test]
fn keeps_what_is_undefined_as_null_so_the_keys_are_always_there() {
    let json = json_report();
    let sweep = json["questions"]["is_urgent"]["sweep"]
        .as_array()
        .expect("a sweep")
        .clone();
    let last = sweep.last().expect("a last row");
    assert_eq!(last["threshold"], 0.9);
    assert_eq!(last["precision"], json!(null));
    assert_eq!(last["recall"], 0.0);
}

#[test]
fn adds_up_what_every_case_would_cost_before_anything_is_sent() {
    let s = session();
    let parsed = cases(&URGENT.join("\n"), &s);
    let one = evaluate::preflight(&s, &parsed[..1], "jev-latest", None);
    let all = evaluate::preflight(
        &s,
        &parsed,
        "jev-latest",
        Some(Rates {
            input: 0.2,
            output: 1.0,
        }),
    );
    assert_eq!(all.cases, 4);
    assert!(all.input_tokens > one.input_tokens);
    assert!(all.cost.expect("a price").total > 0.0);
    assert_eq!(one.cost, None);
}

// ---- two pages over the same cases ------------------------------------------------------------

/// The candidate: the same noul and choice, a score renamed away, a noul of its own added.
const PAGE_B: &str = "A payout failed for the third time.
---
is_urgent? The message conveys urgency or time pressure
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales = Pricing and plans
frustration? Is the customer frustrated
sarcasm? Is the customer being sarcastic
";

fn session_b() -> Session {
    headless::load(PAGE_B).expect("page b parses")
}

const LABELS: evaluate::Labels<'static> = evaluate::Labels {
    a: "a.jev",
    b: "b.jev",
};

fn names(expect: &[(String, Expectation)]) -> Vec<&str> {
    expect.iter().map(|(name, _)| name.as_str()).collect()
}

#[test]
fn gives_each_page_the_expectations_for_its_own_questions() {
    let (a, b) = evaluate::parse_compare_cases(
        concat!(
            r#"{"state": "one", "expect": {"is_urgent": true, "sarcasm": false}}"#,
            "\n",
            r#"{"state": "two", "expect": {"sarcasm": true}}"#,
        ),
        &session(),
        &session_b(),
        LABELS,
    )
    .expect("these cases parse");
    // The second case asks nothing of page a, so page a does not send it.
    assert_eq!(a.iter().map(|c| c.line).collect::<Vec<_>>(), vec![1]);
    assert_eq!(names(&a[0].expect), vec!["is_urgent"]);
    assert_eq!(b.iter().map(|c| c.line).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(names(&b[0].expect), vec!["is_urgent", "sarcasm"]);
}

#[test]
fn names_a_question_on_neither_page_and_the_page_a_label_does_not_fit() {
    let why2 = |line: &str| {
        evaluate::parse_compare_cases(line, &session(), &session_b(), LABELS)
            .expect_err("expected these cases to be rejected")
    };
    assert_eq!(
        why2(r#"{"state": "a", "expect": {"nope": true}}"#),
        r#"cases line 1: no question named "nope" on either page."#
    );
    // frustration is a score on page a and a noul on page b: `2` only fits one of them.
    assert_eq!(
        why2(r#"{"state": "a", "expect": {"frustration": 2}}"#),
        "cases line 1: b.jev: frustration is a noul: expected true or false, got 2."
    );
    assert!(why2("\nnot json").starts_with("cases line 2: not valid JSON"));
}

#[test]
fn tests_the_discordant_pairs_as_a_binomial_tail() {
    let six = evaluate::mcnemar(0, 6);
    assert_eq!(six.discordant, 6);
    assert!(close(six.p, 2.0 / 64.0), "{}", six.p);
    assert_eq!(six.verdict, evaluate::Verdict::Worse);
    assert_eq!(evaluate::mcnemar(6, 0).verdict, evaluate::Verdict::Better);
    let one = evaluate::mcnemar(1, 5);
    assert!(close(one.p, 14.0 / 64.0), "{}", one.p);
    assert_eq!(one.verdict, evaluate::Verdict::Same);
}

#[test]
fn says_too_few_below_six_pairs_where_no_p_can_reach_the_level() {
    let five = evaluate::mcnemar(0, 5);
    assert_eq!(five.discordant, 5);
    assert!(close(five.p, 2.0 / 32.0), "{}", five.p);
    assert_eq!(five.verdict, evaluate::Verdict::TooFew);
    let none = evaluate::mcnemar(0, 0);
    assert_eq!((none.discordant, none.p), (0, 1.0));
    assert_eq!(none.verdict.as_str(), "too few");
}

#[test]
fn stays_a_number_when_two_to_the_n_is_not_one() {
    let test = evaluate::mcnemar(1500, 1600);
    assert!(test.p.is_finite());
    assert!(test.p > 0.05 && test.p < 1.0, "{}", test.p);
    assert_eq!(
        evaluate::mcnemar(1000, 1400).verdict,
        evaluate::Verdict::Worse
    );
}

const COMPARED: [&str; 4] = [
    r#"{"id": "t-1", "state": "one", "expect": {"is_urgent": true, "department": "billing"}}"#,
    r#"{"state": "two", "expect": {"is_urgent": false, "department": "technical"}}"#,
    r#"{"state": "three", "expect": {"is_urgent": true, "department": "sales"}}"#,
    r#"{"state": "four", "expect": {"is_urgent": false, "sarcasm": true}}"#,
];

fn comparison() -> evaluate::Comparison {
    let (a, b) = (session(), session_b());
    let (cases_a, cases_b) =
        evaluate::parse_compare_cases(&COMPARED.join("\n"), &a, &b, LABELS).expect("they parse");
    let outcomes_a = vec![
        answered(
            &[
                ("is_urgent", noul(0.9)),
                ("department", choice("billing", 0.9)),
            ],
            None,
        ),
        answered(
            &[
                ("is_urgent", noul(0.8)),
                ("department", choice("billing", 0.6)),
            ],
            None,
        ),
        answered(
            &[
                ("is_urgent", noul(0.2)),
                ("department", choice("billing", 0.5)),
            ],
            None,
        ),
        answered(&[("is_urgent", noul(0.1))], None),
    ];
    let outcomes_b = vec![
        // Broke: a said yes and was right, b says no.
        answered(
            &[
                ("is_urgent", noul(0.3)),
                ("department", choice("billing", 0.9)),
            ],
            None,
        ),
        // Fixed both: a was wrong on both questions, b is right.
        answered(
            &[
                ("is_urgent", noul(0.1)),
                ("department", choice("technical", 0.8)),
            ],
            None,
        ),
        // Changed: a and b both pick a wrong department, a different one each.
        answered(
            &[
                ("is_urgent", noul(0.2)),
                ("department", choice("technical", 0.7)),
            ],
            None,
        ),
        failed("Timeout"),
    ];
    evaluate::compare(
        evaluate::Side {
            label: "a.jev",
            session: &a,
            cases: &cases_a,
            outcomes: &outcomes_a,
            model: "jev-latest",
        },
        evaluate::Side {
            label: "b.jev",
            session: &b,
            cases: &cases_b,
            outcomes: &outcomes_b,
            model: "jev-2",
        },
        evaluate::CompareOptions {
            threshold: 0.5,
            rates: None,
        },
    )
}

fn shared<'a>(comparison: &'a evaluate::Comparison, name: &str) -> &'a evaluate::Shared {
    comparison
        .questions
        .iter()
        .find(|q| q.name == name)
        .unwrap_or_else(|| panic!("no shared question {name}"))
}

#[test]
fn pairs_only_the_cases_both_pages_scored() {
    let comparison = comparison();
    let urgent = shared(&comparison, "is_urgent");
    assert_eq!(urgent.paired, 3);
    let metrics: Vec<(&str, f64, f64)> = urgent.metrics.iter().map(|m| (m.key, m.a, m.b)).collect();
    assert_eq!(metrics[0], ("threshold", 0.5, 0.5));
    assert_eq!(metrics[1].0, "brier");
    assert!(close(metrics[1].1, (0.01 + 0.64 + 0.64) / 3.0));
    assert!(close(metrics[1].2, 1.14 / 3.0));
    assert_eq!(metrics[2], ("accuracy", 1.0 / 3.0, 1.0 / 3.0));
    assert_eq!(metrics[3], ("f1", 0.5, 0.0));
}

#[test]
fn calls_each_flip_fixed_broke_or_changed() {
    let comparison = comparison();
    let urgent = shared(&comparison, "is_urgent");
    assert_eq!(
        urgent.flips,
        vec![
            evaluate::Flip {
                turn: None,
                case: 1,
                id: Some("t-1".to_owned()),
                expected: json!(true),
                a: json!(true),
                b: json!(false),
                status: "broke",
            },
            evaluate::Flip {
                turn: None,
                case: 2,
                id: None,
                expected: json!(false),
                a: json!(true),
                b: json!(false),
                status: "fixed",
            },
        ]
    );
    let department = shared(&comparison, "department");
    assert_eq!(
        (department.fixed, department.broke, department.changed),
        (1, 0, 1)
    );
    assert_eq!(department.mcnemar.verdict, evaluate::Verdict::TooFew);
}

#[test]
fn lists_what_could_not_be_compared() {
    let comparison = comparison();
    let names: Vec<&str> = comparison
        .questions
        .iter()
        .map(|q| q.name.as_str())
        .collect();
    assert_eq!(names, vec!["is_urgent", "department"]);
    assert!(comparison.only_a.is_empty());
    assert_eq!(comparison.only_b, vec!["sarcasm"]);
    assert_eq!(
        comparison.mismatched,
        vec![evaluate::Mismatch {
            name: "frustration".to_owned(),
            a: "score",
            b: "noul",
        }]
    );
    assert_eq!(comparison.cases, 4);
    assert_eq!(
        comparison.b.report.errors,
        vec![CaseError {
            turn: None,
            case: 4,
            id: None,
            message: "Timeout".to_owned(),
        }]
    );
}

/// Whether some line of `text` matches every part of `parts` in order, with any run of spaces
/// between them — the Rust spelling of the reference's `toMatch(/a\s+b\s+c/)`.
fn has_row(text: &str, parts: &[&str]) -> bool {
    text.lines().any(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let wanted: Vec<&str> = parts.iter().flat_map(|p| p.split_whitespace()).collect();
        words.windows(wanted.len()).any(|w| w == wanted.as_slice())
    })
}

#[test]
fn draws_the_difference_and_says_what_the_test_can_and_cannot_tell() {
    let text = evaluate::compare_text(&comparison());
    for wanted in [
        "  a  a.jev  jev-latest · 4 cases",
        "  b  b.jev  jev-2 · 4 cases",
        "1 fixed · 1 broke · 0 changed",
        "McNemar: too few discordant pairs to call (2; 6 are needed for p < 0.05)",
        "only in b: sarcasm",
        "mismatched: frustration is a score in a and a noul in b",
        "b case 4: Timeout",
        "4 cases · a 4 answered, 0 errors · b 3 answered, 1 error",
    ] {
        assert!(text.contains(wanted), "{wanted:?} in\n{text}");
    }
    for row in [
        &["is_urgent", "noul", "3 paired cases"][..],
        &["accuracy", "0.33", "0.33", "+0.00"],
        &["f1", "0.50", "0.00", "-0.50"],
        &["case 1 (t-1)", "yes → no", "broke"],
        &["case 3", "billing → technical", "changed"],
    ] {
        assert!(has_row(&text, row), "{row:?} in\n{text}");
    }
}

#[test]
fn prints_the_json_a_script_reads() {
    let json = evaluate::compare_json(&comparison());
    assert_eq!(json["a"]["page"], "a.jev");
    assert_eq!(json["a"]["model"], "jev-latest");
    assert_eq!(json["a"]["cases"], 4);
    assert_eq!(json["b"]["page"], "b.jev");
    assert_eq!(json["b"]["model"], "jev-2");
    assert_eq!(json["b"]["answered"], 3);
    let first_key = json["a"].as_object().and_then(|o| o.keys().next().cloned());
    assert_eq!(first_key.as_deref(), Some("page"));
    let urgent = &json["questions"]["is_urgent"];
    assert_eq!(urgent["kind"], "noul");
    assert_eq!(urgent["paired"], 3);
    assert_eq!(urgent["a"]["threshold"], 0.5);
    assert_eq!(urgent["a"]["accuracy"], 1.0 / 3.0);
    assert_eq!(urgent["a"]["f1"], 0.5);
    assert_eq!(urgent["b"]["f1"], 0);
    assert_eq!(urgent["delta"]["accuracy"], 0);
    assert_eq!(urgent["delta"]["f1"], -0.5);
    assert_eq!(
        urgent["delta"]
            .as_object()
            .expect("an object")
            .keys()
            .collect::<Vec<_>>(),
        vec!["brier", "accuracy", "f1"]
    );
    assert_eq!((&urgent["fixed"], &urgent["broke"]), (&json!(1), &json!(1)));
    assert_eq!(urgent["changed"], 0);
    assert_eq!(
        urgent["mcnemar"],
        json!({"discordant": 2, "p": 1, "verdict": "too few"})
    );
    assert_eq!(
        urgent["flips"][0],
        json!({"case": 1, "id": "t-1", "expected": true, "a": true, "b": false, "status": "broke"})
    );
    let department = &json["questions"]["department"];
    assert_eq!(department["kind"], "choice");
    assert_eq!(department["a"]["accuracy"], 1.0 / 3.0);
    assert_eq!(department["b"]["accuracy"], 2.0 / 3.0);
    assert_eq!(json["onlyA"], json!([]));
    assert_eq!(json["onlyB"], json!(["sarcasm"]));
    assert_eq!(
        json["mismatched"],
        json!([{"name": "frustration", "a": "score", "b": "noul"}])
    );
    assert_eq!(json["unpaired"], json!([]));
    assert_eq!(json["regressions"], json!([]));
    assert_eq!(json["usage"]["estimated"], true);
}

#[test]
fn names_the_questions_b_is_significantly_worse_at() {
    let (a, b) = (session(), session_b());
    let many: Vec<String> = (0..8)
        .map(|i| format!(r#"{{"state": "s{i}", "expect": {{"is_urgent": true}}}}"#))
        .collect();
    let (left, right) =
        evaluate::parse_compare_cases(&many.join("\n"), &a, &b, LABELS).expect("they parse");
    let high: Vec<Outcome> = left
        .iter()
        .map(|_| answered(&[("is_urgent", noul(0.9))], None))
        .collect();
    let low: Vec<Outcome> = right
        .iter()
        .map(|_| answered(&[("is_urgent", noul(0.1))], None))
        .collect();
    let worse = evaluate::compare(
        evaluate::Side {
            label: "a",
            session: &a,
            cases: &left,
            outcomes: &high,
            model: "m",
        },
        evaluate::Side {
            label: "b",
            session: &b,
            cases: &right,
            outcomes: &low,
            model: "m",
        },
        evaluate::CompareOptions {
            threshold: 0.5,
            rates: None,
        },
    );
    let names: Vec<&str> = evaluate::regressions(&worse)
        .iter()
        .map(|q| q.name.as_str())
        .collect();
    assert_eq!(names, vec!["is_urgent"]);
    assert!(
        evaluate::compare_text(&worse)
            .contains("McNemar p 0.008 over 8 discordant pairs: b is significantly worse")
    );
}

#[tokio::test]
async fn runs_both_pages_through_one_pool_and_hands_each_its_outcomes_in_order() {
    let (a, b) = (session(), session_b());
    let (cases_a, cases_b) =
        evaluate::parse_compare_cases(&COMPARED.join("\n"), &a, &b, LABELS).expect("they parse");
    let flying = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let ask = |tag: &'static str| {
        let (flying, most) = (Arc::clone(&flying), Arc::clone(&most));
        move |one: Session| {
            let (flying, most) = (Arc::clone(&flying), Arc::clone(&most));
            async move {
                let now = flying.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(3)).await;
                flying.fetch_sub(1, Ordering::SeqCst);
                failed(&format!("{tag}:{}", one.state.as_str().unwrap_or_default()))
            }
        }
    };
    let (left, right) = evaluate::run_compare(
        evaluate::Leg {
            session: &a,
            cases: &cases_a,
            ask: ask("a"),
        },
        evaluate::Leg {
            session: &b,
            cases: &cases_b,
            ask: ask("b"),
        },
        3,
    )
    .await;
    assert_eq!(most.load(Ordering::SeqCst), 3);
    let errors = |outcomes: &[Outcome]| -> Vec<String> {
        outcomes
            .iter()
            .map(|o| match o {
                Outcome::Failed { error } => error.clone(),
                Outcome::Ok { .. } => String::new(),
            })
            .collect()
    };
    assert_eq!(errors(&left), vec!["a:one", "a:two", "a:three", "a:four"]);
    assert_eq!(errors(&right), vec!["b:one", "b:two", "b:three", "b:four"]);
}

#[test]
fn writes_decimals_the_way_to_fixed_does_even_off_a_tie() {
    // 0.475 is a hair below itself in binary, so `toFixed(2)` rounds it down; scaling by 100 first
    // would land on 47.5 and round it up.
    assert_eq!(evaluate::two(0.475), "0.47");
    assert_eq!(evaluate::two(1.005), "1.00");
    assert_eq!(evaluate::two(0.375), "0.38");
    assert_eq!(evaluate::two(-0.125), "-0.13");
    assert_eq!(evaluate::two(-0.0), "0.00");
    assert_eq!(evaluate::two(9.995), "9.99");
    assert_eq!(evaluate::two(0.995), "0.99");
    assert_eq!(evaluate::two(99.875), "99.88");
    assert_eq!(evaluate::three(0.0625), "0.063");
    assert_eq!(evaluate::three(0.0078125), "0.008");
    assert_eq!(evaluate::signed(-0.001), "+0.00");
    assert_eq!(evaluate::signed(0.05), "+0.05");
    assert_eq!(evaluate::signed(-0.5), "-0.50");
}

// ---- the page's own bars, and writing them back -----------------------------------------------

fn barred() -> Session {
    let mut s = session();
    s.set_bar("is_urgent", 0.8);
    s
}

#[test]
fn stars_and_scores_the_pages_threshold_not_the_runs() {
    let s = barred();
    let report = evaluate::report(
        &s,
        &cases(&URGENT.join("\n"), &s),
        &[0.9, 0.7, 0.3, 0.1]
            .iter()
            .map(|p| answered(&[("is_urgent", noul(*p))], None))
            .collect::<Vec<_>>(),
        ReportOptions {
            model: "jev-latest",
            threshold: 0.5,
            rates: None,
        },
    );
    let QuestionReport::Noul {
        threshold,
        accuracy,
        ..
    } = question(&report, "is_urgent")
    else {
        panic!("is_urgent is a noul")
    };
    assert_eq!((*threshold, *accuracy), (0.8, 0.5));
    let text = evaluate::report_text(&report);
    assert!(
        text.lines()
            .any(|l| l.trim_start().starts_with("0.80 *") && l.contains("0.50")),
        "{text}"
    );
    assert!(!text.contains("0.50 *"), "{text}");
    let json = evaluate::report_json(&report);
    assert_eq!(json["threshold"], 0.5);
    let keys: Vec<&String> = json["questions"]["is_urgent"]
        .as_object()
        .expect("an object")
        .keys()
        .take(5)
        .collect();
    assert_eq!(
        keys,
        vec!["kind", "cases", "brier", "threshold", "accuracy"]
    );
}

#[test]
fn compares_each_page_at_its_own_threshold() {
    let (a, b) = (session(), barred());
    let (left, right) =
        evaluate::parse_compare_cases(&URGENT.join("\n"), &a, &b, LABELS).expect("they parse");
    let outcomes: Vec<Outcome> = [0.9, 0.7, 0.3, 0.1]
        .iter()
        .map(|p| answered(&[("is_urgent", noul(*p))], None))
        .collect();
    let comparison = evaluate::compare(
        evaluate::Side {
            label: "a",
            session: &a,
            cases: &left,
            outcomes: &outcomes,
            model: "m",
        },
        evaluate::Side {
            label: "b",
            session: &b,
            cases: &right,
            outcomes: &outcomes,
            model: "m",
        },
        evaluate::CompareOptions {
            threshold: 0.5,
            rates: None,
        },
    );
    let urgent = &comparison.questions[0];
    assert_eq!(
        (
            urgent.metrics[0].key,
            urgent.metrics[0].a,
            urgent.metrics[0].b
        ),
        ("threshold", 0.5, 0.8)
    );
    // 0.7 is a yes at 0.5 and a no at 0.8, and the case expects a yes.
    assert_eq!(
        urgent.flips,
        vec![evaluate::Flip {
            turn: None,
            case: 2,
            id: None,
            expected: json!(true),
            a: json!(true),
            b: json!(false),
            status: "broke",
        }]
    );
}

const CALIBRATED: [&str; 4] = [
    r#"{"state": "a", "expect": {"is_urgent": true, "department": "billing", "frustration": 0}}"#,
    r#"{"state": "b", "expect": {"is_urgent": true, "department": "billing", "frustration": 1}}"#,
    r#"{"state": "c", "expect": {"is_urgent": false, "department": "technical", "frustration": 2}}"#,
    r#"{"state": "d", "expect": {"is_urgent": false, "department": "sales", "frustration": 2}}"#,
];

fn calibrated(target: f64, s: &Session) -> evaluate::Calibration {
    let outcomes = vec![
        answered(
            &[
                ("is_urgent", noul(0.9)),
                ("department", choice("billing", 0.9)),
                ("frustration", score(1.0, 0.3)),
            ],
            None,
        ),
        answered(
            &[
                ("is_urgent", noul(0.65)),
                ("department", choice("billing", 0.72)),
                ("frustration", score(1.0, 0.3)),
            ],
            None,
        ),
        answered(
            &[
                ("is_urgent", noul(0.55)),
                ("department", choice("technical", 0.66)),
                ("frustration", score(2.0, 0.3)),
            ],
            None,
        ),
        answered(
            &[
                ("is_urgent", noul(0.2)),
                ("department", choice("billing", 0.4)),
                ("frustration", score(0.0, 0.3)),
            ],
            None,
        ),
    ];
    let parsed = cases(&CALIBRATED.join("\n"), s);
    let report = evaluate::report(
        s,
        &parsed,
        &outcomes,
        ReportOptions {
            model: "jev-latest",
            threshold: 0.5,
            rates: None,
        },
    );
    evaluate::calibrate(s, &parsed, &outcomes, &report, target)
}

#[test]
fn takes_a_nouls_best_f1_threshold_and_the_lowest_confidence_bar_that_reaches_the_target() {
    let c = calibrated(0.9, &session());
    let (urgent, department, frustration) = (&c.questions[0], &c.questions[1], &c.questions[2]);
    assert_eq!(urgent.name, "is_urgent");
    assert_eq!(
        (urgent.bar, urgent.was, urgent.f1),
        (Some(0.6), None, Some(1.0))
    );
    // At 0.45 the 0.4 billing miss is gone and the three that are left are right.
    assert_eq!(
        (department.bar, department.accuracy, department.coverage),
        (Some(0.45), Some(1.0), Some(0.75))
    );
    assert_eq!(frustration.bar, None);
    assert_eq!(
        frustration.reason.as_deref(),
        Some("no confidence bar reaches accuracy 0.90 (best 0.50 at 0.00)")
    );
    assert_eq!(
        c.changed,
        vec![
            ("is_urgent".to_owned(), 0.6),
            ("department".to_owned(), 0.45)
        ]
    );
}

#[test]
fn prints_the_cuts_as_the_short_decimals_they_are() {
    let cuts: Vec<String> = evaluate::calibration_cuts()
        .iter()
        .map(|c| c.to_string())
        .collect();
    assert_eq!(cuts[..4], ["0", "0.05", "0.1", "0.15"]);
    assert_eq!(cuts.len(), 20);
    assert_eq!(cuts[13], "0.65");
}

#[test]
fn leaves_a_bar_that_is_already_right_alone_and_says_so() {
    let mut s = session();
    s.set_bar("is_urgent", 0.6);
    let c = calibrated(0.5, &s);
    assert_eq!(
        (c.questions[0].bar, c.questions[0].was),
        (Some(0.6), Some(0.6))
    );
    assert!(!c.changed.iter().any(|(name, _)| name == "is_urgent"));
    let text = evaluate::calibration_text(&c, "triage.jev");
    assert!(
        has_row(
            &text,
            &["is_urgent", "@threshold 0.6", "unchanged", "f1 1.00"]
        ),
        "{text}"
    );
    // At a target of 0.5 every answer is good enough, so the bar is 0: act on all of them.
    assert!(
        has_row(
            &text,
            &[
                "department",
                "@confidence 0",
                "was none",
                "accuracy 0.75 over 1.00 of cases"
            ]
        ),
        "{text}"
    );
}

#[test]
fn says_what_it_wrote_and_what_it_left_alone() {
    let c = calibrated(0.9, &session());
    let text = evaluate::calibration_text(&c, "triage.jev");
    assert!(text.contains("calibration  target accuracy 0.90"), "{text}");
    assert!(
        has_row(
            &text,
            &[
                "frustration",
                "left alone",
                "no confidence bar reaches accuracy 0.90"
            ]
        ),
        "{text}"
    );
    assert!(text.contains("wrote 2 bars to triage.jev"), "{text}");
    assert_eq!(
        serde_json::Value::Object(evaluate::calibration_json(&c, "triage.jev")),
        json!({
            "page": "triage.jev",
            "target": 0.9,
            "written": true,
            "questions": {
                "is_urgent": {"kind": "noul", "bar": 0.6, "was": null, "f1": 1},
                "department": {"kind": "choice", "bar": 0.45, "was": null, "accuracy": 1, "coverage": 0.75},
                "frustration": {
                    "kind": "score",
                    "bar": null,
                    "was": null,
                    "reason": "no confidence bar reaches accuracy 0.90 (best 0.50 at 0.00)",
                },
            },
        })
    );
    assert_eq!(
        evaluate::not_calibrating(1),
        "not calibrating: 1 case came back with errors, so the numbers are incomplete."
    );
    assert_eq!(
        evaluate::not_calibrating(2),
        "not calibrating: 2 cases came back with errors, so the numbers are incomplete."
    );
}

// ---- a conversation labelled per turn ---------------------------------------------------------

const THREAD: &str = r#"[{"who":"customer","said":"Hi"},{"who":"customer","said":"It is down"},{"who":"customer","said":"We lose money every minute"}]"#;

fn yes_of(case: &evaluate::Case, name: &str) -> Option<bool> {
    case.expect
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, e)| matches!(e, Expectation::Noul { yes: true, .. }))
}

#[test]
fn sends_a_conversation_once_per_turn_each_prefix_a_case_of_its_own() {
    let parsed = cases(
        &format!(
            r#"{{"id": "t-9", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 2}}, "department": "technical"}}}}"#
        ),
        &session(),
    );
    let shape: Vec<_> = parsed
        .iter()
        .map(|c| (c.line, c.turn, c.turns, c.id.as_deref()))
        .collect();
    assert_eq!(
        shape,
        vec![
            (1, Some(1), Some(3), Some("t-9")),
            (1, Some(2), Some(3), Some("t-9")),
            (1, Some(3), Some(3), Some("t-9")),
        ]
    );
    let lengths: Vec<usize> = parsed
        .iter()
        .map(|c| c.state.as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(lengths, vec![1, 2, 3]);
    let yes: Vec<Option<bool>> = parsed.iter().map(|c| yes_of(c, "is_urgent")).collect();
    assert_eq!(yes, vec![Some(false), Some(true), Some(true)]);
    // A plain label was written about the whole conversation, so only the last prefix carries it.
    let department: Vec<bool> = parsed
        .iter()
        .map(|c| c.expect.iter().any(|(n, _)| n == "department"))
        .collect();
    assert_eq!(department, vec![false, false, true]);
}

#[test]
fn reads_null_as_never_and_leaves_a_case_without_by_turn_exactly_as_it_was() {
    let never = cases(
        &format!(r#"{{"state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": null}}}}}}"#),
        &session(),
    );
    let yes: Vec<Option<bool>> = never.iter().map(|c| yes_of(c, "is_urgent")).collect();
    assert_eq!(yes, vec![Some(false); 3]);
    let plain = cases(
        &format!(r#"{{"state": {THREAD}, "expect": {{"is_urgent": true}}}}"#),
        &session(),
    );
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].turn, None);
}

#[test]
fn says_what_is_wrong_with_a_per_turn_label_by_line() {
    let s = session();
    assert_eq!(
        why(
            r#"{"state": "text", "expect": {"is_urgent": {"by_turn": 1}}}"#,
            &s
        ),
        "cases line 1: is_urgent gives by_turn, but the state is not a conversation of turns."
    );
    assert_eq!(
        why(
            &format!(r#"{{"state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 7}}}}}}"#),
            &s
        ),
        "cases line 1: is_urgent by_turn must be a whole turn from 1 to 3, or null for never; got 7."
    );
    assert!(
        why(
            &format!(r#"{{"state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 1.5}}}}}}"#),
            &s
        )
        .contains("got 1.5.")
    );
    assert_eq!(
        why(
            &format!(r#"{{"state": {THREAD}, "expect": {{"is_urgent": {{"when": 3}}}}}}"#),
            &s
        ),
        r#"cases line 1: is_urgent: a per-turn expectation is {"by_turn": n}, the turn it becomes true, or null for never; got {"when":3}."#
    );
    assert_eq!(
        why(
            &format!(r#"{{"state": {THREAD}, "expect": {{"department": {{"by_turn": 2}}}}}}"#),
            &s
        ),
        "cases line 1: by_turn is for a noul, and department is a choice."
    );
}

fn latency_lines() -> Vec<String> {
    vec![
        format!(
            r#"{{"id": "on-time", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 2}}}}}}"#
        ),
        format!(
            r#"{{"id": "early", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 3}}}}}}"#
        ),
        format!(
            r#"{{"id": "missed", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 3}}}}}}"#
        ),
        format!(
            r#"{{"id": "quiet", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": null}}}}}}"#
        ),
        format!(
            r#"{{"id": "broken", "state": {THREAD}, "expect": {{"is_urgent": {{"by_turn": 1}}}}}}"#
        ),
        r#"{"state": "plain", "expect": {"is_urgent": true}}"#.to_owned(),
    ]
}

fn latency_report() -> Report {
    const PROBABILITIES: [[f64; 3]; 5] = [
        [0.1, 0.8, 0.9],
        [0.2, 0.7, 0.9],
        [0.1, 0.1, 0.2],
        [0.1, 0.6, 0.1],
        [0.9, 0.9, 0.9],
    ];
    let s = session();
    let parsed = cases(&latency_lines().join("\n"), &s);
    let outcomes: Vec<Outcome> = parsed
        .iter()
        .enumerate()
        .map(|(at, one)| match one.turn {
            None => answered(&[("is_urgent", noul(0.9))], None),
            Some(2) if one.id.as_deref() == Some("broken") => failed("Timeout"),
            Some(turn) => answered(
                &[("is_urgent", noul(PROBABILITIES[at / 3][turn - 1]))],
                None,
            ),
        })
        .collect();
    evaluate::report(
        &s,
        &parsed,
        &outcomes,
        ReportOptions {
            model: "jev-latest",
            threshold: 0.5,
            rates: None,
        },
    )
}

fn latency_of(report: &Report) -> &evaluate::Latency {
    match question(report, "is_urgent") {
        QuestionReport::Noul {
            latency: Some(latency),
            ..
        } => latency,
        _ => panic!("is_urgent has a latency"),
    }
}

#[test]
fn counts_every_prefix_as_the_case_it_is() {
    let report = latency_report();
    assert_eq!(report.cases, 16);
    assert_eq!(
        report.errors,
        vec![CaseError {
            case: 5,
            turn: Some(2),
            id: Some("broken".to_owned()),
            message: "Timeout".to_owned(),
        }]
    );
    assert_eq!(question(&report, "is_urgent").cases(), 15);
}

#[test]
fn finds_the_first_turn_at_the_threshold_and_how_far_off_it_was() {
    let report = latency_report();
    let latency = latency_of(&report);
    assert_eq!(
        (
            latency.threads,
            latency.on_time,
            latency.early,
            latency.late,
            latency.missed,
            latency.false_alarms,
            latency.mean
        ),
        (4, 1, 1, 0, 1, 1, Some(-0.5))
    );
    let thread = |case, id: &str, expected, detected, lag| evaluate::ThreadLatency {
        case,
        id: Some(id.to_owned()),
        expected,
        detected,
        latency: lag,
    };
    assert_eq!(
        latency.cases,
        vec![
            thread(1, "on-time", Some(2), Some(2), Some(0)),
            thread(2, "early", Some(3), Some(2), Some(-1)),
            thread(3, "missed", Some(3), None, None),
            thread(4, "quiet", None, Some(2), None),
        ]
    );
}

#[test]
fn prints_it_under_the_sweep_and_in_the_json() {
    let report = latency_report();
    let text = evaluate::report_text(&report);
    assert!(
        text.contains(
            "    by turn  4 threads · 1 on time · 1 early · 0 late · 1 missed · 1 false alarm · mean latency -0.50 turns"
        ),
        "{text}"
    );
    assert!(text.contains("case 5 turn 2 (broken): Timeout"), "{text}");
    let json = evaluate::report_json(&report);
    let latency = &json["questions"]["is_urgent"]["latency"];
    assert_eq!(latency["threads"], 4);
    assert_eq!(latency["falseAlarms"], 1);
    assert_eq!(latency["mean"], -0.5);
    assert_eq!(
        latency["cases"][3],
        json!({"case": 4, "id": "quiet", "expected": null, "detected": 2, "latency": null})
    );
    let keys: Vec<&String> = json["questions"]["is_urgent"]
        .as_object()
        .expect("an object")
        .keys()
        .collect();
    assert_eq!(keys.last().map(|k| k.as_str()), Some("latency"));
    assert_eq!(
        json["errors"][0],
        json!({"case": 5, "turn": 2, "id": "broken", "message": "Timeout"})
    );
}

#[test]
fn leaves_latency_out_when_nothing_was_labelled_per_turn() {
    let report = urgent_report([0.9, 0.9, 0.9, 0.9], 0.5);
    assert!(matches!(
        question(&report, "is_urgent"),
        QuestionReport::Noul { latency: None, .. }
    ));
    assert!(
        evaluate::report_json(&report)["questions"]["is_urgent"]
            .get("latency")
            .is_none()
    );
}

#[test]
fn pairs_prefixes_by_line_and_turn_when_two_pages_are_compared() {
    let (a, b) = (session(), session_b());
    let (left, right) =
        evaluate::parse_compare_cases(&latency_lines()[0], &a, &b, LABELS).expect("they parse");
    let urgent = |ps: [f64; 3]| -> Vec<Outcome> {
        ps.iter()
            .map(|p| answered(&[("is_urgent", noul(*p))], None))
            .collect()
    };
    let (outcomes_a, outcomes_b) = (urgent([0.1, 0.8, 0.9]), urgent([0.1, 0.3, 0.9]));
    let comparison = evaluate::compare(
        evaluate::Side {
            label: "a",
            session: &a,
            cases: &left,
            outcomes: &outcomes_a,
            model: "m",
        },
        evaluate::Side {
            label: "b",
            session: &b,
            cases: &right,
            outcomes: &outcomes_b,
            model: "m",
        },
        evaluate::CompareOptions {
            threshold: 0.5,
            rates: None,
        },
    );
    assert_eq!(comparison.cases, 3);
    assert_eq!(
        comparison.questions[0].flips,
        vec![evaluate::Flip {
            case: 1,
            turn: Some(2),
            id: Some("on-time".to_owned()),
            expected: json!(true),
            a: json!(true),
            b: json!(false),
            status: "broke",
        }]
    );
    assert_eq!(
        evaluate::compare_json(&comparison)["questions"]["is_urgent"]["flips"][0],
        json!({"case": 1, "turn": 2, "id": "on-time", "expected": true, "a": true, "b": false, "status": "broke"})
    );
    assert!(evaluate::compare_text(&comparison).contains("case 1 turn 2 (on-time)"));
}
