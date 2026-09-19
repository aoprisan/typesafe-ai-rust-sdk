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
