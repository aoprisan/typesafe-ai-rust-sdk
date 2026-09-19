//! The REPL driven without a terminal: commands in, transcript and session out.

use jev_repl::app::{App, Msg};
use jev_repl::builder::{Builder, Outcome};
use jev_repl::{codegen, highlight, mock, session, ui, wrap};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn app() -> App {
    // Tests never talk to the API: without a key the app starts in mock mode, and with one in the
    // environment we force mock so the suite stays offline either way.
    let (tx, _rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(tx);
    app.mock = true;
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

#[test]
fn bare_text_becomes_the_state() {
    let mut app = app();
    app.exec("The payout failed again, this is the third time.");
    assert_eq!(
        app.session.state,
        json!("The payout failed again, this is the third time.")
    );
    assert!(!app.session.state_is_empty());
}

#[test]
fn preset_builds_a_full_session() {
    let mut app = app();
    app.exec(":preset triage");
    let names: Vec<&str> = app
        .session
        .questions
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["department", "frustration", "is_urgent"]);

    let body: Value = serde_json::from_str(&app.session.request_json("jev-latest")).unwrap();
    assert_eq!(body["questions"]["department"]["type"], "choice");
    assert_eq!(body["questions"]["frustration"]["type"], "score");
    assert_eq!(body["questions"]["is_urgent"]["type"], "noul");
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(
        body["questions"]["frustration"]["criteria"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn question_commands_parse_criteria() {
    let (name, q) =
        session::parse_noul("is_urgent Conveys urgency | yes: A deadline | no: Routine")
            .expect("parses");
    assert_eq!(name, "is_urgent");
    let v = serde_json::to_value(&q).unwrap();
    assert_eq!(v["type"], "noul");
    assert_eq!(v["criteria"]["true"], "A deadline");
    assert_eq!(v["criteria"]["false"], "Routine");

    let (_, q) = session::parse_choice("dept Which team | billing=Payments | tech").unwrap();
    let v = serde_json::to_value(&q).unwrap();
    assert_eq!(v["criteria"]["billing"], "Payments");
    assert_eq!(v["criteria"]["tech"], Value::Null);

    // Structured instructions stay JSON rather than becoming a string.
    let (_, q) = session::parse_score(r#"tone {"task": "rate"} | low | high"#).unwrap();
    let v = serde_json::to_value(&q).unwrap();
    assert_eq!(v["instructions"]["task"], "rate");

    assert!(session::parse_choice("dept Which team | only_one").is_err());
    assert!(session::parse_score("tone Rate it | just_one").is_err());
    assert!(session::parse_noul("nameonly").is_err());
}

#[test]
fn asking_in_mock_mode_answers_every_question() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":ask");

    let raw: Value = serde_json::from_str(app.last_raw.as_ref().expect("a body")).unwrap();
    let answers = raw["answers"].as_object().unwrap();
    assert_eq!(answers.len(), 3);
    let p = answers["is_urgent"]["noul"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&p), "noul out of range: {p}");
    let sum: f64 = answers["department"]["probabilities"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_f64().unwrap())
        .sum();
    assert!((sum - 1.0).abs() < 1e-6, "probabilities sum to {sum}");

    let text = transcript(&app);
    assert!(text.contains("answers"), "{text}");
    assert!(text.contains("department"));
}

#[test]
fn mock_answers_are_deterministic() {
    let state = json!("a ticket about billing");
    let q = json!({"type": "noul", "instructions": "urgent?"});
    let first = mock::answer(&state, "urgent", &q);
    let second = mock::answer(&state, "urgent", &q);
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert!(mock::answer(&state, "x", &json!({"type": "mystery"})).is_none());

    // Pinned, because the TypeScript port simulates from the same FNV-1a seed and the two are
    // meant to answer a page identically. The same vectors are asserted there.
    let noul = mock::answer(&state, "urgent", &q).map(|a| mock::answer_json(&a));
    assert_eq!(noul, Some(json!({"type": "noul", "noul": 0.742})));
    let choice = mock::answer(
        &state,
        "team",
        &json!({"type": "choice", "instructions": "which team",
                "criteria": {"billing": "money", "technical": "bugs"}}),
    )
    .map(|a| mock::answer_json(&a));
    assert_eq!(
        choice,
        Some(json!({
            "type": "choice",
            "choice": "billing",
            "probabilities": {"billing": 0.599, "technical": 0.401},
            "confidence": 0.198
        }))
    );
}

#[test]
fn asking_without_questions_is_refused() {
    let mut app = app();
    app.exec("some state");
    app.exec(":ask");
    assert!(app.last_raw.is_none());
    assert!(transcript(&app).contains("no questions yet"));
}

#[test]
fn generated_rust_reflects_the_session() {
    let mut app = app();
    app.exec(":preset triage");
    let code = codegen::rust(&app.session, "jev-2", 0.8);
    assert!(
        code.contains("use typesafe::{Choice, Client, Noul, Questions, Score};"),
        "{code}"
    );
    assert!(code.contains(".with(\"department\", Choice::new("));
    assert!(code.contains(".option(\"billing\""));
    assert!(code.contains("Score::new("));
    assert!(code.contains(".model(\"jev-2\")"));
    assert!(code.contains("is_yes(0.80)"));
    assert!(
        !code.contains("json!("),
        "no json! is needed for plain text: {code}"
    );
}

#[test]
fn saved_sessions_round_trip() {
    let mut app = app();
    app.exec(":preset moderation");
    let body = app.session.request_json("jev-latest");
    let reopened = session::from_body(&body).expect("reopens");
    assert_eq!(reopened.questions.len(), app.session.questions.len());
    assert_eq!(
        reopened.request_json("jev-latest"),
        app.session.request_json("jev-latest")
    );
}

#[test]
fn threshold_changes_what_counts_as_yes() {
    let mut app = app();
    app.exec(":threshold 0.9");
    assert_eq!(app.threshold, 0.9);
    app.exec(":threshold 4");
    assert_eq!(app.threshold, 0.9, "out-of-range thresholds are rejected");
}

#[test]
fn builder_mode_produces_the_same_question_as_the_command() {
    // A name was given, so the form opens on the type field.
    let mut b = Builder::new(String::new(), "department");
    press(&mut b, KeyCode::Char('c')); // noul → choice
    press(&mut b, KeyCode::Tab);
    type_text(&mut b, "Which team should handle this");
    press(&mut b, KeyCode::Tab);
    type_text(&mut b, "billing");
    press(&mut b, KeyCode::Tab);
    type_text(&mut b, "Payment or subscription issues");
    press(&mut b, KeyCode::Tab);
    type_text(&mut b, "technical");

    assert_eq!(
        b.as_command(),
        ":choice department Which team should handle this | billing=Payment or subscription issues | technical"
    );
    let preview = b.preview();
    assert_eq!(preview["department"]["type"], "choice");
    assert_eq!(
        preview["department"]["criteria"]["billing"],
        "Payment or subscription issues"
    );

    match b.key(ctrl(KeyCode::Char('s'))) {
        Outcome::Commit(name, question, _) => {
            assert_eq!(name, "department");
            let v = serde_json::to_value(&*question).unwrap();
            assert_eq!(v["criteria"]["technical"], Value::Null);
        }
        _ => panic!("Ctrl-S should commit a complete question"),
    }
}

#[test]
fn builder_mode_refuses_an_incomplete_question() {
    let mut b = Builder::new(String::new(), "");
    assert!(matches!(b.key(ctrl(KeyCode::Char('s'))), Outcome::Open));
    assert!(b.message.as_deref().unwrap().contains("name"));

    let mut b = Builder::new(String::new(), "tone");
    press(&mut b, KeyCode::Char('s')); // score
    press(&mut b, KeyCode::Tab);
    type_text(&mut b, "How warm the reply is");
    assert!(matches!(b.key(ctrl(KeyCode::Char('s'))), Outcome::Open));
    assert!(b.message.as_deref().unwrap().contains("two ordered levels"));
}

#[test]
fn builder_rows_can_be_added_and_dropped() {
    let mut b = Builder::new(String::new(), "tone");
    press(&mut b, KeyCode::Char('s')); // score
    let before = b.fields().len();
    b.add_row();
    assert_eq!(b.fields().len(), before + 1);
    // Rows only vanish under the cursor, so step onto one first.
    press(&mut b, KeyCode::Tab); // instructions
    press(&mut b, KeyCode::Tab); // level 0
    b.delete_row();
    assert_eq!(b.fields().len(), before);
}

#[test]
fn json_highlighting_separates_keys_from_values() {
    let spans = highlight::json_spans(r#"  "model": "jev-latest","#);
    let colored: Vec<(String, String)> = spans
        .iter()
        .filter(|s| !s.content.trim().is_empty())
        .map(|s| (s.content.to_string(), format!("{:?}", s.style.fg)))
        .collect();
    assert_eq!(colored[0].0, "\"model\"");
    assert_eq!(colored[2].0, "\"jev-latest\"");
    assert_ne!(
        colored[0].1, colored[2].1,
        "a key is not styled as a string"
    );
}

#[test]
fn command_highlighting_flags_unknown_commands() {
    let known = |c: &str| c == ":choice";
    let good = highlight::command(":choice dept Pick | a=1 | b=2", known);
    assert_eq!(good[0].content, ":choice");
    let bad = highlight::command(":nope x", known);
    assert_ne!(bad[0].style.fg, good[0].style.fg);
}

#[test]
fn long_lines_wrap_to_the_pane() {
    let text = "word ".repeat(40);
    let wrapped = wrap::wrap(&Line::from(text), 20);
    assert!(wrapped.len() > 1);
    for line in &wrapped {
        let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(width <= 20, "line of {width} columns in a 20-column pane");
    }
}

#[test]
fn the_whole_screen_renders() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":ask");
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("jev"));
    assert!(screen.contains("MOCK"));
    assert!(screen.contains("department"));

    // …and again with builder mode open over it.
    app.exec(":build sentiment");
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("build a question"), "{screen}");
    assert!(screen.contains("sentiment"));
}

