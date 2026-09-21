//! What a call is expected to cost: the token estimate, the rates, and the `:cost` command.

use jev_repl::app::{App, Msg};
use jev_repl::cost::{self, Rates};
use jev_repl::editor::{Editor, Preview};
use jev_repl::format::cost_lines;
use jev_repl::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use typesafe::Usage;

fn app() -> App {
    // Tests never talk to the API, and never take their rates from the machine they run on.
    let (tx, _rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(tx);
    app.mock = true;
    app.rates = None;
    app
}

fn transcript(app: &App) -> String {
    app.transcript
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const RATES: Rates = Rates {
    input: 0.2,
    output: 1.0,
};

#[test]
fn prose_costs_about_four_characters_a_token() {
    let text = "The payout failed again, third time this month.";
    let tokens = cost::estimate_tokens(text);
    assert!(tokens > text.len() / 8, "{tokens}");
    assert!(tokens < text.len() / 2, "{tokens}");
}

#[test]
fn whitespace_is_free_and_text_only_grows() {
    assert_eq!(cost::estimate_tokens(""), 0);
    assert_eq!(cost::estimate_tokens("   \n\t "), 0);
    assert_eq!(
        cost::estimate_tokens("payout failed"),
        cost::estimate_tokens("payout     failed")
    );
    assert!(
        cost::estimate_tokens("The payout failed. The payout failed.")
            > cost::estimate_tokens("The payout failed.")
    );
    // A CJK character is a token of its own.
    assert_eq!(cost::estimate_tokens("支払いが失敗"), 6);
}

#[test]
fn a_request_splits_into_state_questions_and_envelope() {
    let mut app = app();
    app.exec(":preset triage");
    let estimate = cost::estimate(&app.session, "jev-latest");

    let questions: usize = estimate.questions.iter().map(|q| q.input_tokens).sum();
    assert_eq!(
        estimate.input_tokens,
        estimate.state_tokens + estimate.envelope_tokens + questions
    );
    let answers: usize = estimate.questions.iter().map(|q| q.output_tokens).sum();
    assert_eq!(
        estimate.output_tokens,
        estimate.answer_envelope_tokens + answers
    );
    let names: Vec<&str> = estimate.questions.iter().map(|q| q.name.as_str()).collect();
    assert_eq!(names, ["department", "frustration", "is_urgent"]);
    let kinds: Vec<&str> = estimate.questions.iter().map(|q| q.kind.as_str()).collect();
    assert_eq!(kinds, ["choice", "score", "noul"]);
}

#[test]
fn the_answer_a_question_asks_for_is_what_it_costs() {
    // More labels, more probabilities to send back.
    let mut small = app();
    small.exec(":choice department Which team | billing=Payments | technical=Bugs");
    let mut big = app();
    big.exec(
        ":choice department Which team | billing=Payments | technical=Bugs | sales=Pricing | legal=Contracts",
    );
    assert!(
        cost::estimate(&big.session, "jev-latest").output_tokens
            > cost::estimate(&small.session, "jev-latest").output_tokens
    );

    // A score echoes its legend; a noul is one number.
    let mut app = app();
    app.exec(":noul is_urgent The message conveys urgency");
    app.exec(":score frustration How frustrated | Calm | Annoyed | Furious");
    let estimate = cost::estimate(&app.session, "jev-latest");
    let noul = &estimate.questions[0];
    let score = &estimate.questions[1];
    assert!(noul.output_tokens < score.output_tokens);
    assert!(!noul.assumed);
}

#[test]
fn an_unmodelled_question_gets_an_assumed_answer() {
    let mut app = app();
    app.exec(r#":raw tone {"type": "tone", "instructions": "Polite?"}"#);
    let estimate = cost::estimate(&app.session, "jev-latest");
    assert_eq!(estimate.questions[0].kind, "tone");
    assert!(estimate.questions[0].assumed);
    assert!(estimate.questions[0].output_tokens > 0);
}

#[test]
fn a_longer_state_costs_more() {
    let mut app = app();
    app.exec(":noul is_urgent The message conveys urgency");
    let short = cost::estimate(&app.session, "jev-latest").input_tokens;
    app.exec("The payout failed again, third time this month, and nobody has replied yet.");
    assert!(cost::estimate(&app.session, "jev-latest").input_tokens > short);
}

#[test]
fn rates_take_the_usual_separators_and_refuse_the_rest() {
    for text in ["0.20/1.00", "0.20 1.00", "$0.20, $1.00", " 0.20 / 1.00 "] {
        assert_eq!(cost::parse_rates(text), Ok(RATES), "{text}");
    }
    for text in ["", "0.20", "cheap/free", "1/2/3", "-1/2"] {
        assert!(cost::parse_rates(text).is_err(), "{text}");
    }
    assert_eq!(cost::rates_from_str("0.20/1.00"), Some(RATES));
    assert_eq!(cost::rates_from_str(""), None);
    assert_eq!(cost::rates_from_str("free"), None);
    // The environment takes the form the REPL prints.
    assert_eq!(cost::rates_value(RATES), "0.20/1.00");
    assert_eq!(cost::rates_from_str(&cost::rates_value(RATES)), Some(RATES));
    assert_eq!(cost::format_rates(RATES), "$0.20/$1.00 per Mtok");
}

#[test]
fn tokens_are_priced_per_million() {
    let priced = cost::price(1_000_000, 500_000, RATES);
    assert!((priced.input - 0.2).abs() < 1e-10);
    assert!((priced.output - 0.5).abs() < 1e-10);
    assert!((priced.total - 0.7).abs() < 1e-10);

    // What a call reported, and nothing when it reported nothing. `Usage` is non-exhaustive, so
    // it is built the way a response builds it: by deserializing the wire shape.
    let usage = |input: Value, output: Value| -> Usage {
        serde_json::from_value(json!({"input_tokens": input, "output_tokens": output})).unwrap()
    };
    let counted = cost::price_usage(&usage(json!(1_000_000), json!(0)), RATES).unwrap();
    assert!((counted.total - 0.2).abs() < 1e-10);
    assert!(cost::price_usage(&usage(json!(10), Value::Null), RATES).is_none());
    assert!(cost::price_usage(&usage(Value::Null, Value::Null), RATES).is_none());
}

#[test]
fn small_money_keeps_its_digits() {
    assert_eq!(cost::usd(0.0), "$0");
    assert_eq!(cost::usd(0.000_000_2), "<$0.000001");
    assert_eq!(cost::usd(0.000232), "$0.000232");
    assert_eq!(cost::usd(0.2316), "$0.2316");
    assert_eq!(cost::usd(12.5), "$12.50");
}

#[test]
fn cost_counts_tokens_before_any_rates_are_set() {
    let mut app = app();
    app.exec(":preset triage");
    app.transcript.clear();
    app.exec(":cost");
    let text = transcript(&app);
    assert!(text.contains("cost estimate"), "{text}");
    assert!(text.contains("department"), "{text}");
    assert!(text.contains("no rates set"), "{text}");
    assert!(!text.contains("per 1,000 calls"), "{text}");
}

#[test]
fn cost_prices_the_session_once_it_has_rates() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":cost 0.20/1.00");
    assert_eq!(app.rates, Some(RATES));
    let text = transcript(&app);
    assert!(text.contains("per 1,000 calls"), "{text}");
    assert!(text.contains("JEV_PRICE=0.20/1.00"), "{text}");

    // A pair it cannot read changes nothing.
    app.transcript.clear();
    app.exec(":cost gratis");
    assert_eq!(app.rates, Some(RATES));
    assert!(
        transcript(&app).contains("dollars per million tokens"),
        "{}",
        transcript(&app)
    );

    app.transcript.clear();
    app.exec(":cost off");
    assert_eq!(app.rates, None);
    assert!(
        transcript(&app).contains("rates cleared"),
        "{}",
        transcript(&app)
    );
}

#[test]
fn cost_says_when_there_is_nothing_to_price() {
    let mut app = app();
    app.transcript.clear();
    app.exec(":cost");
    assert!(
        transcript(&app).contains("nothing to price yet"),
        "{}",
        transcript(&app)
    );
}

#[test]
fn simulated_answers_carry_the_estimate() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":cost 0.20/1.00");
    app.transcript.clear();
    app.exec(":ask");
    let text = transcript(&app);
    assert!(text.contains("out tokens"), "{text}");
    assert!(text.contains("estimated, since nothing was sent"), "{text}");
}

