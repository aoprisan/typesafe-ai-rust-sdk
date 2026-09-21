//! Sketch mode: a page of text in, a session out, and back again.

use jev_repl::app::{App, Msg};
use jev_repl::editor::{Editor, Outcome, Preview};
use jev_repl::sketch::{self, Tag};
use jev_repl::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn app() -> App {
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

fn questions_json(parsed: &sketch::Parsed) -> Value {
    let session = parsed.to_session();
    let model = session.model.clone().unwrap_or_else(|| "jev-latest".into());
    serde_json::from_str(&session.request_json(&model)).unwrap()
}

const PAGE: &str = "\
The payout failed again, third time this month. I'm done waiting.
---
is_urgent? The message conveys urgency
  yes: A deadline or a threat to leave
  no: Routine

department: Which team should handle this
  billing = Payment or subscription issues
  - technical = Bugs or integration problems
  sales

frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry
";

#[test]
fn punctuation_decides_the_question_type() {
    let parsed = sketch::parse(PAGE);
    assert!(parsed.ok(), "{:?}", parsed.problems);
    assert_eq!(
        parsed.state,
        json!("The payout failed again, third time this month. I'm done waiting.")
    );

    let body = questions_json(&parsed);
    let q = &body["questions"];
    assert_eq!(q["is_urgent"]["type"], "noul");
    assert_eq!(
        q["is_urgent"]["criteria"]["true"],
        "A deadline or a threat to leave"
    );
    assert_eq!(q["is_urgent"]["criteria"]["false"], "Routine");
    assert_eq!(q["department"]["type"], "choice");
    assert_eq!(
        q["department"]["criteria"]["billing"],
        "Payment or subscription issues"
    );
    assert_eq!(
        q["department"]["criteria"]["technical"],
        "Bugs or integration problems"
    );
    assert_eq!(q["department"]["criteria"]["sales"], Value::Null);
    assert_eq!(q["frustration"]["type"], "score");
    assert_eq!(
        q["frustration"]["criteria"],
        json!(["Calm", "Frustrated but civil", "Very angry"])
    );

    // Questions come out in page order, and every line has a tag for the gutter.
    let names: Vec<&str> = parsed.questions.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["is_urgent", "department", "frustration"]);
    assert_eq!(parsed.tags[0], Tag::State);
    assert_eq!(parsed.tags[1], Tag::Rule);
    assert_eq!(parsed.tags[2], Tag::Noul);
    assert_eq!(parsed.tags[3], Tag::Yes);
    assert_eq!(parsed.tags[4], Tag::No);
    assert_eq!(parsed.tags[6], Tag::Choice);
    assert_eq!(parsed.tags[7], Tag::Option);
    assert_eq!(parsed.tags[11], Tag::Score);
    assert_eq!(parsed.tags[12], Tag::Level);
}

#[test]
fn parts_can_share_the_first_line() {
    let parsed = sketch::parse(
        "text\n---\ndept: Which team | billing = Payments | tech | cheap = < $10\ntone: Rate it | low < high\nok? Fine | yes: yes | no: no\n",
    );
    assert!(parsed.ok(), "{:?}", parsed.problems);
    let q = &questions_json(&parsed)["questions"];
    assert_eq!(q["dept"]["type"], "choice");
    assert_eq!(q["dept"]["criteria"]["billing"], "Payments");
    assert_eq!(q["dept"]["criteria"]["tech"], Value::Null);
    // A `<` inside a description is just text.
    assert_eq!(q["dept"]["criteria"]["cheap"], "< $10");
    assert_eq!(q["tone"]["criteria"], json!(["low", "high"]));
    assert_eq!(q["ok"]["criteria"]["true"], "yes");
}

#[test]
fn levels_can_continue_on_their_own_lines() {
    let parsed = sketch::parse(
        "text\n---\nseverity: How bad\n  Fine as written\n  < Rude but publishable\n  < Abusive, needs review\n",
    );
    assert!(parsed.ok(), "{:?}", parsed.problems);
    let q = &questions_json(&parsed)["questions"];
    assert_eq!(
        q["severity"]["criteria"],
        json!([
            "Fine as written",
            "Rude but publishable",
            "Abusive, needs review"
        ])
    );
}

