//! jev as an MCP server: the one-shot commands, offered to an agent as tools.
//!
//! This half is pure — a JSON-RPC message in, a JSON-RPC message out — so the protocol can be
//! tested without a pipe. Everything that touches the network goes through [`Host::ask`], which
//! the caller supplies; [`crate::serve`] is the part that puts it on stdin and stdout.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::cost::Rates;
use crate::evaluate::{self, Outcome, ReportOptions};
use crate::headless;
use crate::session::Session;
use crate::skill::SKILL_MD;
use crate::{cost, presets, sketch};

/// The protocol revision this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions a client may ask for and still be understood; anything else gets ours back.
pub const KNOWN_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The name the server reports at `initialize`, and the prefix on every tool.
pub const SERVER_NAME: &str = "jev";

/// JSON-RPC error codes, the ones this server can actually raise.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// What the host got back from one send: the answers `jev eval` scores, plus the raw body when
/// there was one, so `jev_ask` can hand it over for a script to read.
#[derive(Debug, Clone)]
pub struct Sent {
    pub outcome: Outcome,
    pub raw: Option<Value>,
}

impl Sent {
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Failed {
                error: error.into(),
            },
            raw: None,
        }
    }
}

/// Send one session and come back with its answers, or with why it did not.
///
/// Boxed rather than generic: the server hands this to spawned tasks and to the eval runner, and
/// one `Arc` is easier to pass around than a type parameter threaded through every function.
pub type Ask =
    Arc<dyn Fn(Session) -> Pin<Box<dyn Future<Output = Sent> + Send>> + Send + Sync + 'static>;

/// What the host does for the tools that need more than text: send a request, price it, name a model.
#[derive(Clone)]
pub struct Host {
    /// The version reported at `initialize`.
    pub version: String,
    /// The model a page that pins none is sent with.
    pub model: String,
    /// Whether answers come from the API. False means every answer is simulated.
    pub live: bool,
    /// Token prices, when the host was given any.
    pub rates: Option<Rates>,
    pub ask: Ask,
}

/// What a tool call comes back with: text, and whether it describes a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn failed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }

    pub fn to_value(&self) -> Value {
        let mut value = json!({ "content": [{ "type": "text", "text": self.text }] });
        if self.is_error {
            value["isError"] = Value::Bool(true);
        }
        value
    }
}

const PAGE_DESCRIPTION: &str = "The request as a jev sketch page, or a raw /v1/systemone request body. \
     jev_notation has the notation.";

