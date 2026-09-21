//! The REPL driven without a terminal: commands in, transcript and session out.

use jev_repl::app::{App, Msg};
use jev_repl::builder::{Builder, Field, Kind, Outcome};
use jev_repl::{codegen, highlight, mock, session, sketch, ui, wrap};
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
fn builder_keeps_the_type_for_the_next_question_of_the_same_shape() {
    let mut app = app();
    app.exec(":state A ticket");
    key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert_eq!(
        app.builder.as_ref().map(|b| b.kind),
        Some(Kind::Noul),
        "a fresh builder opens on a noul"
    );

    build_choice(&mut app, "department", true);
    assert_eq!(names(&app), ["department"]);
    // The form stays open on `choice`, so a second choice costs no keystrokes.
    let b = app.builder.as_ref().expect("still in the builder");
    assert_eq!(b.kind, Kind::Choice);
    assert_eq!(b.existing, ["department"]);
    assert!(transcript(&app).contains("type still `choice`"));

    build_choice(&mut app, "owner", false);
    assert_eq!(names(&app), ["department", "owner"]);
}

#[test]
fn builder_asks_before_a_new_question_replaces_one() {
    let mut b = Builder::new("A ticket".into(), "")
        .of_kind(Kind::Noul)
        .over(vec!["is_urgent".into()]);
    type_text(&mut b, "is_urgent");
    press(&mut b, KeyCode::Tab); // type
    press(&mut b, KeyCode::Tab); // instructions
    type_text(&mut b, "Conveys urgency");
    assert!(matches!(b.key(ctrl(KeyCode::Char('s'))), Outcome::Open));
    assert!(b.message.as_deref().unwrap().contains("already a question"));
    // Saying it again means it.
    assert!(matches!(
        b.key(ctrl(KeyCode::Char('s'))),
        Outcome::Commit(..)
    ));

    // Renaming disarms the warning, so the next one is added rather than replaced.
    let mut c = Builder::new("A ticket".into(), "").over(vec!["is_urgent".into()]);
    type_text(&mut c, "is_urgent");
    press(&mut c, KeyCode::Tab);
    press(&mut c, KeyCode::Tab);
    type_text(&mut c, "Conveys urgency");
    assert!(matches!(c.key(ctrl(KeyCode::Char('s'))), Outcome::Open));
    press(&mut c, KeyCode::Up);
    press(&mut c, KeyCode::Up);
    type_text(&mut c, "_too");
    match c.key(ctrl(KeyCode::Char('s'))) {
        Outcome::Commit(name, ..) => assert_eq!(name, "is_urgent_too"),
        _ => panic!("a renamed question is added, not held back"),
    }
}

#[test]
fn builder_crosses_a_word_with_alt_arrow() {
    let mut b = Builder::new("the payout failed again".into(), "");
    assert_eq!(b.focused(), Field::Name);
    press(&mut b, KeyCode::Up); // onto the state field, cursor at its end
    assert_eq!(b.cursor, 23);
    b.key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(b.cursor, 18);
    b.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(b.cursor, 23);
    assert_eq!(b.state, "the payout failed again", "and nothing was typed");
    // The type row keeps ← → for switching the type.
    let mut typed = Builder::new(String::new(), "tone");
    assert_eq!(typed.focused(), Field::Kind);
    typed.key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(typed.kind, Kind::Score);
}