#[test]
fn json_state_raw_questions_and_model_are_supported() {
    let parsed = sketch::parse(
        "{\"subject\": \"Payouts\", \"messages\": [\"…\"]}\n---\n@model jev-2\n# a comment\ntone! {\"type\": \"noul\",\n  \"instructions\": \"Polite?\"}\nrubric: {\"task\": \"rate\"} | a = 1 | b = 2\n",
    );
    assert!(parsed.ok(), "{:?}", parsed.problems);
    assert_eq!(parsed.state["subject"], "Payouts");
    assert_eq!(parsed.model.as_deref(), Some("jev-2"));
    let body = questions_json(&parsed);
    assert_eq!(body["model"], "jev-2");
    assert_eq!(body["questions"]["tone"]["type"], "noul");
    assert_eq!(body["questions"]["tone"]["instructions"], "Polite?");
    // Structured instructions stay JSON.
    assert_eq!(body["questions"]["rubric"]["instructions"]["task"], "rate");
    assert_eq!(parsed.tags[2], Tag::Model);
    assert_eq!(parsed.tags[3], Tag::Comment);
    assert_eq!(parsed.tags[4], Tag::Raw);
    assert_eq!(parsed.tags[5], Tag::Json);
}

#[test]
fn problems_point_at_the_line_and_say_what_to_do() {
    let cases: &[(&str, usize, &str)] = &[
        ("text\n---\nnot a question\n", 2, "not a question"),
        ("text\n---\ndept: Which team\n", 2, "add options"),
        (
            "text\n---\ndept: Which team\n  billing\n",
            2,
            "one option is not a choice",
        ),
        (
            "text\n---\ntone: Rate it\n  only < \n",
            2,
            "at least two levels",
        ),
        ("text\n---\nok? Fine\n  maybe = so\n", 3, "only takes `yes"),
        (
            "text\n---\nsev: How bad\n  a < b\n  maybe = so\n",
            2,
            "are mixed",
        ),
        ("text\n---\nx? one\nx? two\n", 3, "already named"),
        ("text\n---\nraw! not json\n", 2, "not valid JSON"),
        ("text\n---\nraw! {\"no\": \"type\"}\n", 2, "with a `type`"),
        ("text\n---\n@model\n", 2, "needs a name"),
        ("text\n---\n@speed fast\n", 2, "unknown directive"),
        ("text\n---\ndept:\n", 2, "needs instructions"),
        ("just text and no rule\n", 1, "no `---` yet"),
    ];
    for (page, line, expected) in cases {
        let parsed = sketch::parse(page);
        let p = parsed
            .problems
            .first()
            .unwrap_or_else(|| panic!("{page:?} should have a problem"));
        assert_eq!(p.line, *line, "{page:?}: {p:?}");
        assert!(p.message.contains(expected), "{page:?}: {p:?}");
    }
    // A broken question drops out; the good ones stay.
    let parsed = sketch::parse("text\n---\na? fine\nb: broken\nc? also fine\n");
    let names: Vec<&str> = parsed.questions.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "c"]);
    assert_eq!(parsed.problems.len(), 1);
}

#[test]
fn every_preset_survives_a_round_trip_through_the_page() {
    for preset in jev_repl::presets::PRESETS {
        let mut app = app();
        app.exec(&format!(":preset {}", preset.name));
        app.exec(":model jev-2");
        let page = sketch::render(&app.session);
        let parsed = sketch::parse(&page);
        assert!(
            parsed.ok(),
            "preset {}:\n{page}\n{:?}",
            preset.name,
            parsed.problems
        );
        assert_eq!(
            parsed.to_session().request_json("x"),
            app.session.request_json("x"),
            "preset {} changed through the page:\n{page}",
            preset.name
        );
    }
}

#[test]
fn awkward_values_are_quoted_so_they_round_trip() {
    let mut app = app();
    app.exec(":state json {\"rows\": [1, 2]}");
    app.exec(":choice op Pick | lt=a < b | pipe=x | y");
    app.exec(":score s {\"task\": \"rate\"} | a = b | c");
    app.exec(":noul n Multi\nline");
    app.exec(":raw r {\"type\": \"mystery\", \"k\": [1]}");
    let page = sketch::render(&app.session);
    let parsed = sketch::parse(&page);
    assert!(parsed.ok(), "{page}\n{:?}", parsed.problems);
    assert_eq!(
        parsed.to_session().request_json("x"),
        app.session.request_json("x"),
        "{page}"
    );
}

#[test]
fn an_empty_session_renders_an_empty_page_with_a_rule() {
    let app = app();
    let page = sketch::render(&app.session);
    assert_eq!(page, "\n---\n");
    let parsed = sketch::parse(&page);
    assert!(parsed.ok());
    assert!(parsed.questions.is_empty());
}

