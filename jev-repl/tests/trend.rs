//! Following a rubric across a conversation: the prefixes, the series and the lines.

use jev_repl::app::{App, Msg};
use jev_repl::headless::{self, Answered};
use jev_repl::session::Session;
use jev_repl::trend::{self, Series};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use typesafe::Answer;

const PAGE: &str = r#"[{"who": "customer", "said": "Hi"}, {"who": "agent", "said": "Hello"}, {"role": "user", "content": "It is down and we lose money"}]
---
is_urgent? The message conveys urgency
department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
frustration: How frustrated the customer appears
  Calm < Annoyed < Furious
"#;

fn session() -> Session {
    headless::load(PAGE).expect("the page parses")
}

fn noul(p: f64) -> Answer {
    Answer::Noul(serde_json::from_value(json!({"noul": p})).expect("a noul"))
}

fn choice(label: &str, probabilities: Value) -> Answer {
    Answer::Choice(
        serde_json::from_value(json!({
            "choice": label, "probabilities": probabilities, "confidence": 0.5
        }))
        .expect("a choice"),
    )
}

fn score(value: f64) -> Answer {
    Answer::Score(
        serde_json::from_value(json!({
            "score": value, "confidence": 0.5,
            "legend": {"0": "Calm", "1": "Annoyed", "2": "Furious"},
            "probabilities": {},
        }))
        .expect("a score"),
    )
}

fn per_turn() -> Vec<Vec<Answered>> {
    let turn = |urgent: f64, label: &str, billing: f64, technical: f64, level: f64| {
        vec![
            ("is_urgent".to_owned(), Some(noul(urgent))),
            (
                "department".to_owned(),
                Some(choice(
                    label,
                    json!({"billing": billing, "technical": technical}),
                )),
            ),
            ("frustration".to_owned(), Some(score(level))),
        ]
    };
    vec![
        turn(0.12, "billing", 0.6, 0.35, 0.2),
        turn(0.4, "technical", 0.45, 0.55, 0.4),
        turn(0.91, "technical", 0.2, 0.8, 1.7),
    ]
}

fn plain(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
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
fn asks_after_every_turn_the_way_the_cost_table_prices_the_thread() {
    let steps = trend::prefixes(&session());
    let states: Vec<&Value> = steps.iter().map(|s| &s.state).collect();
    assert_eq!(
        states,
        vec![
            &json!([{"who": "customer", "said": "Hi"}]),
            &json!([{"who": "customer", "said": "Hi"}, {"who": "agent", "said": "Hello"}]),
            &json!([
                {"who": "customer", "said": "Hi"},
                {"who": "agent", "said": "Hello"},
                {"who": "user", "said": "It is down and we lose money"},
            ]),
        ]
    );
    assert_eq!(steps[0].questions.len(), 3);
    let mut not_a_thread = session();
    not_a_thread.state = json!("not a thread");
    assert!(trend::prefixes(&not_a_thread).is_empty());
}

#[test]
fn follows_a_nouls_probability_and_reads_it_at_the_threshold() {
    let all = trend::series(&session(), &per_turn(), 0.5);
    assert_eq!(
        all[0],
        Series {
            name: "is_urgent".to_owned(),
            kind: "noul",
            values: vec![0.12, 0.4, 0.91],
            top: 1.0,
            readings: vec!["no".into(), "no".into(), "yes".into()],
            label: None,
        }
    );
    let mut barred = session();
    barred.set_bar("is_urgent", 0.3);
    assert_eq!(
        trend::series(&barred, &per_turn(), 0.5)[0].readings,
        vec!["no", "yes", "yes"]
    );
}

#[test]
fn follows_the_label_a_choice_ended_on_and_names_the_leader_at_every_turn() {
    let all = trend::series(&session(), &per_turn(), 0.5);
    assert_eq!(all[1].values, vec![0.35, 0.55, 0.8]);
    assert_eq!(all[1].label.as_deref(), Some("technical"));
    assert_eq!(all[1].readings, vec!["billing", "technical", "technical"]);
}

#[test]
fn follows_a_scores_weighted_level_against_its_scale() {
    let all = trend::series(&session(), &per_turn(), 0.5);
    assert_eq!(all[2].values, vec![0.2, 0.4, 1.7]);
    assert_eq!(all[2].top, 2.0);
    assert_eq!(all[2].readings, vec!["level 0", "level 0", "level 2"]);
}

#[test]
fn has_nothing_to_follow_when_a_turn_came_back_without_the_answer() {
    let mut gappy = per_turn();
    gappy[1].remove(0);
    assert!(trend::series(&session(), &gappy, 0.5)[0].values.is_empty());
}

#[test]
fn scales_the_spark_to_a_fixed_top_not_to_the_values() {
    assert_eq!(trend::sparkline(&[0.0, 0.5, 1.0], 1.0), "▁▅█");
    assert_eq!(trend::sparkline(&[0.9, 0.9], 1.0), "▇▇");
    assert_eq!(trend::sparkline(&[0.0, 1.0, 2.0], 2.0), "▁▅█");
    assert_eq!(trend::sparkline(&[-1.0, 3.0], 2.0), "▁█");
}

#[test]
fn lists_the_turns_it_changed_its_mind_or_says_it_never_did() {
    let words = |all: &[&str]| all.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        trend::changes(&words(&["no", "no", "yes", "no"])),
        "turn 3 yes · turn 4 no"
    );
    assert_eq!(trend::changes(&words(&["no", "no"])), "no throughout");
}