/// Every tool this server offers, in the order an agent should reach for them.
pub fn tools() -> Value {
    let page = json!({ "type": "string", "description": PAGE_DESCRIPTION });
    let state = json!({
        "type": "string",
        "description": "Judge this text instead of the state written on the page.",
    });
    let model = json!({
        "type": "string",
        "description":
            "The model to ask. Defaults to the one the page pins, then to the server's default.",
    });
    let threshold = json!({
        "type": "number",
        "description": "What counts as a yes for a noul, from 0 to 1. Defaults to 0.5.",
    });
    let price = json!({
        "type": "string",
        "description": "Dollars per million tokens, input then output, as \"0.20/1.00\".",
    });
    json!([
        {
            "name": "jev_notation",
            "title": "jev notation",
            "description":
                "The jev sketch notation and workflow: how to write a page of questions, what each \
                 kind of question answers with, and how to check, price, run and score one. Read \
                 this before writing a page, and again when one will not parse.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        },
        {
            "name": "jev_check",
            "title": "Check a page",
            "description":
                "Parse a page and report every problem with a line number, or say what it parsed \
                 into. Sends nothing and costs nothing — use it on every page before running one.",
            "inputSchema": {
                "type": "object",
                "properties": { "page": page },
                "required": ["page"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_request",
            "title": "Request body",
            "description":
                "The exact JSON body this page would POST to /v1/systemone. Sends nothing.",
            "inputSchema": {
                "type": "object",
                "properties": { "page": page, "state": state, "model": model },
                "required": ["page"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_cost",
            "title": "Estimate cost",
            "description":
                "Estimated tokens for this page, per question and on both sides of the wire, \
                 priced when rates are given. Sends nothing. Check this before a run over many \
                 cases.",
            "inputSchema": {
                "type": "object",
                "properties": { "page": page, "state": state, "model": model, "price": price },
                "required": ["page"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_ask",
            "title": "Ask the questions",
            "description":
                "Send the page and return one answer per question. Spends money when the server \
                 holds an API key; without one every answer is simulated noise and must not be \
                 reported as judgement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "page": page,
                    "state": state,
                    "model": model,
                    "threshold": threshold,
                    "json": {
                        "type": "boolean",
                        "description": "Return the raw response body instead of the answer page.",
                    },
                },
                "required": ["page"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_eval",
            "title": "Score a page",
            "description":
                "Run a page over labelled cases and score the answers: accuracy per question, a \
                 confusion table, a threshold sweep for each noul. One request per case, so price \
                 it first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "page": page,
                    "cases": {
                        "type": "string",
                        "description":
                            "JSON Lines, one labelled state per line: \
                             {\"state\": \"...\", \"expect\": {\"is_urgent\": true}}.",
                    },
                    "model": model,
                    "threshold": threshold,
                    "price": price,
                    "concurrency": {
                        "type": "integer",
                        "description": "How many cases are in the air at once. Defaults to 4.",
                        "minimum": 1,
                    },
                    "compare": {
                        "type": "string",
                        "description":
                            "A second page to run over the same cases. The report becomes the \
                             difference: deltas per question, the cases whose answer flipped, and \
                             an exact McNemar test.",
                    },
                    "calibrate": {
                        "type": "boolean",
                        "description":
                            "Also return the page with the bars this run supports written in: \
                             each noul's best-F1 @threshold, and the lowest @confidence at which a \
                             choice or score reaches targetAccuracy. Nothing else on the page \
                             changes.",
                    },
                    "targetAccuracy": {
                        "type": "number",
                        "description": "The accuracy a confidence bar has to reach. Defaults to 0.9.",
                        "minimum": 0,
                        "maximum": 1,
                    },
                    "json": {
                        "type": "boolean",
                        "description": "Return the report as JSON instead of a table.",
                    },
                },
                "required": ["page", "cases"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_code",
            "title": "Page as code",
            "description":
                "The page as a working Rust program against typesafe-ai-sdk. Start here instead of \
                 writing a client by hand.",
            "inputSchema": {
                "type": "object",
                "properties": { "page": page, "model": model, "threshold": threshold },
                "required": ["page"],
                "additionalProperties": false,
            },
        },
        {
            "name": "jev_presets",
            "title": "Ready-made pages",
            "description":
                "Worked pages to start from — support triage, content moderation, lead \
                 qualification and reply grading — each as a sketch page ready to edit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "One preset by name. Omit for all of them.",
                    },
                },
                "additionalProperties": false,
            },
        },
    ])
}

/// The line every simulated answer is stamped with, so nobody reports noise as judgement.
const SIMULATED: &str = "\nSimulated answers: deterministic noise, not judgement. \
                         The server has no TYPESAFE_API_KEY, so nothing was sent.";

fn string_arg(args: &Value, name: &str) -> Result<String, String> {
    match args.get(name) {
        None | Some(Value::Null) => Err(format!("{name} is required.")),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("{name} must be a string.")),
    }
}

fn optional_string(args: &Value, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{name} must be a string.")),
    }
}

fn threshold_arg(args: &Value) -> Result<f64, String> {
    let value = match args.get("threshold") {
        None | Some(Value::Null) => return Ok(0.5),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(_) => return Err("threshold must be a number.".to_owned()),
    };
    if !(0.0..=1.0).contains(&value) {
        return Err("threshold must be from 0 to 1.".to_owned());
    }
    Ok(value)
}

fn bool_arg(args: &Value, name: &str) -> Result<bool, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{name} must be true or false.")),
    }
}

fn concurrency_arg(args: &Value) -> Result<usize, String> {
    let workers = match args.get("concurrency") {
        None | Some(Value::Null) => return Ok(4),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(_) => return Err("concurrency must be a number.".to_owned()),
    };
    if workers < 1 {
        return Err("concurrency must be a whole number of 1 or more.".to_owned());
    }
    Ok(workers as usize)
}