#[test]
fn the_panel_and_the_sketch_preview_show_it_too() {
    let mut app = app();
    app.exec(":preset triage");
    let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    assert!(terminal.backend().to_string().contains("cost"));

    let mut editor = Editor::new("A payout failed.\n---\nis_urgent? The message conveys urgency");
    editor.preview = Preview::Cost;
    app.sketch = Some(editor);
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("tokens per call"), "{screen}");
}

#[test]
fn the_table_totals_what_it_lists() {
    let mut app = app();
    app.exec(":preset triage");
    let estimate = cost::estimate(&app.session, "jev-latest");
    let text = cost_lines(&estimate, Some(RATES), "unused", None)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("state"), "{text}");
    assert!(text.contains("envelope"), "{text}");
    assert!(text.contains(&estimate.input_tokens.to_string()), "{text}");
    assert!(text.contains(&estimate.output_tokens.to_string()), "{text}");
    assert!(text.contains("per call"), "{text}");
}

/// A thread of three turns, built the way it is typed: one `:turn` at a time.
fn thread_app() -> App {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":state clear");
    app.exec(":turn customer: The payout failed again, third time this month.");
    app.exec(":turn agent: Sorry about that — can you confirm the last four digits?");
    app.exec(":turn customer: I have sent them twice already. I want a refund now.");
    app
}

