//! jev with no terminal: a page in, one answer out.

use jev_repl::cost::{self, Rates};
use jev_repl::headless;
use jev_repl::session::{self, Session};
use serde_json::{Value, json};
use typesafe::Usage;

const PAGE: &str = "The payout failed again, third time this month.
---
is_urgent? The message conveys urgency
  yes: A deadline or money being lost now
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry
";

fn page() -> Session {
    headless::load(PAGE).expect("the page parses")
}

fn rates(text: &str) -> Rates {
    cost::parse_rates(text).expect("the rates parse")
}

#[test]
fn reads_a_sketch_page() {
    let session = page();
    let names: Vec<&str> = session
        .questions
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["is_urgent", "department", "frustration"]);
    assert!(session.state_preview().contains("payout failed"));
}

#[test]
fn reads_a_request_body_back_in() {
    let body = page().request_json("jev-latest");
    let session = headless::load(&body).expect("a body reads");
    assert_eq!(session.request_json("jev-latest"), body);
}

#[test]
fn tells_a_page_from_a_body_by_the_text() {
    let session = headless::load("  {\"state\": \"hi\", \"questions\": {}}").expect("a body");
    assert_eq!(session.state, Value::String("hi".to_owned()));
}

#[test]
fn refuses_an_empty_input() {
    let error = headless::load("   \n  ").expect_err("nothing to read");
    assert!(error.contains("empty"), "{error}");
}

#[test]
fn points_at_the_line_a_page_went_wrong_on() {
    let error = headless::load("a state\n---\nbroken?\n").expect_err("a broken page");
    assert!(error.starts_with("line 3: "), "{error}");
}

#[test]
fn check_says_what_the_page_parsed_into() {
    let summary = headless::check_text(PAGE).expect("the page parses");
    assert!(summary.contains("3 questions"), "{summary}");
    assert!(summary.contains("is_urgent (noul)"), "{summary}");
    assert!(summary.contains("frustration (score)"), "{summary}");
}

#[test]
fn check_reports_every_problem() {
    let problems =
        headless::check_text("a state\n---\nbroken?\nalso_broken?\n").expect_err("two problems");
    assert_eq!(problems.lines().count(), 2, "{problems}");
}

#[test]
fn check_warns_that_a_page_with_no_state_cannot_be_sent() {
    let summary =
        headless::check_text("---\nis_urgent? conveys urgency\n").expect("the page parses");
    assert!(summary.contains("no state"), "{summary}");
}

#[test]
fn will_not_send_a_request_that_asks_nothing() {
    let session = Session {
        state: Value::String("hi".to_owned()),
        questions: Vec::new(),
        model: None,
        bars: Vec::new(),
    };
    assert!(
        headless::sendable(&session)
            .expect("nothing to ask")
            .contains("No questions")
    );
}

#[test]
fn will_not_send_a_request_with_nothing_to_judge() {
    let entry = session::parse_noul("is_urgent conveys urgency").expect("a noul");
    let session = Session {
        state: Value::String(String::new()),
        questions: vec![entry],
        model: None,
        bars: Vec::new(),
    };
    assert!(
        headless::sendable(&session)
            .expect("nothing to judge")
            .contains("No state")
    );
}

#[test]
fn is_happy_with_a_state_and_a_question() {
    assert!(headless::sendable(&page()).is_none());
}

#[test]
fn prints_the_request_body_newline_terminated() {
    let text = headless::request_text(&page(), "jev-latest");
    assert!(text.ends_with("}\n"), "{text}");
    let body: Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(body["model"], "jev-latest");
}

#[test]
fn prints_the_cost_table_and_how_to_price_it() {
    let text = headless::cost_text(&page(), "jev-latest", None);
    assert!(text.contains("total"), "{text}");
    assert!(text.contains("--price 0.20/1.00"), "{text}");
}

#[test]
fn prices_the_table_when_rates_are_given() {
    let text = headless::cost_text(&page(), "jev-latest", Some(rates("0.20/1.00")));
    assert!(text.contains("per call"), "{text}");
    assert!(text.contains("$0.20/$1.00 per Mtok"), "{text}");
}

#[test]
fn generates_the_session_as_code() {
    let code = headless::code_text(&page(), "jev-latest", 0.5);
    assert!(code.contains("typesafe"), "{code}");
    assert!(code.ends_with('\n'));
}

#[test]
fn simulates_one_answer_per_question_deterministically() {
    let session = page();
    let once = headless::mock_answers(&session);
    let twice = headless::mock_answers(&session);
    let names: Vec<&str> = once.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["is_urgent", "department", "frustration"]);
    assert_eq!(
        headless::answers_json(&once, "jev-latest", None),
        headless::answers_json(&twice, "jev-latest", None),
    );
}

#[test]
fn renders_the_answer_page_without_the_colour() {
    let text = headless::answers_text(&headless::mock_answers(&page()), 0.5);
    assert!(text.contains("is_urgent"), "{text}");
    assert!(text.contains("threshold 0.50"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
}

#[test]
fn names_a_question_that_went_unanswered() {
    let text = headless::answers_text(&[("mystery".to_owned(), None)], 0.5);
    assert!(text.contains("mystery: no answer came back"), "{text}");
}

#[test]
fn prints_a_raw_body_that_parses() {
    let answers = headless::mock_answers(&page());
    let body = headless::answers_json(&answers, "jev-latest", None);
    let parsed: Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["model"], "jev-latest");
}

#[test]
fn passes_a_live_body_through_untouched() {
    let raw = json!({"model": "jev-1", "answers": {}});
    let body = headless::answers_json(&[], "jev-latest", Some(&raw));
    let parsed: Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["model"], "jev-1");
}

#[test]
fn prefers_the_counted_usage_over_the_estimate() {
    // `Usage` is non-exhaustive, so it is built the way a response builds it: from the wire shape.
    let usage: Usage =
        serde_json::from_value(json!({"input_tokens": 120, "output_tokens": 30})).unwrap();
    let text = headless::usage_text(
        &page(),
        "jev-latest",
        Some(rates("0.20/1.00")),
        Some(&usage),
    );
    assert!(text.contains("120 in / 30 out"), "{text}");
    assert!(text.contains('$'), "{text}");
    assert!(!text.contains('≈'), "{text}");
}

#[test]
fn falls_back_to_the_estimate_and_says_so() {
    let text = headless::usage_text(&page(), "jev-latest", None, None);
    assert!(text.contains('≈'), "{text}");
    assert!(text.contains("estimated"), "{text}");
}

#[test]
fn knows_its_own_subcommands() {
    assert!(headless::is_command("run"));
    assert!(headless::is_command("check"));
    assert!(!headless::is_command("triage.jev"));
    assert!(!headless::is_command("--help"));
}

#[test]
fn check_says_when_the_state_is_a_conversation() {
    let page =
        "[{\"who\": \"customer\", \"said\": \"Refund me\"}]\n---\nis_urgent? conveys urgency\n";
    let summary = headless::check_text(page).expect("the page parses");
    assert!(summary.contains("conversation of 1 turn"), "{summary}");
}