/// The rates a call was given, falling back to the host's.
fn rates_arg(args: &Value, host: &Host) -> Result<Option<Rates>, String> {
    match optional_string(args, "price")? {
        None => Ok(host.rates),
        Some(text) => cost::parse_rates(&text).map(Some),
    }
}

/// The page, loaded, with the overrides a call may carry applied.
fn session_arg(args: &Value, host: &Host) -> Result<(Session, String), String> {
    let mut session = headless::load(&string_arg(args, "page")?)?;
    if let Some(state) = optional_string(args, "state")? {
        session.state = Value::String(state);
    }
    if let Some(model) = optional_string(args, "model")? {
        session.model = Some(model);
    }
    let model = session.model.clone().unwrap_or_else(|| host.model.clone());
    Ok((session, model))
}

async fn ask(args: &Value, host: &Host) -> Result<ToolResult, String> {
    let (session, model) = session_arg(args, host)?;
    if let Some(why) = headless::sendable(&session) {
        return Err(why);
    }
    let threshold = threshold_arg(args)?;
    let rates = rates_arg(args, host)?;
    let json_wanted = bool_arg(args, "json")?;

    let sent = (host.ask)(session.clone()).await;
    let (answers, usage) = match sent.outcome {
        Outcome::Failed { error } => return Ok(ToolResult::failed(error)),
        Outcome::Ok { answers, usage } => (answers, usage),
    };
    if json_wanted {
        return Ok(ToolResult::ok(headless::answers_json(
            &answers,
            &model,
            sent.raw.as_ref(),
        )));
    }
    let mut text = headless::session_answers_text(&answers, threshold, &session);
    text.push_str(&headless::usage_text(
        &session,
        &model,
        rates,
        usage.as_ref(),
    ));
    if !host.live {
        text.push_str(SIMULATED);
    }
    Ok(ToolResult::ok(text))
}

async fn score(args: &Value, host: &Host) -> Result<ToolResult, String> {
    let (session, model) = session_arg(args, host)?;
    let threshold = threshold_arg(args)?;
    let concurrency = concurrency_arg(args)?;
    let rates = rates_arg(args, host)?;
    if let Some(second) = optional_string(args, "compare")? {
        return compared(
            args,
            host,
            &second,
            (session, model),
            threshold,
            concurrency,
            rates,
        )
        .await;
    }
    let cases = evaluate::parse_cases(&string_arg(args, "cases")?, &session)?;
    if cases.is_empty() {
        return Err("cases is empty: nothing to score.".to_owned());
    }
    let calibrating = bool_arg(args, "calibrate")?;
    let target = match args.get("targetAccuracy") {
        None | Some(Value::Null) => evaluate::DEFAULT_TARGET,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(_) => return Err("targetAccuracy must be a number.".to_owned()),
    };
    if !(0.0..=1.0).contains(&target) {
        return Err("targetAccuracy must be from 0 to 1.".to_owned());
    }
    let page = string_arg(args, "page")?;
    if calibrating && page.trim_start().starts_with('{') {
        return Err(
            "calibrate needs a .jev page: a request body has nowhere to keep a bar.".to_owned(),
        );
    }
    let json_wanted = bool_arg(args, "json")?;

    let outcomes = evaluate::run(&session, &cases, asker(host), concurrency).await;
    let report = evaluate::report(
        &session,
        &cases,
        &outcomes,
        ReportOptions {
            model: &model,
            threshold,
            rates,
        },
    );
    let calibration = (calibrating && report.errors.is_empty())
        .then(|| evaluate::calibrate(&session, &cases, &outcomes, &report, target));
    let calibrated = calibration
        .as_ref()
        .map(|c| sketch::set_bars(&page, &c.changed));
    let refused = calibrating && calibration.is_none();
    let why = evaluate::not_calibrating(report.errors.len());

    if json_wanted {
        let mut json = evaluate::report_json(&report);
        if let Value::Object(object) = &mut json {
            if let (Some(calibration), Some(text)) = (&calibration, &calibrated) {
                let mut value = evaluate::calibration_json(calibration, "page");
                value.insert("text".to_owned(), Value::String(text.clone()));
                object.insert("calibration".to_owned(), Value::Object(value));
            }
            if refused {
                object.insert("calibration".to_owned(), json!({ "refused": why }));
            }
        }
        let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        return Ok(ToolResult::ok(format!("{body}\n")));
    }
    let mut text = evaluate::report_text(&report);
    if let (Some(calibration), Some(page)) = (&calibration, &calibrated) {
        text.push('\n');
        text.push_str(&evaluate::calibration_text(calibration, "the page"));
        text.push_str(&format!("\n# the page, calibrated\n\n{page}"));
    }
    if refused {
        text.push_str(&format!("\n{why}\n"));
    }
    if !host.live {
        text.push_str(SIMULATED);
    }
    Ok(ToolResult::ok(text))
}

