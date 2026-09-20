//! Scoring a rubric: the cases file, the metrics, the runner and the two reports.

use jev_repl::evaluate::{self, Expectation};
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