#[test]
fn every_lesson_suggests_a_command_the_repl_accepts() {
    for lesson in jev_repl::lessons::LESSONS {
        let mut app = app();
        app.exec(":preset triage"); // so :ask-style suggestions have something to work with
        app.exec(lesson.try_this);
        let text = transcript(&app);
        assert!(
            !text.contains("unknown command"),
            "lesson {:?} suggests something the REPL rejects:\n{text}",
            lesson.title
        );
    }
}

#[test]
fn every_preset_loads() {
    for preset in jev_repl::presets::PRESETS {
        let mut app = app();
        app.exec(&format!(":preset {}", preset.name));
        assert!(
            app.session.questions.len() >= 3,
            "preset {} loaded {} questions",
            preset.name,
            app.session.questions.len()
        );
        assert!(
            !app.session.state_is_empty(),
            "preset {} has no state",
            preset.name
        );
        app.exec(":ask");
        assert!(
            app.last_raw.is_some(),
            "preset {} produced no answers",
            preset.name
        );
    }
}

fn press(b: &mut Builder, code: KeyCode) {
    b.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(b: &mut Builder, text: &str) {
    for c in text.chars() {
        press(b, KeyCode::Char(c));
    }
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

#[test]
fn an_api_key_is_never_echoed_or_remembered() {
    let mut app = app();
    // Typed at the prompt, the way a user would.
    for c in ":key sk-not-a-real-key".chars() {
        app.handle(Msg::Term(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        ))));
    }
    app.handle(Msg::Term(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    ))));
    let text = transcript(&app);
    assert!(!text.contains("sk-not-a-real-key"), "{text}");
    assert!(
        text.contains("…-key"),
        "the tail is enough to tell keys apart: {text}"
    );
}
