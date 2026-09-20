//! `jev` — an interactive playground for learning TypeSafe AI System One questions.
//!
//! Run it with a `TYPESAFE_API_KEY` for live answers, or without one to explore offline with
//! simulated answers. `:help` inside lists every command; `:lesson` starts the guided track.
//!
//! With a subcommand it does not open a terminal at all: `jev run page.jev`, `jev json`, `jev cost`
//! and friends read a page (or stdin) and print one answer, so a session shaped in the REPL can be
//! saved with `:save` and then run from a script, a Makefile or CI.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use ratatui::crossterm::event;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use jev_repl::app::{App, Msg};
use jev_repl::cost::Rates;
use jev_repl::evaluate::{self, Outcome};
use jev_repl::headless::Answered;
use jev_repl::session::Session;
use jev_repl::{cost, headless, ui};
use typesafe::{Client, Usage};

/// 0 when it worked, 1 when the call or the file did not, 2 when the command line did not parse.
const OK: u8 = 0;
const FAILED: u8 = 1;
const BAD_USAGE: u8 = 2;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(command) = args.first().filter(|a| headless::is_command(a)) {
        let command = command.clone();
        return ExitCode::from(one_shot(&command, &args[1..]).await);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", help());
        return ExitCode::from(OK);
    }
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::from(OK);
    }
    if let Some(unknown) = args.first() {
        eprintln!("jev: unknown command {unknown:?}. jev --help lists them.");
        return ExitCode::from(BAD_USAGE);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "jev needs an interactive terminal (stdin and stdout must be a TTY).\n\
             Without one, `jev run <file>` sends a saved page and prints the answers; \
             jev --help lists the rest."
        );
        return ExitCode::from(FAILED);
    }
    match repl().await {
        Ok(()) => ExitCode::from(OK),
        Err(e) => {
            eprintln!("jev: {e}");
            ExitCode::from(FAILED)
        }
    }
}

fn help() -> String {
    let commands: String = headless::COMMANDS
        .iter()
        .map(|(name, about)| format!("  {name:<22} {about}\n"))
        .collect();
    format!(
        "jev — a REPL for TypeSafe AI System One questions\n\n\
         Set TYPESAFE_API_KEY for live answers; without one, answers are simulated locally.\n\n\
         \x20 jev                    the REPL: :help for commands, :lesson for the guided track,\n\
         \x20                        :sketch to write a request as one page, :quit to leave\n\
         \x20 jev <command> [file]   one shot, no terminal needed\n\n\
         Commands\n{commands}\n\
         The file is a .jev sketch page or a request body; `-`, or no file at all, reads stdin.\n\n\
         Options\n\
         \x20 --state <text>         set the state, or replace the one on the page\n\
         \x20 --model <name>         the model to ask\n\
         \x20 --threshold <0-1>      what counts as a yes for a noul (default 0.5)\n\
         \x20 --price <in>/<out>     dollars per million tokens, input then output\n\
         \x20 --timeout <seconds>    per-attempt timeout for a live call\n\
         \x20 --mock                 simulated answers, even when a key is set\n\
         \x20 --json                 print the raw response body instead of the answer page\n\
         \x20 --help, -h             this message\n\
         \x20 --version, -v          the version of this package\n\n\
         Options for eval\n\
         \x20 --cases <file>         the JSON Lines file of labelled states to score, or `-`\n\
         \x20 --concurrency <n>      how many cases are in the air at once (default 4)\n\
         \x20 --cache <dir>          keep the responses here, so running it again sends nothing\n\
         \x20 --max-cost <dollars>   refuse to send when the estimate is above this\n\
         \x20 --min-accuracy <0-1>   exit 1 when a scored question falls below this\n\n\
         Exit status is 0 when it worked, 1 when the call or the file did not, 2 when the\n\
         command line did not parse.\n"
    )
}

/// Everything the one-shot commands read off the command line.
#[derive(Debug, Default)]
struct Options {
    file: String,
    state: Option<String>,
    model: Option<String>,
    threshold: f64,
    rates: Option<Rates>,
    timeout: Option<Duration>,
    mock: bool,
    json: bool,
    /// `jev eval`: the file of labelled cases, and what to do with them.
    cases: Option<String>,
    concurrency: usize,
    cache: Option<String>,
    max_cost: Option<f64>,
    min_accuracy: Option<f64>,
}