/// The host's `ask`, as the eval runner wants it: a function from a session to an outcome.
fn asker(
    host: &Host,
) -> impl Fn(Session) -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync + Clone + 'static
{
    let send = Arc::clone(&host.ask);
    move |one: Session| {
        let send = Arc::clone(&send);
        Box::pin(async move { send(one).await.outcome })
    }
}

/// `jev_eval` with `compare`: both pages over the same cases, labelled `a` and `b`.
async fn compared(
    args: &Value,
    host: &Host,
    second: &str,
    (a, model_a): (Session, String),
    threshold: f64,
    concurrency: usize,
    rates: Option<Rates>,
) -> Result<ToolResult, String> {
    let mut b = headless::load(second).map_err(|e| format!("compare: {e}"))?;
    // A model named in the call overrides both pages, the way --model does on the command line.
    if let Some(model) = optional_string(args, "model")? {
        b.model = Some(model);
    }
    let model_b = b.model.clone().unwrap_or_else(|| host.model.clone());
    let labels = evaluate::Labels { a: "a", b: "b" };
    let (cases_a, cases_b) =
        evaluate::parse_compare_cases(&string_arg(args, "cases")?, &a, &b, labels)?;
    let json_wanted = bool_arg(args, "json")?;
    let (outcomes_a, outcomes_b) = evaluate::run_compare(
        evaluate::Leg {
            session: &a,
            cases: &cases_a,
            ask: asker(host),
        },
        evaluate::Leg {
            session: &b,
            cases: &cases_b,
            ask: asker(host),
        },
        concurrency,
    )
    .await;
    let comparison = evaluate::compare(
        evaluate::Side {
            label: "a",
            session: &a,
            cases: &cases_a,
            outcomes: &outcomes_a,
            model: &model_a,
        },
        evaluate::Side {
            label: "b",
            session: &b,
            cases: &cases_b,
            outcomes: &outcomes_b,
            model: &model_b,
        },
        evaluate::CompareOptions { threshold, rates },
    );
    if json_wanted {
        let body = serde_json::to_string_pretty(&evaluate::compare_json(&comparison))
            .map_err(|e| e.to_string())?;
        return Ok(ToolResult::ok(format!("{body}\n")));
    }
    let mut text = evaluate::compare_text(&comparison);
    if !host.live {
        text.push_str(SIMULATED);
    }
    Ok(ToolResult::ok(text))
}

/// A preset as the page it builds, with its name and what it is for above it.
fn preset_page(preset: &presets::Preset) -> String {
    format!(
        "# {} — {}\n\n{}",
        preset.name,
        preset.about,
        sketch::render(&presets::to_session(preset))
    )
}

fn preset_pages(args: &Value) -> Result<ToolResult, String> {
    match optional_string(args, "name")? {
        Some(name) => {
            let Some(preset) = presets::find(&name) else {
                let names = presets::PRESETS
                    .iter()
                    .map(|p| p.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!("no preset {name:?}; there is {names}."));
            };
            Ok(ToolResult::ok(preset_page(preset)))
        }
        None => Ok(ToolResult::ok(
            presets::PRESETS
                .iter()
                .map(preset_page)
                .collect::<Vec<_>>()
                .join("\n\n"),
        )),
    }
}