#[test]
fn alt_arrow_crosses_and_deletes_words_on_the_input_line() {
    let mut app = app();
    for c in ":noul is_urgent conveys urgency".chars() {
        key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    assert_eq!(app.cursor, 31);
    key(&mut app, KeyCode::Left, KeyModifiers::ALT);
    assert_eq!(app.cursor, 24);
    key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(
        app.cursor, 16,
        "Ctrl-← is the other spelling of the same key"
    );
    key(&mut app, KeyCode::Char('f'), KeyModifiers::ALT);
    assert_eq!(app.cursor, 23);
    key(&mut app, KeyCode::Backspace, KeyModifiers::ALT);
    assert_eq!(app.input, ":noul is_urgent  urgency");
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

/// A key straight into the app, the way the terminal thread delivers one.
fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle(Msg::Term(Event::Key(KeyEvent::new(code, modifiers))));
}

fn names(app: &App) -> Vec<String> {
    app.session
        .questions
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// Fill the app's open builder in as a two-option choice and add it.
fn build_choice(app: &mut App, name: &str, pick_choice: bool) {
    assert!(app.builder.is_some(), "the builder is not open");
    for c in name.chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(app, KeyCode::Tab, KeyModifiers::NONE); // type
    if pick_choice {
        key(app, KeyCode::Char('c'), KeyModifiers::NONE);
    }
    key(app, KeyCode::Tab, KeyModifiers::NONE); // instructions
    for c in "Which team".chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(app, KeyCode::Tab, KeyModifiers::NONE);
    for c in "billing".chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(app, KeyCode::Tab, KeyModifiers::NONE);
    key(app, KeyCode::Tab, KeyModifiers::NONE);
    for c in "technical".chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(app, KeyCode::Char('s'), KeyModifiers::CONTROL);
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

// ---- a conversation as the state ------------------------------------------------------------

#[test]
fn turns_grow_the_state_one_at_a_time() {
    let mut app = app();
    app.exec(":turn customer: The payout failed again.");
    app.exec(":turn agent: Can you confirm the last four digits?");
    assert_eq!(
        app.session.state,
        json!([
            {"who": "customer", "said": "The payout failed again."},
            {"who": "agent", "said": "Can you confirm the last four digits?"},
        ])
    );
    assert_eq!(app.session.turns().map(|t| t.len()), Some(2));
}

#[test]
fn a_speaker_is_named_only_when_the_first_word_ends_in_a_colon() {
    let turn = session::parse_turn("customer: I want a refund").expect("a turn");
    assert_eq!(turn.who.as_deref(), Some("customer"));
    assert_eq!(turn.said, "I want a refund");

    let bare = session::parse_turn("I want a refund now").expect("a turn");
    assert_eq!(bare.who, None);
    assert_eq!(bare.said, "I want a refund now");

    assert!(session::parse_turn("customer:").is_err());
    assert!(session::parse_turn("   ").is_err());
}

#[test]
fn the_text_already_in_the_state_becomes_the_first_turn() {
    let mut app = app();
    app.exec("The payout failed again.");
    app.exec(":turn agent: We are looking into it.");
    assert_eq!(
        app.session.state,
        json!([
            {"said": "The payout failed again."},
            {"who": "agent", "said": "We are looking into it."},
        ])
    );
    assert!(transcript(&app).contains("the state you had became the first turn"));
}

#[test]
fn a_state_that_is_not_a_conversation_is_refused_not_reshaped() {
    let mut app = app();
    app.exec(r#":state json {"ticket": 1}"#);
    app.exec(":turn agent: We are looking into it.");
    assert_eq!(app.session.state, json!({"ticket": 1}));
    assert!(transcript(&app).contains("not a conversation"));
}

#[test]
fn dropping_takes_the_last_turn_back_and_the_last_of_all_empties_the_state() {
    let mut app = app();
    app.exec(":turn customer: The payout failed again.");
    app.exec(":turn agent: We are looking into it.");
    app.exec(":turn drop");
    assert_eq!(
        app.session.state,
        json!([{"who": "customer", "said": "The payout failed again."}])
    );
    app.exec(":turn drop");
    assert!(app.session.state_is_empty());
    app.transcript.clear();
    app.exec(":turn drop");
    assert!(transcript(&app).contains("no turns to drop"));
}

#[test]
fn a_transcript_written_with_chat_api_keys_reads_as_turns() {
    let mut app = app();
    app.exec(r#":state json [{"role": "user", "content": "Refund me"}]"#);
    let turns = app.session.turns().expect("a conversation");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].who.as_deref(), Some("user"));
    assert_eq!(turns[0].said, "Refund me");
    app.exec(":turn agent: Looking into it.");
    assert_eq!(app.session.turns().map(|t| t.len()), Some(2));
}

#[test]
fn the_questions_stay_fixed_so_one_rubric_reads_the_whole_thread() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":state clear");
    let before: Vec<String> = app
        .session
        .questions
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    app.exec(":turn customer: The payout failed again.");
    app.exec(":turn customer: I want a refund now.");
    let after: Vec<String> = app
        .session
        .questions
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    assert_eq!(after, before);

    let body: Value = serde_json::from_str(&app.session.request_json("jev-latest")).unwrap();
    assert!(body["state"].is_array(), "{body}");
    let asked: Vec<&String> = body["questions"]
        .as_object()
        .expect("the questions")
        .keys()
        .collect();
    assert_eq!(asked, before.iter().collect::<Vec<_>>());
}

#[test]
fn the_state_command_lists_the_turns_instead_of_raw_json() {
    let mut app = app();
    app.exec(":turn customer: The payout failed again.");
    app.transcript.clear();
    app.exec(":state");
    let text = transcript(&app);
    assert!(text.contains("conversation (1 turn)"), "{text}");
    assert!(text.contains("The payout failed again."), "{text}");
}

#[test]
fn a_conversation_survives_a_round_trip_through_a_sketch_page() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":turn agent: Have you tried another browser?");
    let page = sketch::render(&app.session);
    let back = sketch::parse(&page).to_session();
    assert_eq!(back.turns(), app.session.turns());
}

#[test]
fn the_panel_preview_counts_the_turns() {
    let mut app = app();
    app.exec(":turn customer: The payout failed again.");
    app.exec(":turn agent: Looking into it.");
    assert_eq!(
        app.session.state_preview(),
        "2 turns · agent: Looking into it."
    );
}
