//! The agent side: the MCP protocol, where an install puts its files, and the skill the two ship.
//!
//! The protocol half is pure, so it is driven here as messages in and messages out — no pipe, no
//! child process — and the tests at the bottom check that the same handler is on the binary.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::{Value, json};

use jev_repl::evaluate::Outcome;
use jev_repl::headless;
use jev_repl::install::{self, Kind, Scope, ServerEntry};
use jev_repl::mcp::{self, Ask, Host, Sent};
use jev_repl::session::Session;
use jev_repl::skill::{SKILL_MD, SKILL_NAME};

const JEV: &str = env!("CARGO_BIN_EXE_jev");

const PAGE: &str = "A payout failed again, third time this month.
---
is_urgent? The message conveys urgency
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
";

/// A host that answers offline, the way `jev mcp` does without a key.
fn offline() -> Host {
    Host {
        version: "9.9.9".to_owned(),
        model: "jev-test".to_owned(),
        live: false,
        rates: None,
        ask: mock_ask(),
    }
}

fn mock_ask() -> Ask {
    Arc::new(|session: Session| {
        Box::pin(async move {
            Sent {
                outcome: Outcome::Ok {
                    answers: headless::mock_answers(&session),
                    usage: None,
                },
                raw: None,
            }
        })
    })
}

async fn request(method: &str, params: Value, host: &Host) -> Value {
    let message = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    mcp::handle(&message, host)
        .await
        .expect("a request with an id is answered")
}

/// The `result` of a reply, or a failure naming what came back instead.
fn result_of(reply: &Value) -> &Value {
    reply
        .get("result")
        .unwrap_or_else(|| panic!("expected a result, got {reply}"))
}

/// The text a tool call printed, and whether it was an error.
fn tool_text(reply: &Value) -> (String, bool) {
    let result = result_of(reply);
    let text = result["content"]
        .as_array()
        .expect("content is a list")
        .iter()
        .map(|block| block["text"].as_str().unwrap_or_default())
        .collect::<String>();
    (text, result.get("isError") == Some(&Value::Bool(true)))
}

async fn call_tool(name: &str, args: Value, host: &Host) -> (String, bool) {
    let reply = request(
        "tools/call",
        json!({ "name": name, "arguments": args }),
        host,
    )
    .await;
    tool_text(&reply)
}

#[tokio::test]
async fn answers_initialize_with_the_protocol_the_client_asked_for() {
    let reply = request(
        "initialize",
        json!({ "protocolVersion": "2024-11-05", "capabilities": {} }),
        &offline(),
    )
    .await;
    let result = result_of(&reply);
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(
        result["capabilities"],
        json!({ "tools": { "listChanged": false } })
    );
    assert_eq!(result["serverInfo"]["name"], "jev");
    assert_eq!(result["serverInfo"]["version"], "9.9.9");
}

#[tokio::test]
async fn falls_back_to_its_own_protocol() {
    let reply = request(
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
        &offline(),
    )
    .await;
    assert_eq!(result_of(&reply)["protocolVersion"], mcp::PROTOCOL_VERSION);
}