/// Read the flags after a subcommand. `--flag value` and `--flag=value` both work, and the first
/// bare word is the file — a page is a path, not a flag, so there is only ever one.
fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        file: "-".to_owned(),
        threshold: 0.5,
        rates: cost::rates_from_env(),
        concurrency: 4,
        ..Options::default()
    };
    let mut file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (name, inline) = match arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        let mut value = || -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => {
                    i += 1;
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| format!("{name} needs a value."))
                }
            }
        };
        match name.as_str() {
            "--state" => options.state = Some(value()?),
            "--model" => options.model = Some(value()?),
            "--threshold" => {
                let text = value()?;
                let n: f64 = text
                    .parse()
                    .map_err(|_| "--threshold takes a number from 0 to 1.".to_owned())?;
                if !(0.0..=1.0).contains(&n) {
                    return Err("--threshold takes a number from 0 to 1.".to_owned());
                }
                options.threshold = n;
            }
            "--price" => options.rates = Some(cost::parse_rates(&value()?)?),
            "--timeout" => {
                let text = value()?;
                let seconds: f64 = text.parse().unwrap_or(0.0);
                if !(seconds.is_finite() && seconds > 0.0) {
                    return Err("--timeout takes a number of seconds greater than 0.".to_owned());
                }
                options.timeout = Some(Duration::from_secs_f64(seconds));
            }
            "--cases" => options.cases = Some(value()?),
            "--concurrency" => {
                let text = value()?;
                let workers: i64 = text
                    .parse()
                    .map_err(|_| "--concurrency takes a whole number of 1 or more.".to_owned())?;
                if workers < 1 {
                    return Err("--concurrency takes a whole number of 1 or more.".to_owned());
                }
                options.concurrency = workers as usize;
            }
            "--cache" => options.cache = Some(value()?),
            "--max-cost" => {
                let text = value()?;
                let dollars: f64 = text.parse().unwrap_or(f64::NAN);
                if !(dollars.is_finite() && dollars > 0.0) {
                    return Err("--max-cost takes a number of dollars greater than 0.".to_owned());
                }
                options.max_cost = Some(dollars);
            }
            "--min-accuracy" => {
                let text = value()?;
                let bar: f64 = text
                    .parse()
                    .map_err(|_| "--min-accuracy takes a number from 0 to 1.".to_owned())?;
                if !(0.0..=1.0).contains(&bar) {
                    return Err("--min-accuracy takes a number from 0 to 1.".to_owned());
                }
                options.min_accuracy = Some(bar);
            }
            "--mock" => options.mock = true,
            "--json" => options.json = true,
            other => {
                if other.starts_with('-') && other != "-" {
                    return Err(format!("unknown option {other}. jev --help lists them."));
                }
                if let Some(first) = &file {
                    return Err(format!("expected one file, got {first:?} and {arg:?}."));
                }
                file = Some(arg.clone());
            }
        }
        i += 1;
    }
    if let Some(path) = file {
        options.file = path;
    }
    Ok(options)
}

/// The flags `eval` reads differently from the other commands.
///
/// `--state` is the interesting one: a page's state is what the cases replace, so passing one
/// would quietly judge the same text forty times.
fn check_eval_options(options: &Options) -> Result<(), String> {
    if options.state.is_some() {
        return Err("--state does not apply to eval: the cases carry the states.".to_owned());
    }
    let Some(cases) = &options.cases else {
        return Err("--cases <file> is required: jev eval page.jev --cases cases.jsonl".to_owned());
    };
    if cases == "-" && options.file == "-" {
        return Err("the page and the cases cannot both come from stdin.".to_owned());
    }
    if options.max_cost.is_some() && options.rates.is_none() {
        return Err("--max-cost needs rates: pass --price <in>/<out> or set JEV_PRICE.".to_owned());
    }
    Ok(())
}

/// The page: a file, or everything on stdin when the path is `-`.
fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut text = String::new();
        return std::io::stdin()
            .read_to_string(&mut text)
            .map(|_| text)
            .map_err(|e| format!("could not read stdin: {e}"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))
}

