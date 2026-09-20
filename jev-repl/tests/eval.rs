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
        vec![("is_urgent".to_owned(), Expectation::Noul { yes: true })]
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
                case: 1,
                id: Some("t-001".to_owned()),
                message: "Timeout  the request did not complete".to_owned(),
            },
            CaseError {
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
    assert_eq!(urgent["best"], json!({"threshold": 0.5, "f1": 1.0}));
    assert_eq!(
        json["questions"]["department"]["labels"],
        json!(["billing", "technical", "sales", "other"])
    );
    assert_eq!(
        json["usage"],
        json!({"inputTokens": 200, "outputTokens": 40, "estimated": false, "cost": 0.00008})
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