fn press(ed: &mut Editor, code: KeyCode) -> Outcome {
    ed.key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(ed: &mut Editor, c: char) -> Outcome {
    ed.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn type_text(ed: &mut Editor, text: &str) {
    for c in text.chars() {
        if c == '\n' {
            press(ed, KeyCode::Enter);
        } else {
            press(ed, KeyCode::Char(c));
        }
    }
}

#[test]
fn typing_a_page_builds_the_questions() {
    let mut ed = Editor::new("\n---\n");
    type_text(&mut ed, "Refund me now or I cancel.");
    press(&mut ed, KeyCode::Down);
    press(&mut ed, KeyCode::Down);
    type_text(
        &mut ed,
        "wants_refund? Asks for money back\n  yes: Explicit ask",
    );
    // The indented line above passes its indentation on.
    press(&mut ed, KeyCode::Enter);
    assert_eq!(ed.lines[ed.row], "  ");
    type_text(&mut ed, "no: Anything else");
    assert!(ed.dirty);
    let parsed = ed.parsed();
    assert!(parsed.ok(), "{:?}\n{}", parsed.problems, ed.text());
    assert_eq!(parsed.state, json!("Refund me now or I cancel."));
    let q = &questions_json(&parsed)["questions"]["wants_refund"];
    assert_eq!(q["criteria"]["true"], "Explicit ask");
    assert_eq!(q["criteria"]["false"], "Anything else");
}

#[test]
fn lines_can_be_cut_pasted_and_moved() {
    let mut ed = Editor::new("a\nb\nc");
    ctrl(&mut ed, 'x');
    assert_eq!(ed.lines, ["b", "c"]);
    press(&mut ed, KeyCode::Down);
    ctrl(&mut ed, 'u');
    assert_eq!(ed.lines, ["b", "a", "c"]);
    assert_eq!(ed.row, 2, "the cursor stays below the pasted line");
    press(&mut ed, KeyCode::Up);
    ed.key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(ed.lines, ["a", "b", "c"]);
    assert_eq!(ed.row, 0);
    // Backspace at column 0 joins with the line above; Delete at the end joins the next.
    press(&mut ed, KeyCode::Down);
    press(&mut ed, KeyCode::Backspace);
    assert_eq!(ed.lines, ["ab", "c"]);
    press(&mut ed, KeyCode::End);
    press(&mut ed, KeyCode::Delete);
    assert_eq!(ed.lines, ["abc"]);
}

#[test]
fn alt_arrow_crosses_and_deletes_words() {
    let mut ed = Editor::new("is_urgent? conveys urgency\n---");
    press(&mut ed, KeyCode::End);
    ed.key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(ed.col, 19, "onto the start of `urgency`");
    // The same keypress from a terminal that reports Alt as Meta.
    ed.key(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER));
    assert_eq!(ed.col, 11);
    ed.key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(ed.col, 18);
    // Alt-b / Alt-f are the same keys in a terminal that sends those instead.
    ed.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(ed.col, 11);
    ed.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(ed.col, 18);
    assert_eq!(
        ed.lines[0], "is_urgent? conveys urgency",
        "and nothing was typed"
    );

    // At the edges it steps to the neighbouring line, the way a plain arrow does.
    press(&mut ed, KeyCode::Home);
    ed.key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!((ed.row, ed.col), (0, 0));

    press(&mut ed, KeyCode::End);
    ed.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
    assert_eq!(ed.lines[0], "is_urgent? conveys ");
}

#[test]
fn esc_asks_before_discarding_edits_and_preview_cycles() {
    let mut ed = Editor::new("\n---\n");
    assert!(matches!(press(&mut ed, KeyCode::Esc), Outcome::Cancel));

    let mut ed = Editor::new("\n---\n");
    type_text(&mut ed, "x");
    assert!(matches!(press(&mut ed, KeyCode::Esc), Outcome::Open));
    assert!(ed.message.as_deref().unwrap().contains("Esc again"));
    // Any other key disarms it.
    type_text(&mut ed, "y");
    assert!(matches!(press(&mut ed, KeyCode::Esc), Outcome::Open));
    assert!(matches!(press(&mut ed, KeyCode::Esc), Outcome::Cancel));

    assert_eq!(ed.preview, Preview::Json);
    ctrl(&mut ed, 'p');
    assert_eq!(ed.preview, Preview::Answers);
    ctrl(&mut ed, 'p');
    assert_eq!(ed.preview, Preview::Rust);
    ctrl(&mut ed, 'p');
    assert_eq!(ed.preview, Preview::Cost);
    ctrl(&mut ed, 'p');
    assert_eq!(ed.preview, Preview::Json);
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle(Msg::Term(Event::Key(KeyEvent::new(code, modifiers))));
}

#[test]
fn applying_a_page_replaces_the_session_and_can_send_it() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":sketch");
    let ed = app.sketch.as_mut().expect("sketch open");
    // Drop everything after the rule and write a different question.
    ed.lines.truncate(2);
    ed.row = 1;
    ed.col = 3;
    press(ed, KeyCode::Enter);
    type_text(ed, "refund? The customer wants money back");
    key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(app.sketch.is_none(), "^S closes the page");
    let names: Vec<&str> = app
        .session
        .questions
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["refund"]);
    assert!(transcript(&app).contains("session ← 1 question"));

    // ^G applies and sends in one go.
    app.exec(":sketch");
    let ed = app.sketch.as_mut().unwrap();
    ed.row = ed.lines.len() - 1;
    ed.col = 0;
    type_text(ed, "\ntone: Tone | warm < cold");
    assert!(app.last_raw.is_none());
    key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert!(app.sketch.is_none());
    assert_eq!(app.session.questions.len(), 2);
    let raw: Value = serde_json::from_str(app.last_raw.as_ref().expect("sent")).unwrap();
    assert!(raw["answers"]["tone"]["score"].is_number());
}