#[test]
fn draws_one_line_per_question() {
    let text = plain(&trend::trend_lines(&trend::series(
        &session(),
        &per_turn(),
        0.5,
    )));
    assert_eq!(
        text.split('\n').collect::<Vec<_>>(),
        vec![
            "  is_urgent    noul    ▂▄▇  0.12 → 0.91            turn 3 yes",
            "  department   choice  ▃▅▇  technical 0.35 → 0.80  turn 2 technical",
            "  frustration  score   ▂▂▇  0.20 → 1.70 of 2       turn 3 level 2",
        ]
    );
    let missing = trend::trend_lines(&[Series {
        name: "tone".to_owned(),
        kind: "raw",
        values: Vec::new(),
        top: 1.0,
        readings: Vec::new(),
        label: None,
    }]);
    assert_eq!(plain(&missing), "  tone  raw     no answer to follow");
}

fn app() -> App {
    let (tx, _rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(tx);
    app.mock = true;
    app
}

fn transcript(app: &App) -> String {
    plain(&app.transcript)
}

/// Whether some line holds `name` and `kind` and then a spark of `n` blocks.
fn has_trend(text: &str, name: &str, kind: &str, n: usize, after: impl Fn(&str) -> bool) -> bool {
    text.lines().any(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.len() > 3
            && words[0] == name
            && words[1] == kind
            && words[2].chars().count() == n
            && words[2].chars().all(|c| "▁▂▃▄▅▆▇█".contains(c))
            && after(&words[3..].join(" "))
    })
}

fn two_decimals(word: &str) -> bool {
    word.len() == 4 && word.as_bytes()[1] == b'.' && word.replace('.', "").parse::<u32>().is_ok()
}

#[test]
fn asks_after_every_turn_and_prices_every_call() {
    let mut app = app();
    app.exec(":preset triage");
    app.exec(":turn agent: Sorry — can you confirm the last four digits?");
    app.exec(":turn customer: I sent them twice already. I want a refund now.");
    app.exec(":cost 0.20/1.00");
    app.exec(":trend");
    let text = transcript(&app);
    assert!(text.contains("trend · 3 turns"), "{text}");
    let arrow = |rest: &str| {
        let w: Vec<&str> = rest.split(' ').collect();
        w.len() >= 3 && two_decimals(w[0]) && w[1] == "→" && two_decimals(w[2])
    };
    assert!(has_trend(&text, "is_urgent", "noul", 3, arrow), "{text}");
    assert!(
        has_trend(&text, "department", "choice", 3, |rest| arrow(
            rest.split_once(' ').map_or("", |(_, r)| r)
        )),
        "{text}"
    );
    assert!(
        has_trend(&text, "frustration", "score", 3, |rest| arrow(rest)
            && rest.contains(" of 2")),
        "{text}"
    );
    let cost = text
        .lines()
        .find(|l| l.contains("over 3 calls"))
        .unwrap_or_else(|| panic!("no cost line in\n{text}"));
    assert!(cost.trim_start().starts_with("≈ "), "{cost}");
    assert!(cost.contains(" out tokens · $"), "{cost}");
    assert!(
        cost.ends_with("over 3 calls — estimated, since nothing was sent."),
        "{cost}"
    );
}

#[test]
fn says_what_it_needs_when_there_is_no_conversation_to_follow() {
    let mut app = app();
    app.exec(":trend");
    assert!(transcript(&app).contains("no questions yet"));
    app.exec(":preset triage");
    app.exec(":trend");
    assert!(
        transcript(&app).contains(
            ":trend needs a conversation — `:turn <who>: <text>` builds one, a turn at a time."
        ),
        "{}",
        transcript(&app)
    );
}

#[test]
fn is_listed_in_help() {
    let mut app = app();
    app.exec(":help");
    let text = transcript(&app);
    assert!(
        text.contains(
            ":trend      every question after each turn of the conversation, as one line apiece"
        ),
        "{text}"
    );
    let turn = text.find(":turn").expect(":turn is listed");
    let trend = text.find(":trend ").expect(":trend is listed");
    assert!(turn < trend, "right after :turn");
}
