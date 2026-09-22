//! The binary itself: the one-shot commands, what they print, and what they exit with.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const JEV: &str = env!("CARGO_BIN_EXE_jev");

const PAGE: &str = "The payout failed again, third time this month.
---
is_urgent? The message conveys urgency
  yes: A deadline or money being lost now
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
";

/// What the CLI did: exit status included, because the one-shot commands are meant for scripts.
struct Attempt {
    status: i32,
    stdout: String,
    stderr: String,
}

fn jev(args: &[&str], input: &str, env: &[(&str, &str)]) -> Attempt {
    let mut command = Command::new(JEV);
    command
        .args(args)
        .env("TYPESAFE_API_KEY", "")
        .env("JEV_PRICE", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("jev starts");
    child
        .stdin
        .take()
        .expect("stdin is a pipe")
        .write_all(input.as_bytes())
        .expect("the page goes in");
    let out = child.wait_with_output().expect("jev finishes");
    Attempt {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn reports_its_version() {
    let out = jev(&["--version"], "", &[]);
    assert_eq!(out.status, 0);
    assert!(
        out.stdout.trim().starts_with(char::is_numeric),
        "{}",
        out.stdout
    );
}

#[test]
fn lists_the_commands_in_help() {
    let help = jev(&["--help"], "", &[]).stdout;
    for command in ["run", "json", "cost", "rust", "check"] {
        assert!(help.contains(command), "{help}");
    }
    assert!(help.contains("--state"), "{help}");
    assert!(help.contains("--turn"), "{help}");
}

#[test]
fn checks_a_page_and_says_what_it_parsed_into() {
    let out = jev(&["check"], PAGE, &[]);
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stdout.contains("2 questions"), "{}", out.stdout);
    assert!(out.stdout.contains("is_urgent (noul)"), "{}", out.stdout);
}

#[test]
fn fails_the_check_on_a_broken_page() {
    let out = jev(&["check"], "a state\n---\nbroken?\n", &[]);
    assert_eq!(out.status, 1);
    assert!(out.stderr.contains("line 3"), "{}", out.stderr);
}

#[test]
fn prints_the_request_body_and_takes_it_back_in() {
    let body = jev(&["json"], PAGE, &[]);
    assert_eq!(body.status, 0, "{}", body.stderr);
    let parsed: Value = serde_json::from_str(&body.stdout).expect("valid JSON");
    assert_eq!(parsed["model"], "jev-latest");
    assert_eq!(jev(&["json"], &body.stdout, &[]).stdout, body.stdout);
}

#[test]
fn reads_a_file_as_well_as_stdin() {
    let dir = std::env::temp_dir().join(format!("jev-page-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let path = dir.join("triage.jev");
    std::fs::write(&path, PAGE).expect("the page is written");
    let from_file = jev(&["json", path.to_str().expect("a path")], "", &[]);
    assert_eq!(from_file.stdout, jev(&["json"], PAGE, &[]).stdout);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn answers_offline_without_a_key_and_without_a_terminal() {
    let out = jev(&["run"], PAGE, &[]);
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stdout.contains("is_urgent"), "{}", out.stdout);
    assert!(out.stdout.contains("department"), "{}", out.stdout);
    assert!(out.stderr.contains("Simulated answers"), "{}", out.stderr);
}

#[test]
fn prints_a_raw_body_for_json() {
    let out = jev(&["run", "--json"], PAGE, &[]);
    assert_eq!(out.status, 0, "{}", out.stderr);
    let parsed: Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    assert_eq!(parsed["answers"]["is_urgent"]["type"], "noul");
}

#[test]
fn takes_the_state_off_the_command_line() {
    let out = jev(&["json", "--state", "All fine, thanks!"], PAGE, &[]);
    let parsed: Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    assert_eq!(parsed["state"], "All fine, thanks!");
}

#[test]
fn appends_turns_to_the_state_in_the_order_they_were_given() {
    let out = jev(
        &[
            "json",
            "--turn",
            "agent: We are looking into it.",
            "--turn",
            "customer: Refund me.",
        ],
        PAGE,
        &[],
    );
    let parsed: Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    let turns = parsed["state"].as_array().expect("a conversation");
    assert_eq!(turns.len(), 3);
    assert!(turns[0]["said"].is_string(), "{}", out.stdout);
    assert_eq!(
        turns[1],
        json!({"who": "agent", "said": "We are looking into it."})
    );
    assert_eq!(turns[2], json!({"who": "customer", "said": "Refund me."}));
}

#[test]
fn refuses_a_turn_on_a_state_that_is_not_a_conversation() {
    let body =
        r#"{"state": {"ticket": 1}, "questions": {"a": {"type": "noul", "instructions": "x"}}}"#;
    let out = jev(&["json", "--turn", "agent: hello"], body, &[]);
    assert_eq!(out.status, 2);
    assert!(out.stderr.contains("not a conversation"), "{}", out.stderr);
}

#[test]
fn counts_the_thread_in_the_cost_table() {
    let out = jev(
        &[
            "cost",
            "--turn",
            "agent: We are looking into it.",
            "--price",
            "0.20/1.00",
        ],
        PAGE,
        &[],
    );
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stdout.contains("2 turns"), "{}", out.stdout);
    assert!(
        out.stdout.contains("asked after every turn: 2 calls"),
        "{}",
        out.stdout
    );
}

#[test]
fn prices_the_cost_table() {
    let out = jev(&["cost", "--price", "0.20/1.00"], PAGE, &[]);
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stdout.contains("per call"), "{}", out.stdout);
    assert!(
        out.stdout.contains("$0.20/$1.00 per Mtok"),
        "{}",
        out.stdout
    );
}

#[test]
fn takes_the_rates_from_the_environment_too() {
    let out = jev(&["cost"], PAGE, &[("JEV_PRICE", "0.20/1.00")]);
    assert!(out.stdout.contains("per call"), "{}", out.stdout);
}

#[test]
fn generates_the_session_as_code() {
    assert!(jev(&["rust"], PAGE, &[]).stdout.contains("typesafe"));
}

#[test]
fn refuses_to_send_a_request_with_no_state() {
    let out = jev(&["run"], "---\nis_urgent? conveys urgency\n", &[]);
    assert_eq!(out.status, 1);
    assert!(out.stderr.contains("No state"), "{}", out.stderr);
}

#[test]
fn exits_2_on_a_command_line_it_cannot_parse() {
    assert_eq!(jev(&["run", "--nope"], PAGE, &[]).status, 2);
    assert_eq!(jev(&["run", "--threshold", "5"], PAGE, &[]).status, 2);
    assert_eq!(jev(&["run", "--state"], PAGE, &[]).status, 2);
    assert_eq!(jev(&["frobnicate"], "", &[]).status, 2);
}

#[test]
fn exits_1_when_the_file_is_not_there() {
    let out = jev(&["json", "/no/such/page.jev"], "", &[]);
    assert_eq!(out.status, 1);
    assert!(out.stderr.contains("could not read"), "{}", out.stderr);
}

#[test]
fn points_at_the_one_shot_commands_when_the_repl_has_no_terminal() {
    let out = jev(&[], "", &[]);
    assert_eq!(out.status, 1);
    assert!(out.stderr.contains("jev run"), "{}", out.stderr);
}

/// The live path, answered from a server in this process: spawned asynchronously, because a
/// blocking wait would hold the runtime shut and the request could never be served.
async fn live(server: &MockServer, args: &[&str]) -> Attempt {
    let mut child = tokio::process::Command::new(JEV)
        .args(args)
        .env("TYPESAFE_API_KEY", "sk-test")
        .env("TYPESAFE_BASE_URL", server.uri())
        .env("JEV_PRICE", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("jev starts");
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().expect("stdin is a pipe");
        stdin.write_all(PAGE.as_bytes()).await.expect("the page");
        stdin.shutdown().await.ok();
    }
    let out = child.wait_with_output().await.expect("jev finishes");
    Attempt {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

async fn answering(status: u16, body: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn prints_the_answers_the_api_sent_and_the_usage_it_counted() {
    let server = answering(
        200,
        json!({
            "model": "jev-1",
            "answers": {"is_urgent": {"type": "noul", "noul": 0.91}},
            "usage": {"input_tokens": 120, "output_tokens": 30}
        }),
    )
    .await;
    let out = live(&server, &["run", "--price", "0.20/1.00"]).await;
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stdout.contains("0.91"), "{}", out.stdout);
    assert!(out.stdout.contains("120 in / 30 out"), "{}", out.stdout);
    assert!(out.stdout.contains('$'), "{}", out.stdout);
    // The question the server skipped is named, not silently dropped.
    assert!(
        out.stdout.contains("department: no answer came back"),
        "{}",
        out.stdout
    );
}

#[tokio::test]
async fn hands_json_the_body_the_api_actually_sent() {
    let server = answering(200, json!({"model": "jev-1", "answers": {}, "usage": {}})).await;
    let out = live(&server, &["run", "--json"]).await;
    assert_eq!(out.status, 0, "{}", out.stderr);
    let parsed: Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    assert_eq!(parsed["model"], "jev-1");
}

#[tokio::test]
async fn exits_1_and_explains_itself_when_the_api_says_no() {
    let server = answering(401, json!({"error": {"message": "bad key"}})).await;
    let out = live(&server, &["run"]).await;
    assert_eq!(out.status, 1);
    assert!(
        out.stderr.to_lowercase().contains("bad key")
            || out.stderr.to_lowercase().contains("authentication"),
        "{}",
        out.stderr
    );
}

#[tokio::test]
async fn mock_stays_offline_even_with_a_key_set() {
    let server = answering(500, json!({"error": {"message": "boom"}})).await;
    let out = live(&server, &["run", "--mock"]).await;
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert!(out.stderr.contains("Simulated answers"), "{}", out.stderr);
}

const EVAL_PAGE: &str = "placeholder
---
is_urgent? The message conveys urgency
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales = Pricing and plans
frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry
";

const EVAL_CASES: &str = concat!(
    r#"{"id": "t-001", "state": "Stripe has been failing for 3 days", "expect": {"is_urgent": true, "department": "technical", "frustration": 2}}"#,
    "\n",
    r#"{"state": "Can I get a copy of last month's invoice?", "expect": {"department": "billing", "is_urgent": false}}"#,
    "\n",
    r#"{"state": "What does the enterprise plan include?", "expect": {"department": "sales", "frustration": 0}}"#,
);

/// A directory of this test's own, so the pages and cases files do not collide.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jev-eval-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

/// A page and a cases file on disk, for the duration of one test.
fn with_files(name: &str, cases: &str, run: impl FnOnce(&str, &str)) {
    let dir = scratch(name);
    let page = dir.join("triage.jev");
    let path = dir.join("cases.jsonl");
    std::fs::write(&page, EVAL_PAGE).expect("the page is written");
    std::fs::write(&path, cases).expect("the cases are written");
    run(
        page.to_str().expect("a path"),
        path.to_str().expect("a path"),
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reports_every_question_it_was_given_labels_for() {
    with_files("reports", EVAL_CASES, |page, cases| {
        let out = jev(&["eval", page, "--cases", cases, "--mock"], "", &[]);
        assert_eq!(out.status, 0, "{}", out.stderr);
        for (name, kind) in [
            ("is_urgent", "noul"),
            ("department", "choice"),
            ("frustration", "score"),
        ] {
            let line = out
                .stdout
                .lines()
                .find(|line| line.contains(name))
                .unwrap_or_else(|| panic!("no line for {name} in\n{}", out.stdout));
            assert!(line.contains(kind), "{line}");
        }
        assert!(out.stdout.contains("best f1 at"), "{}", out.stdout);
        assert!(
            out.stdout.contains("3 cases · 3 answered · 0 errors"),
            "{}",
            out.stdout
        );
        assert!(out.stderr.contains("Simulated answers"), "{}", out.stderr);
    });
}

#[test]
fn prints_the_whole_report_as_json_for_a_script_to_read() {
    with_files("json", EVAL_CASES, |page, cases| {
        let out = jev(
            &["eval", page, "--cases", cases, "--mock", "--json"],
            "",
            &[],
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        let report: Value = serde_json::from_str(&out.stdout).expect("valid JSON");
        assert_eq!(report["cases"], 3);
        let questions: Vec<&String> = report["questions"]
            .as_object()
            .expect("the questions")
            .keys()
            .collect();
        assert_eq!(questions, ["is_urgent", "department", "frustration"]);
    });
}

#[test]
fn refuses_a_cases_file_with_a_bad_line_and_says_which_line() {
    let broken = format!("{EVAL_CASES}\n{{\"state\": \"no expectations\"}}");
    with_files("broken", &broken, |page, cases| {
        let out = jev(&["eval", page, "--cases", cases, "--mock"], "", &[]);
        assert_eq!(out.status, 1);
        assert!(out.stderr.contains("cases line 4:"), "{}", out.stderr);
    });
}

#[test]
fn exits_2_on_a_command_line_eval_cannot_use() {
    with_files("usage", EVAL_CASES, |page, cases| {
        let state = jev(
            &["eval", page, "--cases", cases, "--mock", "--state", "hello"],
            "",
            &[],
        );
        assert_eq!(state.status, 2);
        assert!(
            state
                .stderr
                .contains("--state does not apply to eval: the cases carry the states."),
            "{}",
            state.stderr
        );

        let turn = jev(
            &["eval", page, "--cases", cases, "--mock", "--turn", "a: b"],
            "",
            &[],
        );
        assert_eq!(turn.status, 2);
        assert!(
            turn.stderr.contains("--turn does not apply to eval"),
            "{}",
            turn.stderr
        );

        let rateless = jev(
            &["eval", page, "--cases", cases, "--mock", "--max-cost", "1"],
            "",
            &[],
        );
        assert_eq!(rateless.status, 2);
        assert!(
            rateless
                .stderr
                .contains("--max-cost needs rates: pass --price <in>/<out> or set JEV_PRICE."),
            "{}",
            rateless.stderr
        );

        assert_eq!(
            jev(
                &["eval", page, "--cases", cases, "--concurrency", "0"],
                "",
                &[]
            )
            .status,
            2
        );
        let both = jev(&["eval", "--cases", "-", "--mock"], EVAL_PAGE, &[]);
        assert!(
            both.stderr
                .contains("the page and the cases cannot both come from stdin."),
            "{}",
            both.stderr
        );
        assert_eq!(jev(&["eval", page, "--mock"], "", &[]).status, 2);
    });
}

#[test]
fn exits_1_and_names_the_question_when_a_bar_is_not_met() {
    // The simulator is deterministic, so a case can be labelled with what it is bound to get wrong.
    let state = json!("A payout failed again.");
    let answer = jev_repl::mock::answer(
        &state,
        "is_urgent",
        &json!({"type": "noul", "instructions": "The message conveys urgency"}),
    );
    let yes = matches!(answer, Some(typesafe::Answer::Noul(a)) if a.is_yes(0.5));
    let cases = format!(
        r#"{{"state": {state}, "expect": {{"is_urgent": {}}}}}"#,
        !yes
    );
    with_files("bar", &cases, |page, path| {
        let out = jev(
            &[
                "eval",
                page,
                "--cases",
                path,
                "--mock",
                "--min-accuracy",
                "1",
            ],
            "",
            &[],
        );
        assert_eq!(out.status, 1);
        assert!(
            out.stderr
                .contains("jev eval: is_urgent accuracy 0.00 is below 1.00."),
            "{}",
            out.stderr
        );
    });
}

#[test]
fn lists_eval_and_its_flags_in_help() {
    let help = jev(&["--help"], "", &[]).stdout;
    assert!(help.contains("eval"), "{help}");
    for flag in [
        "--cases",
        "--concurrency",
        "--cache",
        "--max-cost",
        "--min-accuracy",
        "--compare",
        "--fail-on-regression",
    ] {
        assert!(help.contains(flag), "{help}");
    }
}

/// The candidate page: is_urgent asked another way, frustration dropped, sarcasm added.
const EVAL_PAGE_B: &str = "placeholder
---
is_urgent? The message conveys urgency or time pressure
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales = Pricing and plans
sarcasm? The customer is being sarcastic
";

/// A directory holding two pages and a cases file, for the duration of one test.
fn with_pages(name: &str, cases: &str, run: impl FnOnce(&str, &str, &str)) {
    let dir = scratch(name);
    let (a, b, path) = (
        dir.join("a.jev"),
        dir.join("b.jev"),
        dir.join("cases.jsonl"),
    );
    std::fs::write(&a, EVAL_PAGE).expect("page a is written");
    std::fs::write(&b, EVAL_PAGE_B).expect("page b is written");
    std::fs::write(&path, cases).expect("the cases are written");
    run(
        a.to_str().expect("a path"),
        b.to_str().expect("a path"),
        path.to_str().expect("a path"),
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Whether some line of `text` holds `parts` in order with only spaces between them.
fn has_row(text: &str, parts: &[&str]) -> bool {
    let wanted: Vec<&str> = parts.iter().flat_map(|p| p.split_whitespace()).collect();
    text.lines().any(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.windows(wanted.len()).any(|w| w == wanted.as_slice())
    })
}

#[test]
fn reports_the_difference_per_shared_question_and_what_only_one_page_asks() {
    with_pages("compare", EVAL_CASES, |a, b, cases| {
        let out = jev(
            &["eval", a, "--compare", b, "--cases", cases, "--mock"],
            "",
            &[],
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(
            has_row(&out.stdout, &["is_urgent", "noul", "2 paired cases"]),
            "{}",
            out.stdout
        );
        assert!(
            has_row(&out.stdout, &["department", "choice", "3 paired cases"]),
            "{}",
            out.stdout
        );
        for wanted in [
            "McNemar",
            "only in a: frustration",
            "only in b: sarcasm",
            "3 cases · a 3 answered, 0 errors · b 3 answered, 0 errors",
        ] {
            assert!(out.stdout.contains(wanted), "{wanted:?} in\n{}", out.stdout);
        }
        assert!(out.stderr.contains("Simulated answers"), "{}", out.stderr);
    });
}

#[test]
fn prints_both_reports_and_the_comparison_as_json() {
    with_pages("compare-json", EVAL_CASES, |a, b, cases| {
        let out = jev(
            &[
                "eval",
                a,
                "--compare",
                b,
                "--cases",
                cases,
                "--mock",
                "--json",
            ],
            "",
            &[],
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        let report: Value = serde_json::from_str(&out.stdout).expect("a JSON report");
        assert_eq!(report["a"]["page"], a);
        assert_eq!(report["b"]["page"], b);
        let names: Vec<&String> = report["questions"]
            .as_object()
            .expect("questions")
            .keys()
            .collect();
        assert_eq!(names, vec!["is_urgent", "department"]);
        assert_eq!(report["onlyB"], json!(["sarcasm"]));
        assert_eq!(report["regressions"], json!([]));
    });
}

#[test]
fn names_the_page_a_label_does_not_fit() {
    let cases = r#"{"state": "x", "expect": {"frustration": 2, "sarcasm": 1}}"#;
    with_pages("compare-misfit", cases, |a, b, cases| {
        let out = jev(
            &["eval", a, "--compare", b, "--cases", cases, "--mock"],
            "",
            &[],
        );
        assert_eq!(out.status, 1);
        assert!(
            out.stderr
                .contains(&format!("cases line 1: {b}: sarcasm is a noul")),
            "{}",
            out.stderr
        );
    });
}

#[test]
fn exits_2_when_the_pages_cannot_be_told_apart_on_the_command_line() {
    with_pages("compare-usage", EVAL_CASES, |a, b, cases| {
        let both = jev(
            &["eval", "--compare", "-", "--cases", cases, "--mock"],
            "x",
            &[],
        );
        assert_eq!(both.status, 2);
        assert!(
            both.stderr
                .contains("only one of the page, --compare and --cases can come from stdin."),
            "{}",
            both.stderr
        );
        let lonely = jev(
            &[
                "eval",
                a,
                "--cases",
                cases,
                "--mock",
                "--fail-on-regression",
            ],
            "",
            &[],
        );
        assert_eq!(lonely.status, 2);
        assert!(
            lonely.stderr.contains(
                "--fail-on-regression needs --compare: there is nothing to regress from."
            ),
            "{}",
            lonely.stderr
        );
        assert_eq!(
            jev(&["eval", a, "--compare", b, "--mock"], "", &[]).status,
            2
        );
    });
}

/// What the simulator says about `state` for an is_urgent asked with `instructions`, at 0.5.
fn simulated_yes(instructions: &str, state: &str) -> bool {
    let question = json!({"type": "noul", "instructions": instructions});
    match jev_repl::mock::answer(&json!(state), "is_urgent", &question) {
        Some(typesafe::Answer::Noul(answer)) => answer.is_yes(0.5),
        _ => false,
    }
}

#[test]
fn fails_on_a_regression_the_test_can_see_and_only_when_asked_to() {
    // Label every state the way page a answers it, so page a is always right and every case page
    // b answers differently is one it broke. The simulator is deterministic, so this holds.
    let mut lines = Vec::new();
    let mut broke = 0;
    let mut i = 0;
    while broke < 8 {
        let state = format!("ticket {i}");
        let a = simulated_yes("The message conveys urgency", &state);
        if a != simulated_yes("The message conveys urgency or time pressure", &state) {
            broke += 1;
        }
        lines.push(json!({"state": state, "expect": {"is_urgent": a}}).to_string());
        i += 1;
    }
    with_pages("regression", &lines.join("\n"), |a, b, cases| {
        let args = ["eval", a, "--compare", b, "--cases", cases, "--mock"];
        let quiet = jev(&args, "", &[]);
        assert_eq!(quiet.status, 0, "{}", quiet.stderr);
        assert!(
            quiet.stdout.contains("0 fixed · 8 broke · 0 changed"),
            "{}",
            quiet.stdout
        );
        assert!(
            quiet.stdout.contains("b is significantly worse"),
            "{}",
            quiet.stdout
        );
        let strict = jev(&[&args[..], &["--fail-on-regression"]].concat(), "", &[]);
        assert_eq!(strict.status, 1);
        assert!(
            strict.stderr.contains(&format!(
                "jev eval: is_urgent is significantly worse in {b} (McNemar p 0.008)."
            )),
            "{}",
            strict.stderr
        );
    });
}

#[test]
fn holds_both_pages_to_min_accuracy_and_says_which_one_missed() {
    // States both pages answer alike, labelled the other way: both pages score 0.
    let mut lines = Vec::new();
    let mut i = 0;
    while lines.len() < 3 {
        let state = format!("ticket {i}");
        let a = simulated_yes("The message conveys urgency", &state);
        if a == simulated_yes("The message conveys urgency or time pressure", &state) {
            lines.push(json!({"state": state, "expect": {"is_urgent": !a}}).to_string());
        }
        i += 1;
    }
    with_pages("compare-bar", &lines.join("\n"), |a, b, cases| {
        let out = jev(
            &[
                "eval",
                a,
                "--compare",
                b,
                "--cases",
                cases,
                "--mock",
                "--min-accuracy",
                "0.5",
            ],
            "",
            &[],
        );
        assert_eq!(out.status, 1);
        for page in [a, b] {
            let wanted = format!("jev eval: {page}: is_urgent accuracy 0.00 is below 0.50.");
            assert!(
                out.stderr.contains(&wanted),
                "{wanted:?} in\n{}",
                out.stderr
            );
        }
    });
}

const PAGE_ONE: &str = "placeholder\n---\nis_urgent? The message conveys urgency\n";

const EVAL_LIVE_CASES: &str = concat!(
    r#"{"id": "u-1", "state": "urgent: the payout failed", "expect": {"is_urgent": true}}"#,
    "\n",
    r#"{"id": "u-2", "state": "urgent: checkout is down", "expect": {"is_urgent": true}}"#,
    "\n",
    r#"{"id": "c-1", "state": "a question about the plan", "expect": {"is_urgent": false}}"#,
    "\n",
    r#"{"id": "c-2", "state": "a note of thanks", "expect": {"is_urgent": false}}"#,
);

/// The fake API the live eval talks to: the answer is read off the state in the request body.
struct FromTheState;

impl wiremock::Respond for FromTheState {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let state = body["state"].as_str().unwrap_or_default().to_owned();
        if state.contains("refuse") {
            return ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"message": "this state is not allowed"}}));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1",
            "answers": {"is_urgent": {
                "type": "noul",
                "noul": if state.contains("urgent") { 0.9 } else { 0.1 },
            }},
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }))
    }
}

async fn answering_from_the_state() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(FromTheState)
        .mount(&server)
        .await;
    server
}

async fn asked(server: &MockServer) -> usize {
    server.received_requests().await.map_or(0, |all| all.len())
}

/// A run against the fake API, with the page on stdin and the cases in a file.
async fn live_eval(server: &MockServer, cases: &str, args: &[&str]) -> Attempt {
    let mut child = tokio::process::Command::new(JEV)
        .args(["eval", "--cases", cases])
        .args(args)
        .env("TYPESAFE_API_KEY", "sk-test")
        .env("TYPESAFE_BASE_URL", server.uri())
        .env("JEV_PRICE", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("jev starts");
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().expect("stdin is a pipe");
        stdin
            .write_all(PAGE_ONE.as_bytes())
            .await
            .expect("the page");
        stdin.shutdown().await.ok();
    }
    let out = child.wait_with_output().await.expect("jev finishes");
    Attempt {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The cases file for one live test, in a directory of its own.
fn live_cases(name: &str, cases: &str) -> std::path::PathBuf {
    let path = scratch(name).join("cases.jsonl");
    std::fs::write(&path, cases).expect("the cases are written");
    path
}

#[tokio::test]
async fn asks_once_per_case_and_counts_what_the_api_counted() {
    let server = answering_from_the_state().await;
    let cases = live_cases("live", EVAL_LIVE_CASES);
    let out = live_eval(&server, cases.to_str().expect("a path"), &[]).await;
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert_eq!(asked(&server).await, 4);
    let chosen = out
        .stdout
        .lines()
        .find(|line| line.contains("0.50 *"))
        .unwrap_or_else(|| panic!("no chosen row in\n{}", out.stdout));
    assert!(chosen.contains("1.00  1.00"), "{chosen}");
    assert!(
        out.stdout.contains("40 in / 20 out tokens"),
        "{}",
        out.stdout
    );
    assert!(
        out.stderr.contains("jev eval: 4 cases, ≈"),
        "{}",
        out.stderr
    );
}

#[tokio::test]
async fn answers_a_second_run_from_the_cache_without_sending_anything() {
    let server = answering_from_the_state().await;
    let dir = scratch("cache");
    let cases = live_cases("cache-cases", EVAL_LIVE_CASES);
    let cache = dir.to_str().expect("a path");
    let first = live_eval(
        &server,
        cases.to_str().expect("a path"),
        &["--cache", cache],
    )
    .await;
    assert_eq!(first.status, 0, "{}", first.stderr);
    assert_eq!(asked(&server).await, 4);
    let again = live_eval(
        &server,
        cases.to_str().expect("a path"),
        &["--cache", cache],
    )
    .await;
    assert_eq!(asked(&server).await, 4);
    assert_eq!(again.stdout, first.stdout);
}

/// The eval cache is the SDK's cassette format: a client replaying the cache directory answers
/// every request `jev eval` sent, byte for byte the same body, without a server or a key.
#[tokio::test]
async fn a_cache_directory_replays_through_the_sdk() {
    let server = answering_from_the_state().await;
    let dir = scratch("cassette");
    let cases = live_cases("cassette-cases", EVAL_LIVE_CASES);
    let out = live_eval(
        &server,
        cases.to_str().expect("a path"),
        &["--cache", dir.to_str().expect("a path")],
    )
    .await;
    assert_eq!(out.status, 0, "{}", out.stderr);

    let replaying = typesafe::Client::builder()
        .replay(&dir)
        .build()
        .expect("replaying needs no key");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 4);
    for request in requests {
        let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
        // Raw questions re-send the objects exactly as jev sent them.
        let questions: typesafe::Questions = body["questions"]
            .as_object()
            .expect("questions")
            .iter()
            .map(|(name, q)| (name.clone(), q.clone()))
            .collect();
        let replayed = replaying
            .system_one(body["state"].clone(), questions)
            .model(body["model"].as_str().expect("a model"))
            .await
            .expect("the cache holds this request");
        assert_eq!(replayed.meta.attempts, 0);
        let state = body["state"].as_str().expect("a text state");
        let urgent = replayed.noul("is_urgent").expect("answered").noul;
        assert_eq!(urgent >= 0.5, state.starts_with("urgent"), "{state}");
    }
    assert_eq!(asked(&server).await, 4);
}

#[tokio::test]
async fn refuses_to_send_when_the_estimate_is_above_max_cost() {
    let server = answering_from_the_state().await;
    let cases = live_cases("max-cost", EVAL_LIVE_CASES);
    let out = live_eval(
        &server,
        cases.to_str().expect("a path"),
        &["--max-cost", "0.000001", "--price", "0.20/1.00"],
    )
    .await;
    assert_eq!(out.status, 1);
    assert_eq!(asked(&server).await, 0);
    assert!(out.stderr.contains("refusing to send"), "{}", out.stderr);
}

#[tokio::test]
async fn compares_two_pages_over_one_pool_one_preflight_and_one_cache() {
    let server = answering_from_the_state().await;
    let dir = scratch("live-compare");
    let other = dir.join("b.jev");
    std::fs::write(
        &other,
        "placeholder\n---\nis_urgent? Something needs doing now\n",
    )
    .expect("page b is written");
    let cache = dir.join("cache");
    let cases = live_cases("live-compare-cases", EVAL_LIVE_CASES);
    let cases = cases.to_str().expect("a path");
    let (other, cache) = (
        other.to_str().expect("a path"),
        cache.to_str().expect("a path"),
    );
    let args = ["--compare", other, "--cache", cache, "--price", "0.20/1.00"];
    let first = live_eval(&server, cases, &args).await;
    assert_eq!(first.status, 0, "{}", first.stderr);
    assert_eq!(asked(&server).await, 8);
    assert!(
        first
            .stderr
            .contains("jev eval: 4 + 4 cases over two pages, ≈"),
        "{}",
        first.stderr
    );
    assert!(
        first.stderr.contains("at $0.20/$1.00 per Mtok"),
        "{}",
        first.stderr
    );
    assert!(
        first.stdout.contains("80 in / 40 out tokens"),
        "{}",
        first.stdout
    );
    assert!(
        first
            .stdout
            .contains("McNemar: no discordant pairs, nothing to test"),
        "{}",
        first.stdout
    );
    let again = live_eval(&server, cases, &args).await;
    assert_eq!(asked(&server).await, 8);
    assert_eq!(again.stdout, first.stdout);
    let refused = live_eval(
        &server,
        cases,
        &[
            "--compare",
            other,
            "--max-cost",
            "0.000001",
            "--price",
            "0.20/1.00",
        ],
    )
    .await;
    assert_eq!(refused.status, 1);
    assert!(
        refused.stderr.contains("refusing to send"),
        "{}",
        refused.stderr
    );
    assert_eq!(asked(&server).await, 8);
}

#[tokio::test]
async fn carries_on_when_one_case_is_refused_and_says_which() {
    let server = answering_from_the_state().await;
    let cases = live_cases(
        "refused",
        &format!(
            "{EVAL_LIVE_CASES}\n{}",
            r#"{"id": "bad", "state": "refuse this one", "expect": {"is_urgent": true}}"#
        ),
    );
    let out = live_eval(&server, cases.to_str().expect("a path"), &[]).await;
    assert_eq!(out.status, 1);
    assert!(out.stdout.contains("case 5 (bad):"), "{}", out.stdout);
    assert!(
        out.stdout.contains("5 cases · 4 answered · 1 error"),
        "{}",
        out.stdout
    );
}