#[test]
fn a_page_with_problems_is_not_applied() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":sketch");
    let ed = app.sketch.as_mut().unwrap();
    ed.lines.push("stray line".into());
    key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    let ed = app.sketch.as_ref().expect("still open");
    assert!(ed.message.as_deref().unwrap().contains("1 problem to fix"));
    assert_eq!(
        ed.row,
        ed.lines.len() - 1,
        "the cursor jumps to the problem"
    );
    assert_eq!(app.session.questions.len(), 3, "the session is untouched");

    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.sketch.is_none());
    assert!(transcript(&app).contains("session unchanged"));
}

#[test]
fn ctrl_k_opens_the_page_and_show_prints_it() {
    let mut app = app();
    app.exec(":preset lead");
    key(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert!(app.sketch.is_some());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.exec(":sketch show");
    let text = transcript(&app);
    assert!(
        text.contains("has_budget? The sender indicates budget"),
        "{text}"
    );
    assert!(text.contains("smb = Under 50 people"), "{text}");
    // Long level lists go one level per line, each after a `<`.
    assert!(text.contains("< Researching options"), "{text}");
}

#[test]
fn jev_files_save_and_open_as_sketches() {
    let dir = std::env::temp_dir().join(format!("jev-sketch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("triage.jev");
    let path = path.to_str().unwrap().to_owned();

    let mut app = app();
    app.exec(":preset triage");
    let before = app.session.request_json("x");
    app.exec(&format!(":save {path}"));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("department: Which team"), "{saved}");

    let mut fresh = self::app();
    fresh.exec(&format!(":open {path}"));
    assert_eq!(fresh.session.request_json("x"), before);

    std::fs::write(&path, "text\n---\nbroken line\n").unwrap();
    fresh.exec(&format!(":open {path}"));
    assert!(transcript(&fresh).contains(":3: not a question"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_page_renders_with_its_gutter_and_preview() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":sketch");
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(
        screen.contains("sketch · the request as one page"),
        "{screen}"
    );
    assert!(
        screen.contains("choice │department: Which team"),
        "{screen}"
    );
    assert!(
        screen.contains("level  │  Calm, just stating facts"),
        "{screen}"
    );
    assert!(
        screen.contains("\"type\": \"choice\""),
        "the json preview: {screen}"
    );
    assert!(
        screen.contains("state — the text or JSON"),
        "the status line: {screen}"
    );

    // A problem is marked in the gutter, listed in the preview and explained on its line.
    let ed = app.sketch.as_mut().unwrap();
    ed.lines.push("stray".into());
    ed.row = ed.lines.len() - 1;
    let mut screens = Vec::new();
    for p in [Preview::Answers, Preview::Rust, Preview::Cost] {
        app.sketch.as_mut().unwrap().preview = p;
        terminal.draw(|f| ui::render(f, &mut app)).unwrap();
        screens.push(terminal.backend().to_string());
    }
    let screen = screens.concat();
    assert!(screen.contains("?     !│stray"), "{screen}");
    assert!(screen.contains("1 problem(s)"), "{screen}");
    // The stray line landed under a noul, so that is what it is told.
    assert!(screen.contains("problems"), "{screen}");
    assert!(
        screen.contains("only takes `yes: …` and `no: …` lines"),
        "{screen}"
    );
    assert!(
        screen.contains("use typesafe::"),
        "the rust preview: {screen}"
    );
    assert!(
        screen.contains("tokens per call"),
        "the cost preview: {screen}"
    );

    // Small terminals still draw without panicking.
    let mut tiny = Terminal::new(TestBackend::new(30, 6)).unwrap();
    tiny.draw(|f| ui::render(f, &mut app)).unwrap();
}
