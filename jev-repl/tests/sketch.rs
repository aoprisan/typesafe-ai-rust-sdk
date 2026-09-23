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

// ---- a question's bar -------------------------------------------------------------------------

const BARRED: &str = "A payout failed.
---
# thresholds were calibrated on 40 tickets
is_urgent? The message conveys urgency
  yes: A deadline
  @threshold 0.62

department: Which team should handle this
  @confidence 0.7
  billing = Payment or subscription issues
  technical = Bugs or integration problems

frustration: How frustrated the customer appears
  Calm < Annoyed < Furious
";

fn bars(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
    pairs.iter().map(|(n, b)| ((*n).to_owned(), *b)).collect()
}

#[test]
fn reads_threshold_under_a_noul_and_confidence_under_a_choice_wherever_in_the_block() {
    let parsed = sketch::parse(BARRED);
    assert_eq!(parsed.problems, vec![]);
    assert_eq!(
        parsed.bars,
        bars(&[("is_urgent", 0.62), ("department", 0.7)])
    );
    assert_eq!(parsed.tags[5], Tag::Bar);
    assert_eq!(parsed.tags[8], Tag::Bar);
    assert_eq!(Tag::Bar.label(), "bar");
    assert_eq!(Tag::Bar.color(), ratatui::style::Color::LightBlue);
    let span = |head, last, bar| sketch::BlockSpan { head, last, bar };
    assert_eq!(parsed.block("is_urgent"), Some(span(3, 5, Some(5))));
    assert_eq!(parsed.block("department"), Some(span(7, 10, Some(8))));
    assert_eq!(parsed.block("frustration"), Some(span(12, 13, None)));
}

#[test]
fn keeps_the_bar_off_the_wire_and_on_the_page() {
    let session = sketch::parse(BARRED).to_session();
    assert!(!session.request_json("x").contains("0.62"));
    let page = sketch::render(&session);
    assert!(
        page.contains("  yes: A deadline\n  @threshold 0.62\n"),
        "{page}"
    );
    assert!(
        page.contains("  technical = Bugs or integration problems\n  @confidence 0.7\n"),
        "{page}"
    );
    let again = sketch::parse(&page);
    assert_eq!(again.bars, session.bars);
    assert_eq!(sketch::render(&again.to_session()), page);
}