/// One shot: read a page, print one thing, say whether it worked.
async fn one_shot(command: &str, args: &[String]) -> u8 {
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("jev {command}: {e}");
            return BAD_USAGE;
        }
    };
    if command == "eval"
        && let Err(e) = check_eval_options(&options)
    {
        eprintln!("jev {command}: {e}");
        return BAD_USAGE;
    }

    let text = match read_input(&options.file) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("jev {command}: {e}");
            return FAILED;
        }
    };

    if command == "check" {
        return match headless::check_text(&text) {
            Ok(summary) => {
                println!("{summary}");
                OK
            }
            Err(problems) => {
                eprintln!("{problems}");
                FAILED
            }
        };
    }

    let mut session: Session = match headless::load(&text) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("jev {command}: {e}");
            return FAILED;
        }
    };
    if let Some(state) = &options.state {
        session.state = serde_json::Value::String(state.clone());
    }
    if let Some(model) = &options.model {
        session.model = Some(model.clone());
    }

    // A key makes the model name the client's default; without one the published default stands.
    let client = if options.mock {
        None
    } else {
        Client::from_env().ok()
    };
    let model = session
        .model
        .clone()
        .or_else(|| client.as_ref().map(|c| c.default_model().to_owned()))
        .unwrap_or_else(|| "jev-latest".to_owned());

    if command == "eval" {
        return run_eval(session, &options, &model, client).await;
    }

    match command {
        "json" => {
            print!("{}", headless::request_text(&session, &model));
            return OK;
        }
        "cost" => {
            print!("{}", headless::cost_text(&session, &model, options.rates));
            return OK;
        }
        "rust" => {
            print!(
                "{}",
                headless::code_text(&session, &model, options.threshold)
            );
            return OK;
        }
        _ => {}
    }

    if let Some(why) = headless::sendable(&session) {
        eprintln!("jev run: {why}");
        return FAILED;
    }

    let Some(client) = client else {
        let answers = headless::mock_answers(&session);
        if options.json {
            print!("{}", headless::answers_json(&answers, &model, None));
        } else {
            print!("{}", headless::answers_text(&answers, options.threshold));
            print!(
                "{}",
                headless::usage_text(&session, &model, options.rates, None)
            );
            eprintln!(
                "Simulated answers: deterministic noise, not judgement. Set TYPESAFE_API_KEY for real ones."
            );
        }
        return OK;
    };

    let mut request = client
        .system_one(session.state.clone(), session.to_questions())
        .model(model.clone());
    if let Some(timeout) = options.timeout {
        request = request.timeout(timeout);
    }
    match request.await {
        Ok(response) => {
            let answers = headless::live_answers(&session, &response);
            if options.json {
                print!(
                    "{}",
                    headless::answers_json(&answers, &model, Some(&response.raw))
                );
            } else {
                print!("{}", headless::answers_text(&answers, options.threshold));
                print!(
                    "{}",
                    headless::usage_text(&session, &model, options.rates, Some(&response.usage))
                );
            }
            OK
        }
        Err(e) => {
            let mut stderr = std::io::stderr();
            for line in jev_repl::format::error_lines(&e) {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                let _ = writeln!(stderr, "{text}");
            }
            FAILED
        }
    }
}

/// Simulated answers for a case, the same deterministic ones `jev run --mock` prints.
async fn mock_ask(session: Session) -> Outcome {
    Outcome::Ok {
        answers: headless::mock_answers(&session),
        usage: None,
    }
}

/// The cache key: the request body this case would POST, hashed.
fn cache_key(session: &Session, model: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session.request_json_compact(model));
    format!("{:x}", hasher.finalize())
}

/// A cached response, or `None` when there is none this run can use.
fn cached(file: &Path, session: &Session) -> Option<(Vec<Answered>, Option<Usage>)> {
    // A file this version cannot read is not worth failing a case over; send the request instead.
    let body: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    headless::cached_answers(session, &body)
}

/// One live call for a case, through the cache when there is one.
async fn live_ask(
    client: &Client,
    model: &str,
    timeout: Option<Duration>,
    cache: Option<&str>,
    session: Session,
) -> Outcome {
    let file = cache.map(|dir| Path::new(dir).join(format!("{}.json", cache_key(&session, model))));
    if let Some(file) = &file
        && let Some((answers, usage)) = cached(file, &session)
    {
        return Outcome::Ok { answers, usage };
    }
    let mut request = client
        .system_one(session.state.clone(), session.to_questions())
        .model(model.to_owned());
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    match request.await {
        Ok(response) => {
            if let Some(file) = &file {
                let body = serde_json::to_string(&response.raw).unwrap_or_default();
                let _ = std::fs::write(file, format!("{body}\n"));
            }
            Outcome::Ok {
                answers: headless::live_answers(&session, &response),
                usage: Some(response.usage.clone()),
            }
        }
        Err(e) => Outcome::Failed {
            error: error_text(&e),
        },
    }
}