/// Run one tool. Argument problems come back as an error result, not as a JSON-RPC failure.
pub async fn call(name: &str, args: &Value, host: &Host) -> ToolResult {
    let outcome = match name {
        "jev_notation" => Ok(ToolResult::ok(SKILL_MD)),
        "jev_check" => string_arg(args, "page").map(|page| match headless::check_text(&page) {
            Ok(summary) => ToolResult::ok(format!("{summary}\n")),
            Err(problems) => ToolResult::failed(problems),
        }),
        "jev_request" => session_arg(args, host)
            .map(|(session, model)| ToolResult::ok(headless::request_text(&session, &model))),
        "jev_cost" => rates_arg(args, host).and_then(|rates| {
            session_arg(args, host).map(|(session, model)| {
                ToolResult::ok(headless::cost_text(&session, &model, rates))
            })
        }),
        "jev_ask" => ask(args, host).await,
        "jev_eval" => score(args, host).await,
        "jev_code" => threshold_arg(args).and_then(|threshold| {
            session_arg(args, host).map(|(session, model)| {
                ToolResult::ok(headless::code_text(&session, &model, threshold))
            })
        }),
        "jev_presets" => preset_pages(args),
        other => {
            return ToolResult::failed(format!("No tool named {other:?}. tools/list has them."));
        }
    };
    match outcome {
        Ok(result) => result,
        Err(why) => ToolResult::failed(format!("{name}: {why}")),
    }
}

fn reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn fault(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// What `initialize` answers with: what we speak, what we can do, who we are.
fn greeting(params: &Value, host: &Host) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = match asked {
        Some(asked) if KNOWN_PROTOCOLS.contains(&asked) => asked,
        _ => PROTOCOL_VERSION,
    };
    let instructions = format!(
        "jev shapes and sends TypeSafe AI System One questions. Call jev_notation first to learn \
         the page notation, jev_check to make sure a page parses, jev_cost before anything large, \
         then jev_ask or jev_eval.{}",
        if host.live {
            ""
        } else {
            " This server has no API key: every answer is simulated."
        }
    );
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "title": "jev", "version": host.version },
        "instructions": instructions,
    })
}

/// Answer one JSON-RPC message.
///
/// Returns `None` for a notification — a message with no `id` gets no reply, which is the one rule
/// of the protocol that a hand-written server usually gets wrong.
pub async fn handle(message: &Value, host: &Host) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(fault(
            Value::Null,
            INVALID_REQUEST,
            "Expected a JSON-RPC object.".to_owned(),
        ));
    };
    let id = match object.get("id") {
        None | Some(Value::Null) => None,
        Some(id) => Some(id.clone()),
    };
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return id.map(|id| fault(id, INVALID_REQUEST, "No method named.".to_owned()));
    };
    // Nothing this server keeps state for; the handshake's `initialized` is the usual one.
    let id = id?;
    let empty = Value::Object(Map::new());
    let params = object.get("params").unwrap_or(&empty);

    Some(match method {
        "initialize" => reply(id, greeting(params, host)),
        "ping" => reply(id, json!({})),
        "tools/list" => reply(id, json!({ "tools": tools() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(fault(
                    id,
                    INVALID_PARAMS,
                    "tools/call needs a tool name.".to_owned(),
                ));
            };
            let args = params.get("arguments").unwrap_or(&empty).clone();
            reply(id, call(name, &args, host).await.to_value())
        }
        "resources/list" => reply(id, json!({ "resources": [] })),
        "prompts/list" => reply(id, json!({ "prompts": [] })),
        other => fault(id, METHOD_NOT_FOUND, format!("Unknown method {other:?}.")),
    })
}

/// One line of stdio: parse it, answer it, hand back the line to write — or nothing.
pub async fn handle_line(line: &str, host: &Host) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => {
            return Some(
                fault(
                    Value::Null,
                    PARSE_ERROR,
                    format!("Could not parse the message: {e}"),
                )
                .to_string(),
            );
        }
    };
    handle(&message, host).await.map(|value| value.to_string())
}