#[test]
fn says_what_is_wrong_with_a_bar_on_its_line() {
    let problem = |text: &str| {
        sketch::parse(text)
            .problems
            .first()
            .map(|p| p.message.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        problem("s\n---\nq? x\n  @threshold 1.5\n"),
        "`@threshold` takes a number from 0 to 1, e.g. `@threshold 0.6`"
    );
    assert!(problem("s\n---\nq? x\n  @threshold 1e-1\n").contains("takes a number from 0 to 1"));
    assert_eq!(
        problem("s\n---\nq: x\n  a = 1\n  b = 2\n  @confidence\n"),
        "`@confidence` takes a number from 0 to 1, e.g. `@confidence 0.6`"
    );
    assert_eq!(
        problem("s\n---\n@threshold 0.5\nq? x\n"),
        "`@threshold` belongs under a question — put it below the `name?` line it sets"
    );
    assert_eq!(
        problem("s\n---\n@confidence 0.5\nq? x\n"),
        "`@confidence` belongs under a question — put it below the choice or score it gates"
    );
    assert_eq!(
        problem("s\n---\nq? x\n  @confidence 0.5\n"),
        "a yes/no question takes `@threshold`, not `@confidence`"
    );
    assert_eq!(
        problem("s\n---\nq: x\n  a < b\n  @threshold 0.5\n"),
        "a choice or a score takes `@confidence`, not `@threshold`"
    );
    assert_eq!(
        problem("s\n---\nq! {\"type\": \"noul\"}\n@threshold 0.5\n"),
        "a raw question takes no bar — jev cannot read its answer"
    );
    assert_eq!(
        problem("s\n---\nq? x\n  @threshold 0.5\n  @threshold 0.6\n"),
        "`q` already has a bar on line 4"
    );
    assert_eq!(
        problem("s\n---\n@speed fast\n"),
        "unknown directive `@speed`; there is `@model`, and `@threshold` or `@confidence` under a question"
    );
    let broken = sketch::parse("s\n---\nq:\n  @confidence 0.5\n");
    assert_eq!(broken.tags[3], Tag::Bar);
    assert_eq!(
        broken.problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn reads_a_bar_only_as_a_plain_decimal() {
    for good in ["0", "1", "0.5", ".5", "1.", "0.625", "1.0"] {
        assert!(sketch::parse_bar(good).is_some(), "{good}");
    }
    for bad in [
        "", "1e-1", "0x1", "+0.5", ".5.", "-0", "1.5", ".", " 0.5", "0,5", "inf",
    ] {
        assert_eq!(sketch::parse_bar(bad), None, "{bad}");
    }
    assert_eq!(sketch::parse_bar(".25"), Some(0.25));
}

#[test]
fn writes_bars_back_without_touching_anything_else_on_the_page() {
    let written = sketch::set_bars(
        BARRED,
        &bars(&[("is_urgent", 0.6), ("frustration", 0.55), ("nobody", 0.5)]),
    );
    assert_eq!(
        written,
        BARRED.replace("@threshold 0.62", "@threshold 0.6").replace(
            "  Calm < Annoyed < Furious\n",
            "  Calm < Annoyed < Furious\n  @confidence 0.55\n",
        )
    );
    let inline = sketch::set_bars(
        "s\n---\nq? x | yes: y\nr: z\n    a = 1\n    b = 2",
        &bars(&[("q", 0.3), ("r", 0.8)]),
    );
    assert_eq!(
        inline,
        "s\n---\nq? x | yes: y\n  @threshold 0.3\nr: z\n    a = 1\n    b = 2\n    @confidence 0.8"
    );
    let crlf = sketch::set_bars("s\r\n---\r\nq? x\r\n", &bars(&[("q", 0.4)]));
    assert_eq!(crlf, "s\r\n---\r\nq? x\r\n  @threshold 0.4\r\n");
}

#[test]
fn moves_with_its_question_and_goes_when_the_question_does() {
    let mut session = sketch::parse(BARRED).to_session();
    assert_eq!(session.threshold_of("is_urgent", 0.5), 0.62);
    assert_eq!(session.threshold_of("department", 0.5), 0.5);
    assert_eq!(session.threshold_of("frustration", 0.5), 0.5);
    let mut copy = session.clone();
    copy.remove("is_urgent");
    assert_eq!(copy.bar("is_urgent"), None);
    assert_eq!(session.bar("is_urgent"), Some(0.62));
    let department = session.questions[1].1.clone();
    session.insert("department".to_owned(), department);
    assert_eq!(session.bar("department"), Some(0.7));
    let urgent = session.questions[0].1.clone();
    session.insert("department".to_owned(), urgent);
    assert_eq!(session.bar("department"), None);
}

#[test]
fn survives_save_and_open_and_the_answers_are_read_at_it() {
    let dir = std::env::temp_dir().join(format!("jev-bar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("triage.jev");
    let path = path.to_str().unwrap().to_owned();
    std::fs::write(&path, BARRED).unwrap();
    let mut app = app();
    app.exec(&format!(":open {path}"));
    assert_eq!(app.session.bar("is_urgent"), Some(0.62));
    app.exec(&format!(":save {path}"));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("@threshold 0.62")
    );
    app.exec(":ask");
    let text = transcript(&app);
    assert!(text.contains("at threshold 0.62"), "{text}");
    assert!(!text.contains("at threshold 0.50"), "{text}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn counts_a_changed_bar_as_a_change_when_a_page_is_applied() {
    let mut app = app();
    app.exec(":preset triage");
    key(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    let ed = app.sketch.as_mut().expect("sketch open");
    let at = ed
        .lines
        .iter()
        .position(|l| l.starts_with("is_urgent?"))
        .expect("the noul");
    ed.lines.insert(at + 1, "  @threshold 0.8".to_owned());
    key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(app.session.bar("is_urgent"), Some(0.8));
    assert!(!transcript(&app).contains("nothing changed."));
}