/// The same explanation `jev run` prints when a call fails, as one string.
fn error_text(err: &typesafe::Error) -> String {
    jev_repl::format::error_lines(err)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// `jev eval`: the page over a file of labelled states, scored.
///
/// The order matters. Nothing is sent until the cases have parsed and the estimate has been shown,
/// because a cases file is the one input that turns a typo into a bill.
async fn run_eval(session: Session, options: &Options, model: &str, client: Option<Client>) -> u8 {
    let text = match read_input(options.cases.as_deref().unwrap_or("-")) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("jev eval: {e}");
            return FAILED;
        }
    };
    let cases = match evaluate::parse_cases(&text, &session) {
        Ok(cases) => cases,
        Err(e) => {
            eprintln!("jev eval: {e}");
            return FAILED;
        }
    };

    let live = client.is_some();
    if live {
        let estimate = evaluate::preflight(&session, &cases, model, options.rates);
        let money = match (estimate.cost, options.rates) {
            (Some(cost), Some(rates)) => format!(
                ", ≈ {} at {}",
                cost::usd(cost.total),
                cost::format_rates(rates)
            ),
            _ => String::new(),
        };
        let plural = if estimate.cases == 1 { "" } else { "s" };
        eprintln!(
            "jev eval: {} case{plural}, ≈ {} in / {} out tokens{money}",
            estimate.cases, estimate.input_tokens, estimate.output_tokens
        );
        if let (Some(max), Some(cost)) = (options.max_cost, estimate.cost)
            && cost.total > max
        {
            eprintln!(
                "jev eval: refusing to send: ≈ {} is above --max-cost {}.",
                cost::usd(cost.total),
                cost::usd(max)
            );
            return FAILED;
        }
        if let Some(dir) = &options.cache
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("jev eval: could not use {dir}: {e}");
            return FAILED;
        }
    }

    let outcomes = match client {
        None => evaluate::run(&session, &cases, mock_ask, options.concurrency).await,
        Some(client) => {
            let model = model.to_owned();
            let timeout = options.timeout;
            let cache = options.cache.clone();
            let ask = move |one: Session| {
                let (client, model, cache) = (client.clone(), model.clone(), cache.clone());
                async move { live_ask(&client, &model, timeout, cache.as_deref(), one).await }
            };
            evaluate::run(&session, &cases, ask, options.concurrency).await
        }
    };

    let report = evaluate::report(
        &session,
        &cases,
        &outcomes,
        evaluate::ReportOptions {
            model,
            threshold: options.threshold,
            rates: options.rates,
        },
    );
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&evaluate::report_json(&report)).unwrap_or_default()
        );
    } else {
        print!("{}", evaluate::report_text(&report));
    }
    if !live {
        eprintln!(
            "Simulated answers: deterministic noise, not judgement. Set TYPESAFE_API_KEY for real ones."
        );
    }

    let mut code = if report.errors.is_empty() { OK } else { FAILED };
    if let Some(bar) = options.min_accuracy {
        for (name, accuracy) in evaluate::below_bar(&report, bar) {
            eprintln!(
                "jev eval: {name} accuracy {} is below {}.",
                evaluate::two(accuracy),
                evaluate::two(bar)
            );
            code = FAILED;
        }
    }
    code
}

/// The REPL itself: the terminal, the event loop, the draw.
async fn repl() -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_input(tx.clone());
    spawn_ticker(tx.clone());

    let mut terminal = ratatui::init();
    let mut app = App::new(tx);
    let mut redraw = true;
    let result = loop {
        if redraw && let Err(e) = terminal.draw(|frame| ui::render(frame, &mut app)) {
            break Err(e);
        }
        let Some(msg) = rx.recv().await else {
            break Ok(());
        };
        // Ticks only matter while the spinner is turning; otherwise an idle REPL redraws nothing.
        redraw = !matches!(msg, Msg::Tick) || app.pending;
        app.handle(msg);
        if app.quit {
            break Ok(());
        }
    };
    ratatui::restore();
    result
}

/// Terminal events come from a blocking thread so the async side stays free for API calls.
fn spawn_input(tx: mpsc::UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Term(ev)).is_err() {
                break;
            }
        }
    });
}

/// Drives the spinner while a request is in flight.
fn spawn_ticker(tx: mpsc::UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(120));
        loop {
            interval.tick().await;
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
}