#[tokio::test]
async fn says_when_every_answer_will_be_simulated() {
    let reply = request("initialize", json!({}), &offline()).await;
    let instructions = result_of(&reply)["instructions"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(instructions.contains("no API key"), "{instructions}");

    let live = Host {
        live: true,
        ..offline()
    };
    let reply = request("initialize", json!({}), &live).await;
    let instructions = result_of(&reply)["instructions"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!instructions.contains("no API key"), "{instructions}");
}

#[tokio::test]
async fn answers_a_notification_with_nothing_at_all() {
    let message = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(mcp::handle(&message, &offline()).await.is_none());
}

#[tokio::test]
async fn answers_ping_and_refuses_a_method_it_does_not_have() {
    let reply = request("ping", json!({}), &offline()).await;
    assert_eq!(result_of(&reply), &json!({}));

    let reply = request("sampling/createMessage", json!({}), &offline()).await;
    assert_eq!(reply["error"]["code"], mcp::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn reports_a_line_that_is_not_json_as_a_parse_error() {
    let line = mcp::handle_line("{not json", &offline())
        .await
        .expect("a bad line is answered");
    let reply: Value = serde_json::from_str(&line).expect("the answer is JSON");
    assert_eq!(reply["error"]["code"], mcp::PARSE_ERROR);
    assert!(mcp::handle_line("   ", &offline()).await.is_none());
}

#[tokio::test]
async fn lists_every_tool_with_a_schema() {
    let reply = request("tools/list", json!({}), &offline()).await;
    let tools = result_of(&reply)["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "jev_notation",
            "jev_check",
            "jev_request",
            "jev_cost",
            "jev_ask",
            "jev_eval",
            "jev_code",
            "jev_presets",
        ]
    );
    for tool in &tools {
        assert!(tool["description"].as_str().unwrap().len() > 20, "{tool}");
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

#[tokio::test]
async fn hands_over_the_notation_which_is_the_skill() {
    let (text, _) = call_tool("jev_notation", json!({}), &offline()).await;
    assert_eq!(text, SKILL_MD);
}

#[tokio::test]
async fn checks_a_page_and_says_what_it_parsed_into() {
    let (text, is_error) = call_tool("jev_check", json!({ "page": PAGE }), &offline()).await;
    assert!(!is_error, "{text}");
    assert!(text.contains("is_urgent (noul)"), "{text}");
    assert!(text.contains("department (choice)"), "{text}");
}

#[tokio::test]
async fn reports_a_broken_page_with_its_line_number() {
    let page = json!({ "page": "state\n---\nbroken? \n" });
    let (text, is_error) = call_tool("jev_check", page, &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("line 3"), "{text}");
}

#[tokio::test]
async fn prints_the_request_body_and_takes_the_model_off_the_page() {
    let args = json!({ "page": PAGE, "model": "jev-2" });
    let (text, _) = call_tool("jev_request", args, &offline()).await;
    let body: Value = serde_json::from_str(&text).expect("a request body");
    assert_eq!(body["model"], "jev-2");
    let names: Vec<&str> = body["questions"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["is_urgent", "department"]);

    let pinned = PAGE.replace("---\n", "---\n@model pinned\n");
    let (text, _) = call_tool("jev_request", json!({ "page": pinned }), &offline()).await;
    let body: Value = serde_json::from_str(&text).expect("a request body");
    assert_eq!(body["model"], "pinned");
}

#[tokio::test]
async fn prices_a_page_and_explains_bad_rates() {
    let args = json!({ "page": PAGE, "price": "0.20/1.00" });
    let (text, is_error) = call_tool("jev_cost", args, &offline()).await;
    assert!(!is_error, "{text}");
    assert!(text.contains('$'), "{text}");

    let args = json!({ "page": PAGE, "price": "free" });
    let (text, is_error) = call_tool("jev_cost", args, &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("jev_cost"), "{text}");
}

#[tokio::test]
async fn answers_offline_and_stamps_the_answers_as_simulated() {
    let (text, is_error) = call_tool("jev_ask", json!({ "page": PAGE }), &offline()).await;
    assert!(!is_error, "{text}");
    assert!(text.contains("is_urgent"), "{text}");
    assert!(text.contains("Simulated answers"), "{text}");

    let live = Host {
        live: true,
        ..offline()
    };
    let (text, _) = call_tool("jev_ask", json!({ "page": PAGE }), &live).await;
    assert!(!text.contains("Simulated answers"), "{text}");
}

#[tokio::test]
async fn refuses_to_send_a_page_with_no_state() {
    let args = json!({ "page": "---\nis_urgent? Urgent" });
    let (text, is_error) = call_tool("jev_ask", args, &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("No state"), "{text}");
}

#[tokio::test]
async fn judges_the_state_the_call_passes() {
    let (sender, mut seen) = tokio::sync::mpsc::unbounded_channel::<String>();
    let host = Host {
        ask: Arc::new(move |session: Session| {
            let sender = sender.clone();
            Box::pin(async move {
                let _ = sender.send(session.state_preview());
                Sent {
                    outcome: Outcome::Ok {
                        answers: headless::mock_answers(&session),
                        usage: None,
                    },
                    raw: None,
                }
            })
        }),
        ..offline()
    };
    let args = json!({ "page": PAGE, "state": "Something else entirely" });
    call_tool("jev_ask", args, &host).await;
    assert_eq!(
        seen.recv().await.as_deref(),
        Some("Something else entirely")
    );
}

#[tokio::test]
async fn passes_a_failed_call_back_as_an_error_result() {
    let host = Host {
        ask: Arc::new(|_| Box::pin(async { Sent::failed("401 no key") })),
        ..offline()
    };
    let (text, is_error) = call_tool("jev_ask", json!({ "page": PAGE }), &host).await;
    assert!(is_error, "{text}");
    assert!(text.contains("401 no key"), "{text}");
}

#[tokio::test]
async fn scores_a_page_over_labelled_cases() {
    let cases = "{\"state\": \"My card was declined\", \"expect\": {\"department\": \"billing\"}}\n\
                 {\"state\": \"The webhook returns 500\", \"expect\": {\"department\": \"technical\"}}";
    let args = json!({ "page": PAGE, "cases": cases, "json": true });
    let (text, is_error) = call_tool("jev_eval", args, &offline()).await;
    assert!(!is_error, "{text}");
    let report: Value = serde_json::from_str(&text).expect("a report");
    assert_eq!(report["cases"], 2);
    assert_eq!(report["errors"], json!([]));

    let args = json!({ "page": PAGE, "cases": "{oops}" });
    let (text, is_error) = call_tool("jev_eval", args, &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("jev_eval"), "{text}");
}

#[tokio::test]
async fn compares_two_pages_over_the_same_cases() {
    let cases = "{\"state\": \"My card was declined\", \"expect\": {\"department\": \"billing\", \"tone\": true}}\n\
                 {\"state\": \"The webhook returns 500\", \"expect\": {\"department\": \"technical\"}}";
    let other = format!("{PAGE}tone? The customer is polite\n");
    let args = json!({ "page": PAGE, "compare": other, "cases": cases, "json": true });
    let (text, is_error) = call_tool("jev_eval", args, &offline()).await;
    assert!(!is_error, "{text}");
    let report: Value = serde_json::from_str(&text).expect("a comparison");
    let names: Vec<&String> = report["questions"]
        .as_object()
        .expect("questions")
        .keys()
        .collect();
    assert_eq!(names, vec!["department"]);
    assert_eq!(report["onlyB"], json!(["tone"]));
    assert_eq!(report["unpaired"], json!([]));

    let args = json!({ "page": PAGE, "compare": other, "cases": cases });
    let (table, _) = call_tool("jev_eval", args, &offline()).await;
    assert!(table.contains("McNemar"), "{table}");
    assert!(table.contains("Simulated answers"), "{table}");
    let args = json!({ "page": PAGE, "compare": "nothing here", "cases": cases });
    let (broken, is_error) = call_tool("jev_eval", args, &offline()).await;
    assert!(is_error, "{broken}");
    assert!(broken.contains("compare:"), "{broken}");
}

#[tokio::test]
async fn hands_back_the_page_with_its_bars_written_in_when_asked_to_calibrate() {
    let cases = "{\"state\": \"My card was declined\", \"expect\": {\"department\": \"billing\"}}\n\
                 {\"state\": \"The webhook returns 500\", \"expect\": {\"department\": \"technical\"}}";
    let args = json!({ "page": PAGE, "cases": cases, "calibrate": true, "targetAccuracy": 0 });
    let (table, is_error) = call_tool("jev_eval", args.clone(), &offline()).await;
    assert!(!is_error, "{table}");
    assert!(
        table.contains("calibration  target accuracy 0.00"),
        "{table}"
    );
    assert!(table.contains("# the page, calibrated"), "{table}");
    assert!(table.contains("  @confidence 0\n"), "{table}");
    let mut with_json = args.clone();
    with_json["json"] = json!(true);
    let (text, _) = call_tool("jev_eval", with_json, &offline()).await;
    let report: Value = serde_json::from_str(&text).expect("a report");
    assert_eq!(report["calibration"]["written"], true);
    assert!(
        report["calibration"]["text"]
            .as_str()
            .expect("the page")
            .contains("technical = Bugs or integration problems\n  @confidence 0"),
        "{text}"
    );
    let body = headless::request_text(&headless::load(PAGE).expect("the page"), "m");
    let mut refused = args;
    refused["page"] = json!(body);
    let (text, is_error) = call_tool("jev_eval", refused, &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("calibrate needs a .jev page"), "{text}");
}

#[tokio::test]
async fn writes_the_page_out_as_a_program() {
    let (text, is_error) = call_tool("jev_code", json!({ "page": PAGE }), &offline()).await;
    assert!(!is_error, "{text}");
    assert!(text.contains("fn main"), "{text}");
    assert!(text.contains("system_one"), "{text}");
}

#[tokio::test]
async fn hands_over_ready_made_pages_that_parse() {
    let (all, _) = call_tool("jev_presets", json!({}), &offline()).await;
    assert!(all.contains("# triage"), "{all}");

    let (one, _) = call_tool("jev_presets", json!({ "name": "moderation" }), &offline()).await;
    let page = one.split_once("\n\n").expect("a heading then a page").1;
    assert!(headless::load(page).is_ok(), "{page}");

    let (text, is_error) = call_tool("jev_presets", json!({ "name": "nope" }), &offline()).await;
    assert!(is_error, "{text}");
}

#[tokio::test]
async fn names_a_tool_it_does_not_have_and_rejects_a_bad_argument() {
    let (text, is_error) = call_tool("jev_teleport", json!({}), &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("tools/list"), "{text}");

    let (text, is_error) = call_tool("jev_check", json!({ "page": 12 }), &offline()).await;
    assert!(is_error, "{text}");
    assert!(text.contains("must be a string"), "{text}");
}

// ---- where an install puts things ---------------------------------------------------------

fn server() -> ServerEntry {
    let mut entry = ServerEntry::new("jev", "/opt/jev");
    entry
        .env
        .push(("TYPESAFE_API_KEY".to_owned(), "sk-test".to_owned()));
    entry
}

#[test]
fn knows_the_four_agents_and_both_scopes() {
    assert_eq!(install::ids(), ["claude-code", "codex", "opencode", "pi"]);
    let paths = |scope: Scope| -> Vec<String> {
        install::CLIENTS
            .iter()
            .map(|client| install::file(client, Kind::Mcp, scope).join("/"))
            .collect()
    };
    assert_eq!(
        paths(Scope::User),
        [
            ".claude.json",
            ".codex/config.toml",
            ".config/opencode/opencode.json",
            ".pi/agent/mcp.json",
        ]
    );
    assert_eq!(
        paths(Scope::Project),
        [
            ".mcp.json",
            ".codex/config.toml",
            "opencode.json",
            ".mcp.json"
        ]
    );
}

#[test]
fn puts_the_skill_in_a_folder_of_its_own() {
    for client in install::CLIENTS {
        for scope in [Scope::User, Scope::Project] {
            let segments = install::file(client, Kind::Skill, scope);
            assert_eq!(&segments[segments.len() - 2..], [SKILL_NAME, "SKILL.md"]);
        }
    }
}

#[test]
fn merges_into_a_config_that_already_has_servers_in_it() {
    let client = install::find("claude-code").unwrap();
    let existing = r#"{"numStartups": 5, "mcpServers": {"other": {"command": "y"}}}"#;
    let merged = install::merge(client, Kind::Mcp, existing, &server()).expect("it merges");
    let root: Value = serde_json::from_str(&merged).expect("still JSON");
    assert_eq!(root["numStartups"], 5);
    let names: Vec<&str> = root["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["other", "jev"]);
    assert_eq!(
        root["mcpServers"]["jev"],
        json!({
            "type": "stdio",
            "command": "/opt/jev",
            "args": ["mcp"],
            "env": { "TYPESAFE_API_KEY": "sk-test" },
        })
    );
}

#[test]
fn writes_opencodes_shape_with_the_command_as_a_list() {
    let client = install::find("opencode").unwrap();
    let merged = install::merge(client, Kind::Mcp, "", &server()).expect("it merges");
    let root: Value = serde_json::from_str(&merged).expect("still JSON");
    assert_eq!(root["$schema"], "https://opencode.ai/config.json");
    assert_eq!(
        root["mcp"]["jev"],
        json!({
            "type": "local",
            "command": ["/opt/jev", "mcp"],
            "enabled": true,
            "environment": { "TYPESAFE_API_KEY": "sk-test" },
        })
    );
}

#[test]
fn leaves_a_json_config_alone_when_it_cannot_be_read() {
    let client = install::find("pi").unwrap();
    assert!(install::merge(client, Kind::Mcp, "[1, 2]", &server()).is_err());
    assert!(install::merge(client, Kind::Mcp, "{oops", &server()).is_err());
}

#[test]
fn appends_a_toml_table_without_touching_the_rest() {
    let client = install::find("codex").unwrap();
    let existing = "# mine\n[history]\npersistence = \"save-all\"\n";
    let merged = install::merge(client, Kind::Mcp, existing, &server()).expect("it merges");
    assert!(merged.contains("# mine"), "{merged}");
    assert!(merged.contains("persistence = \"save-all\""), "{merged}");
    assert!(
        merged.contains("[mcp_servers.jev]\ncommand = \"/opt/jev\"\nargs = [\"mcp\"]"),
        "{merged}"
    );
    assert!(
        merged.contains("env = { TYPESAFE_API_KEY = \"sk-test\" }"),
        "{merged}"
    );
}

#[test]
fn replaces_the_table_it_wrote_before_and_only_that_one() {
    let client = install::find("codex").unwrap();
    let existing = "[mcp_servers.other]\ncommand = \"x\"\n\n\
                    [mcp_servers.jev]\ncommand = \"/old/jev\"\nargs = [\"mcp\"]\n\n\
                    [mcp_servers.jev.env]\nTYPESAFE_API_KEY = \"old\"\n\n\
                    [tui]\ntheme = \"dark\"\n";
    let merged = install::merge(client, Kind::Mcp, existing, &server()).expect("it merges");
    assert!(!merged.contains("/old/jev"), "{merged}");
    assert!(!merged.contains("TYPESAFE_API_KEY = \"old\""), "{merged}");
    assert!(
        merged.contains("[mcp_servers.other]\ncommand = \"x\""),
        "{merged}"
    );
    assert!(merged.contains("[tui]\ntheme = \"dark\""), "{merged}");
    assert_eq!(merged.matches("[mcp_servers.jev]").count(), 1, "{merged}");
}

#[test]
fn quotes_a_path_with_a_space_or_a_backslash() {
    let windows = ServerEntry::new("jev", r#"C:\Program Files\jev\"jev".exe"#);
    let table = install::toml_table(&windows);
    assert!(
        table.contains(r#"command = "C:\\Program Files\\jev\\\"jev\".exe""#),
        "{table}"
    );
}

#[test]
fn installs_the_skill_as_is_whatever_the_agent() {
    for client in install::CLIENTS {
        let merged = install::merge(client, Kind::Skill, "old text", &server()).expect("a skill");
        assert_eq!(merged, SKILL_MD);
    }
}

#[test]
fn can_tell_its_own_skill_file_from_someone_elses() {
    assert!(install::looks_like_ours(""));
    assert!(install::looks_like_ours(SKILL_MD));
    assert!(install::looks_like_ours(
        "---\nname: jev\ndescription: an older one\n---\nhi"
    ));
    assert!(!install::looks_like_ours("---\nname: mine\n---\nhi"));
    assert!(!install::looks_like_ours("# my notes"));
}

#[test]
fn the_skill_starts_with_the_frontmatter_every_host_reads_it_by() {
    let end = SKILL_MD.find("\n---").expect("frontmatter ends");
    let frontmatter = &SKILL_MD[..end];
    assert!(frontmatter.starts_with("---\n"), "{frontmatter}");
    assert!(
        frontmatter.contains(&format!("name: {SKILL_NAME}")),
        "{frontmatter}"
    );
    assert!(frontmatter.contains("description:"), "{frontmatter}");
    for mark in ["name? instructions", "label = description", "`<`", "@model"] {
        assert!(SKILL_MD.contains(mark), "the skill teaches {mark}");
    }
}

// ---- the binary ----------------------------------------------------------------------------

struct Attempt {
    status: i32,
    stdout: String,
    stderr: String,
}

fn jev(args: &[&str], input: &str) -> Attempt {
    let mut child = Command::new(JEV)
        .args(args)
        .env("TYPESAFE_API_KEY", "")
        .env("JEV_PRICE", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("jev starts");
    child
        .stdin
        .take()
        .expect("stdin is a pipe")
        .write_all(input.as_bytes())
        .expect("the messages go in");
    let out = child.wait_with_output().expect("jev finishes");
    Attempt {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A home directory of its own for each test that writes one.
fn temp_home(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jev-install-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a home to install into");
    dir
}

#[test]
fn lists_every_agent_and_where_its_files_go() {
    let done = jev(&["install", "--list"], "");
    assert_eq!(done.status, 0);
    for id in install::ids() {
        assert!(done.stdout.contains(id), "{}", done.stdout);
    }
    assert!(
        done.stdout.contains("~/.codex/config.toml"),
        "{}",
        done.stdout
    );
}

#[test]
fn writes_nothing_for_a_dry_run() {
    let home = temp_home("dry");
    let done = jev(
        &[
            "install",
            "--home",
            home.to_str().unwrap(),
            "--client",
            "all",
            "--dry-run",
        ],
        "",
    );
    assert_eq!(done.status, 0, "{}", done.stderr);
    assert!(done.stdout.contains("would be created"), "{}", done.stdout);
    assert!(!home.join(".claude.json").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn installs_for_one_agent_and_leaves_the_others_alone() {
    let home = temp_home("one");
    let done = jev(
        &[
            "install",
            "--home",
            home.to_str().unwrap(),
            "--client",
            "codex",
            "--command",
            "jev",
        ],
        "",
    );
    assert_eq!(done.status, 0, "{}", done.stderr);
    let toml = std::fs::read_to_string(home.join(".codex/config.toml")).expect("a config");
    assert!(
        toml.contains("[mcp_servers.jev]\ncommand = \"jev\""),
        "{toml}"
    );
    let skill = std::fs::read_to_string(home.join(".codex/skills/jev/SKILL.md")).expect("a skill");
    assert_eq!(skill, SKILL_MD);
    assert!(!home.join(".claude.json").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn refuses_to_overwrite_a_skill_it_did_not_write() {
    let home = temp_home("force");
    let path = home.join(".claude/skills/jev/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).expect("a skills directory");
    std::fs::write(&path, "---\nname: mine\n---\nhands off\n").expect("someone else's skill");
    let home_arg = home.to_str().unwrap().to_owned();

    let refused = jev(
        &[
            "install",
            "skill",
            "--home",
            &home_arg,
            "--client",
            "claude-code",
        ],
        "",
    );
    assert_eq!(refused.status, 1, "{}", refused.stdout);
    assert!(refused.stderr.contains("--force"), "{}", refused.stderr);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("hands off"),
        "the file was replaced anyway"
    );

    let forced = jev(
        &[
            "install",
            "skill",
            "--home",
            &home_arg,
            "--client",
            "claude-code",
            "--force",
        ],
        "",
    );
    assert_eq!(forced.status, 0, "{}", forced.stderr);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn says_when_it_cannot_find_an_agent_to_install_into() {
    let home = temp_home("empty");
    let done = jev(&["install", "--home", home.to_str().unwrap()], "");
    assert_eq!(done.status, 1);
    assert!(done.stderr.contains("--client"), "{}", done.stderr);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn exits_2_on_a_command_line_it_cannot_parse() {
    assert_eq!(jev(&["install", "--client", "emacs"], "").status, 2);
    assert_eq!(jev(&["install", "--scope", "galaxy"], "").status, 2);
    assert_eq!(jev(&["mcp", "page.jev"], "").status, 2);
}

#[test]
fn answers_initialize_before_the_client_says_anything_else() {
    // A real client sends `initialize` and waits on the reply before its next message, so the
    // server must flush an answer when it is ready — not when the next line happens to arrive.
    use std::io::{BufRead, BufReader};
    let mut child = Command::new(JEV)
        .arg("mcp")
        .env("TYPESAFE_API_KEY", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("jev starts");
    let mut stdin = child.stdin.take().expect("stdin is a pipe");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is a pipe"));
    let handshake = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18" },
    });
    writeln!(stdin, "{handshake}").expect("the handshake goes in");
    stdin.flush().expect("and is flushed");
    let (sender, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = stdout.read_line(&mut line);
        let _ = sender.send(line);
    });
    let reply = received
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the handshake is answered while stdin is still open");
    let reply: Value = serde_json::from_str(&reply).expect("the reply is JSON");
    assert_eq!(reply["id"], 1);
    assert_eq!(result_of(&reply)["serverInfo"]["name"], "jev");
    drop(stdin);
    let _ = child.wait();
}

#[test]
fn serves_the_protocol_on_stdin_and_stdout() {
    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "jev_check", "arguments": { "page": PAGE } },
    });
    let input = format!(
        "{}\n{}\n{}\n",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18" },
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        call,
    );
    let done = jev(&["mcp"], &input);
    assert_eq!(done.status, 0, "{}", done.stderr);
    let replies: Vec<Value> = done
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect();
    // Two messages carried an id, and the notification must not have been answered.
    assert_eq!(replies.len(), 2, "{}", done.stdout);
    let handshake = replies
        .iter()
        .find(|reply| reply["id"] == 1)
        .expect("the handshake is answered");
    assert_eq!(result_of(handshake)["serverInfo"]["name"], "jev");
    let checked = replies
        .iter()
        .find(|reply| reply["id"] == 2)
        .expect("the tool call is answered");
    assert!(tool_text(checked).0.contains("is_urgent (noul)"));
    assert!(done.stderr.contains("simulated"), "{}", done.stderr);
}