#[test]
fn a_thread_is_one_call_per_turn_over_a_longer_state() {
    let app = thread_app();
    let thread = cost::thread(&app.session, "jev-latest").expect("a conversation");
    assert_eq!(thread.turns, 3);
    let inputs: Vec<usize> = thread.calls.iter().map(|c| c.input_tokens).collect();
    assert_eq!(inputs.len(), 3);
    assert!(inputs[0] < inputs[1], "{inputs:?}");
    assert!(inputs[1] < inputs[2], "{inputs:?}");
    assert_eq!(thread.input_tokens, inputs.iter().sum::<usize>());
}

#[test]
fn a_thread_costs_more_than_its_last_call_alone() {
    let app = thread_app();
    let one = cost::estimate(&app.session, "jev-latest");
    let whole = cost::thread(&app.session, "jev-latest").expect("a conversation");
    assert!(whole.input_tokens > one.input_tokens);
    assert_eq!(whole.calls[2].input_tokens, one.input_tokens);
}

#[test]
fn a_state_that_is_not_a_conversation_has_no_thread() {
    let mut app = app();
    app.exec(":preset triage");
    assert!(cost::thread(&app.session, "jev-latest").is_none());
}

#[test]
fn the_table_counts_the_turns_and_totals_the_thread() {
    let mut app = thread_app();
    app.rates = Some(RATES);
    app.transcript.clear();
    app.exec(":cost");
    let text = transcript(&app);
    assert!(text.contains("3 turns"), "{text}");
    assert!(text.contains("asked after every turn: 3 calls"), "{text}");
    assert!(text.contains("tokens for the thread"), "{text}");
}

#[test]
fn the_table_is_unchanged_when_the_state_is_one_message() {
    let mut app = app();
    app.exec(":preset triage");
    app.transcript.clear();
    app.exec(":cost");
    assert!(!transcript(&app).contains("asked after every turn"));
}
