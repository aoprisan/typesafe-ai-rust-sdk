//! REPL state: the transcript, the input line, the session being built, and what each command does.

use std::time::Duration;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use typesafe::{Answer, Client, ListModelsResponse, Question, SystemOneResponse};

use crate::builder::{Builder, Outcome};
use crate::editor::{self, Editor};
use crate::format::*;
use crate::session::{self, Session};
use crate::{codegen, cost, highlight, lessons, mock, presets, sketch, words};

/// Everything that can move the app forward.
pub enum Msg {
    Term(Event),
    Tick,
    Answered(Box<typesafe::Result<SystemOneResponse>>, Duration),
    Models(Box<typesafe::Result<ListModelsResponse>>),
}

pub const COMMANDS: &[(&str, &str)] = &[
    (":help", "this list; :help concepts for the model itself"),
    (":lesson", "guided track — :lesson next|prev|list|<n>"),
    (
        ":try",
        "put the current lesson's command in the input line (Ctrl-T)",
    ),
    (":preset", "load a ready-made session — :preset list"),
    (
        ":state",
        "set the state — :state <text> | :state json {…} | :state clear",
    ),
    (
        ":turn",
        "grow the state into a conversation — :turn <who>: <text> | :turn list | :turn drop",
    ),
    (":noul", ":noul <name> <instructions> [| yes: …] [| no: …]"),
    (
        ":choice",
        ":choice <name> <instructions> | label=desc | label=desc",
    ),
    (":score", ":score <name> <instructions> | level | level | …"),
    (
        ":raw",
        ":raw <name> {\"type\": …} — a hand-built question object",
    ),
    (
        ":build",
        "builder mode: a form with a live JSON preview (Ctrl-B)",
    ),
    (
        ":sketch",
        "sketch mode: the whole request as one page of text (Ctrl-K) — :sketch show prints it",
    ),
    (":questions", "list what will be sent"),
    (":rm", ":rm <name> — drop one question"),
    (":reset", "empty the session"),
    (":ask", "send it (or press Enter on an empty line)"),
    (":json", "the exact request body this session POSTs"),
    (":last", "the last raw response body"),
    (
        ":cost",
        "what a call costs — :cost <in>/<out> sets dollars per million tokens",
    ),
    (":rust", "this session as a program against the SDK"),
    (
        ":threshold",
        ":threshold <0-1> — what counts as a yes for a noul",
    ),
    (":model", ":model [name] — per-session model"),
    (":models", "models the account can use"),
    (":timeout", ":timeout <seconds> — per-attempt timeout"),
    (":mock", ":mock on|off — offline simulated answers"),
    (":key", ":key [<api-key>] — API key status, or set one"),
    (
        ":save",
        ":save <path> / :open <path> — session JSON, or a sketch if the path ends in .jev",
    ),
    (":clear", "clear the transcript (Ctrl-L)"),
    (":quit", "leave (Ctrl-C)"),
];

pub struct App {
    pub session: Session,
    pub transcript: Vec<Line<'static>>,
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    hist_idx: Option<usize>,
    stash: String,
    /// Lines scrolled back from the bottom; 0 follows the tail.
    pub scroll: usize,
    pub client: Option<Client>,
    pub mock: bool,
    pub pending: bool,
    pub spinner: usize,
    pub lesson: usize,
    pub lesson_open: bool,
    pub suggested: Option<String>,
    pub threshold: f64,
    pub timeout: Option<Duration>,
    /// Dollars per million tokens, from `JEV_PRICE` or `:cost`; `None` counts tokens only.
    pub rates: Option<cost::Rates>,
    pub last_raw: Option<String>,
    /// Some while builder mode is open.
    pub builder: Option<Builder>,
    /// Some while sketch mode is open.
    pub sketch: Option<Editor>,
    pub quit: bool,
    tx: UnboundedSender<Msg>,
}

impl App {
    pub fn new(tx: UnboundedSender<Msg>) -> Self {
        let client = Client::from_env().ok();
        let mock = client.is_none();
        let mut app = Self {
            session: Session::new(),
            transcript: Vec::new(),
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist_idx: None,
            stash: String::new(),
            scroll: 0,
            client,
            mock,
            pending: false,
            spinner: 0,
            lesson: 0,
            lesson_open: false,
            suggested: None,
            threshold: 0.5,
            timeout: None,
            rates: cost::rates_from_env(),
            last_raw: None,
            builder: None,
            sketch: None,
            quit: false,
            tx,
        };
        app.banner();
        app
    }

    pub fn model_name(&self) -> String {
        self.session
            .model
            .clone()
            .or_else(|| self.client.as_ref().map(|c| c.default_model().to_owned()))
            .unwrap_or_else(|| "jev-latest".to_owned())
    }

    // ---- transcript -------------------------------------------------------------------------

    fn push(&mut self, line: Line<'static>) {
        self.transcript.push(line);
        self.scroll = 0;
    }

    fn extend(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        self.transcript.extend(lines);
        self.scroll = 0;
    }

    fn blank(&mut self) {
        match self.transcript.last() {
            None => {}
            Some(last) if last.spans.is_empty() => {}
            _ => self.push(Line::default()),
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.push(Line::from(dim(format!("  {}", text.into()))));
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.push(styled(format!("  {}", text.into()), WARN));
    }

    fn bad(&mut self, text: impl Into<String>) {
        self.push(styled(format!("  {}", text.into()), BAD));
    }

    fn heading(&mut self, text: impl Into<String>) {
        self.blank();
        self.push(Line::from(Span::styled(
            text.into(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
    }

    fn banner(&mut self) {
        self.push(Line::from(vec![
            Span::styled("jev", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            dim("  ·  a playground for TypeSafe System One questions"),
        ]));
        self.push(Line::from(dim(
            "  state in, typed answers out: noul (probability of yes), choice (one of N), score (ordered levels)",
        )));
        self.blank();
        if self.mock {
            self.push(Line::from(vec![
                Span::styled(
                    "  MOCK MODE",
                    Style::new().fg(WARN).add_modifier(Modifier::BOLD),
                ),
                dim("  no TYPESAFE_API_KEY, so answers are simulated locally."),
            ]));
            self.note("Everything else is real: the same questions, the same wire format. :key <api-key> to go live.");
        } else {
            self.note(format!("Live against {}.", self.model_name()));
        }
        self.blank();
        self.note("Press Enter on an empty line to send. :help for commands, :lesson for the guided track, :sketch to write the whole request as a page.");
        self.lesson_open = true;
        self.suggested = Some(lessons::LESSONS[0].try_this.to_owned());
    }

    // ---- events -----------------------------------------------------------------------------

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Term(Event::Key(key)) if key.kind != KeyEventKind::Release => self.key(key),
            Msg::Term(_) => {}
            Msg::Tick => self.spinner = self.spinner.wrapping_add(1),
            Msg::Answered(result, elapsed) => {
                self.pending = false;
                match *result {
                    Ok(res) => self.show_response(&res, elapsed),
                    Err(e) => {
                        self.blank();
                        self.extend(error_lines(&e));
                    }
                }
            }
            Msg::Models(result) => {
                self.pending = false;
                match *result {
                    Ok(res) => {
                        self.heading("models");
                        for m in &res.models {
                            self.push(Line::from(vec![
                                Span::raw("  "),
                                bold(m.name.clone()),
                                dim(format!("  {}  {}", m.release_date, m.description)),
                            ]));
                        }
                    }
                    Err(e) => {
                        self.blank();
                        self.extend(error_lines(&e));
                    }
                }
            }
        }
    }

    fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(key.code, KeyCode::Char('c')) && ctrl {
            self.quit = true;
            return;
        }
        if self.builder.is_some() {
            self.builder_key(key);
            return;
        }
        if self.sketch.is_some() {
            self.sketch_key(key);
            return;
        }
        if words::is_word_left(&key) {
            let chars: Vec<char> = self.input.chars().collect();
            self.cursor = words::word_left(&chars, self.cursor);
            return;
        }
        if words::is_word_right(&key) {
            let chars: Vec<char> = self.input.chars().collect();
            self.cursor = words::word_right(&chars, self.cursor);
            return;
        }
        if words::is_delete_word_left(&key) {
            self.delete_word();
            return;
        }
        match key.code {
            KeyCode::Char('c' | 'd') if ctrl => self.quit = true,
            KeyCode::Char('b') if ctrl => self.open_builder(""),
            KeyCode::Char('k') if ctrl => self.open_sketch(),
            KeyCode::Char('l') if ctrl => {
                self.transcript.clear();
                self.scroll = 0;
            }
            KeyCode::Char('t') if ctrl => self.load_suggestion(),
            KeyCode::Char('n') if ctrl => self.exec(":lesson next"),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.chars().count(),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char(c) if words::is_typed(&key) => self.insert(c),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.remove_at(self.cursor);
                }
            }
            KeyCode::Delete => self.remove_at(self.cursor),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up => self.recall(-1),
            KeyCode::Down => self.recall(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Esc => {
                if self.scroll > 0 {
                    self.scroll = 0;
                } else {
                    self.input.clear();
                    self.cursor = 0;
                }
            }
            KeyCode::Tab => self.complete(),
            KeyCode::Enter => self.submit(),
            _ => {}
        }
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.input.insert(at, c);
        self.cursor += 1;
    }

    fn remove_at(&mut self, index: usize) {
        if index < self.input.chars().count() {
            let at = self.byte_at(index);
            self.input.remove(at);
        }
    }

    fn delete_word(&mut self) {
        let mut i = self.cursor;
        let chars: Vec<char> = self.input.chars().collect();
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let keep: String = chars[..i]
            .iter()
            .chain(chars[self.cursor..].iter())
            .collect();
        self.input = keep;
        self.cursor = i;
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_index)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn recall(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.hist_idx, delta) {
            (None, -1) => {
                self.stash = std::mem::take(&mut self.input);
                Some(self.history.len() - 1)
            }
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), _) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), _) => None,
            (None, _) => None,
        };
        self.hist_idx = next;
        self.input = match next {
            Some(i) => self.history[i].clone(),
            None => std::mem::take(&mut self.stash),
        };
        self.cursor = self.input.chars().count();
    }

    fn complete(&mut self) {
        let word = self.input.trim_start();
        if !word.starts_with(':') || word.contains(' ') {
            return;
        }
        let matches: Vec<&str> = COMMANDS
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| c.starts_with(word))
            .collect();
        match matches.as_slice() {
            [only] => {
                self.input = format!("{only} ");
                self.cursor = self.input.chars().count();
            }
            [] => {}
            many => {
                let list = many.join("  ");
                self.note(list);
            }
        }
    }

    fn load_suggestion(&mut self) {
        if let Some(s) = self.suggested.clone() {
            self.input = s;
            self.cursor = self.input.chars().count();
        }
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_owned();
        self.input.clear();
        self.cursor = 0;
        self.hist_idx = None;
        if line.is_empty() {
            self.ask();
            return;
        }
        // An API key typed at the prompt is neither echoed nor kept in the history.
        let secret = line.starts_with(":key ");
        if !secret && self.history.last().map(String::as_str) != Some(line.as_str()) {
            self.history.push(line.clone());
        }
        self.blank();
        self.push(Line::from(vec![
            Span::styled("› ", Style::new().fg(ACCENT)),
            Span::raw(if secret {
                ":key ••••••••".to_owned()
            } else {
                line.clone()
            }),
        ]));
        self.exec(&line);
    }

    // ---- commands ---------------------------------------------------------------------------

    pub fn exec(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if !line.starts_with(':') {
            // Bare text is the most common thing to want: it becomes the state.
            self.set_state(Value::String(line.to_owned()));
            return;
        }
        let (cmd, args) = match line.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (line, ""),
        };
        match cmd {
            ":help" | ":h" | ":?" => self.help(args),
            ":quit" | ":q" | ":exit" => self.quit = true,
            ":clear" => {
                self.transcript.clear();
                self.scroll = 0;
            }
            ":lesson" | ":l" => self.lesson(args),
            ":try" => self.load_suggestion(),
            ":preset" => self.preset(args),
            ":state" | ":s" => self.state_cmd(args),
            ":turn" => self.turn_cmd(args),
            ":noul" => self.add(session::parse_noul(args)),
            ":choice" => self.add(session::parse_choice(args)),
            ":score" => self.add(session::parse_score(args)),
            ":raw" => self.add(session::parse_raw(args)),
            ":build" | ":b" => self.open_builder(args),
            ":sketch" | ":page" => match args {
                "show" | "print" => self.show_sketch(),
                _ => self.open_sketch(),
            },
            ":questions" | ":qs" => self.list_questions(),
            ":rm" | ":drop" => {
                if self.session.remove(args) {
                    self.note(format!("dropped {args}"));
                } else {
                    self.warn(format!("no question named {args:?}"));
                }
            }
            ":reset" => {
                self.session = Session::new();
                self.note("session emptied: no state, no questions");
            }
            ":ask" | ":send" => self.ask(),
            ":json" => self.show_request(),
            ":last" => self.show_last(),
            ":cost" | ":price" => self.cost_cmd(args),
            ":rust" => self.show_rust(),
            ":threshold" => self.threshold_cmd(args),
            ":model" => self.model_cmd(args),
            ":models" => self.models_cmd(),
            ":timeout" => self.timeout_cmd(args),
            ":mock" => self.mock_cmd(args),
            ":key" => self.key_cmd(args),
            ":save" => self.save(args),
            ":open" | ":load" => self.open(args),
            other => {
                self.warn(format!("unknown command {other}. :help lists them all."));
            }
        }
    }

    fn help(&mut self, topic: &str) {
        if topic.starts_with("concept") {
            self.heading("what jev answers");
            for (title, body) in [
                (
                    "state",
                    "The thing being judged: a string, or any JSON — a ticket, a draft reply, a diff, a row.",
                ),
                (
                    "noul",
                    "A yes/no question answered with a probability from 0 to 1. You choose the threshold; the model never does.",
                ),
                (
                    "choice",
                    "One label out of a set you define, with the probability of every label and a confidence over the spread.",
                ),
                (
                    "score",
                    "Ordered levels you define. The answer is probability-weighted, so 1.4 sits between level 1 and 2.",
                ),
                (
                    "criteria",
                    "The descriptions attached to a question — what a yes means, what each option or level means. Vague criteria are what low confidence usually means.",
                ),
                (
                    "confidence",
                    "How concentrated the distribution is. Gate automation on it and send the rest to a human.",
                ),
                (
                    "conversation",
                    "A state that is a list of turns instead of one message. The questions stay fixed and the thread grows, so the same rubric can be re-read after every reply. `:turn` builds one.",
                ),
                (
                    "names",
                    "Questions are a name → question map, and answers come back under the same names. Keep names stable across versions.",
                ),
            ] {
                self.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{title:12}"),
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(body),
                ]));
            }
            return;
        }
        self.heading("commands");
        for (cmd, about) in COMMANDS {
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{cmd:11}"), Style::new().fg(ACCENT)),
                Span::raw(" "),
                Span::raw(*about),
            ]));
        }
        self.blank();
        self.note("Bare text with no leading colon sets the state. Enter on an empty line sends.");
        self.note("Keys: Ctrl-T try the lesson's command · Ctrl-N next lesson · Ctrl-K sketch · PgUp/PgDn scroll · Ctrl-L clear · Ctrl-C quit");
        self.note(":help concepts explains noul, choice, score and confidence.");
    }

    fn lesson(&mut self, args: &str) {
        let total = lessons::LESSONS.len();
        match args {
            "list" => {
                self.heading("lessons");
                for (i, l) in lessons::LESSONS.iter().enumerate() {
                    let marker = if i == self.lesson { "▸" } else { " " };
                    self.push(Line::from(vec![
                        Span::raw(format!("  {marker} ")),
                        dim(format!("{:>2}. ", i + 1)),
                        Span::raw(l.title),
                    ]));
                }
                self.note("`:lesson 3` jumps to one.");
                return;
            }
            "next" => {
                if self.lesson_open {
                    self.lesson = (self.lesson + 1).min(total - 1);
                }
            }
            "prev" | "back" => self.lesson = self.lesson.saturating_sub(1),
            "" => {}
            n => match n.parse::<usize>() {
                Ok(n) if (1..=total).contains(&n) => self.lesson = n - 1,
                _ => {
                    self.warn(format!("lessons run 1 to {total}; try `:lesson list`."));
                    return;
                }
            },
        }
        self.lesson_open = true;
        let l = &lessons::LESSONS[self.lesson];
        self.heading(format!(
            "lesson {}/{}  ·  {}",
            self.lesson + 1,
            total,
            l.title
        ));
        for para in l.body {
            self.push(plain(format!("  {para}")));
            self.push(Line::default());
        }
        let try_this = l.try_this.to_owned();
        self.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("try ", Style::new().fg(SCORE)),
            Span::styled(
                try_this.clone(),
                Style::new().fg(SCORE).add_modifier(Modifier::BOLD),
            ),
        ]));
        self.note("Ctrl-T puts that in the input line · Ctrl-N for the next lesson");
        self.suggested = Some(try_this);
    }

    fn preset(&mut self, args: &str) {
        if args.is_empty() || args == "list" {
            self.heading("presets");
            for p in presets::PRESETS {
                self.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{:10}", p.name), Style::new().fg(ACCENT)),
                    Span::raw(p.about),
                ]));
            }
            self.note("`:preset triage` loads one; `:questions` then shows what it built.");
            return;
        }
        let Some(preset) = presets::find(args) else {
            self.warn(format!("no preset {args:?}; `:preset list` has them."));
            return;
        };
        self.session = Session::new();
        for line in preset.script {
            self.exec(line);
        }
        self.heading(format!("preset {}", preset.name));
        self.note(preset.about);
        self.note("`:ask` to send it, `:json` to see the body, `:questions` to review.");
    }

    fn state_cmd(&mut self, args: &str) {
        match args {
            "" => {
                if self.session.turns().is_some() {
                    self.show_turns();
                    return;
                }
                self.heading("state");
                if self.session.state_is_empty() {
                    self.note("empty — type any text (no colon) or `:state <text>` to set it.");
                } else {
                    let pretty = serde_json::to_string_pretty(&self.session.state)
                        .unwrap_or_else(|_| self.session.state_preview());
                    self.extend(highlight::json(&pretty));
                }
            }
            "clear" => {
                self.session.state = Value::String(String::new());
                self.note("state cleared");
            }
            _ => match args.split_once(char::is_whitespace) {
                Some(("json", rest)) => match serde_json::from_str::<Value>(rest.trim()) {
                    Ok(v) => self.set_state(v),
                    Err(e) => self.bad(format!("not valid JSON: {e}")),
                },
                _ => self.set_state(Value::String(args.to_owned())),
            },
        }
    }

    /// `:turn` — the state as a conversation.
    ///
    /// Nothing new goes on the wire: the state becomes a list of turns and grows by one each time,
    /// so the questions stay exactly as they were and Enter re-reads the whole thread. Watching a
    /// noul move across the turns is the thing this is for.
    fn turn_cmd(&mut self, args: &str) {
        let trimmed = args.trim();
        if trimmed.is_empty() || trimmed == "list" {
            self.show_turns();
            return;
        }
        if trimmed == "drop" || trimmed == "pop" {
            match self.session.drop_turn() {
                Some(dropped) => {
                    self.note(format!("dropped {}", session::turn_text(&dropped)));
                    if self.session.state_is_empty() {
                        self.note("that was the last one; the state is empty.");
                    }
                }
                None => self.warn("no turns to drop — the state is not a conversation yet."),
            }
            return;
        }
        let turn = match session::parse_turn(trimmed) {
            Ok(turn) => turn,
            Err(e) => {
                self.bad(e);
                return;
            }
        };
        // Before it is added, so the note below can say what became of the text that was there.
        let seeded = !self.session.state_is_empty() && self.session.turns().is_none();
        let turns = match self.session.add_turn(turn.clone()) {
            Ok(turns) => turns,
            Err(e) => {
                self.bad(e);
                self.note("`:state clear` starts one from nothing.");
                return;
            }
        };
        if seeded {
            self.note("the state you had became the first turn.");
        }
        self.extend(turn_lines(turns.len() - 1, &turn));
        if turn.who.is_none() {
            self.note("nobody named — `:turn customer: …` attributes it.");
        }
        if turns.len() == 1 {
            self.note(
                "add the reply with another :turn; Enter re-asks every question over the thread.",
            );
        }
    }

    fn show_turns(&mut self) {
        let Some(turns) = self.session.turns() else {
            self.heading("state");
            if self.session.state_is_empty() {
                self.note("empty — `:turn customer: <text>` starts a conversation.");
            } else {
                let pretty = serde_json::to_string_pretty(&self.session.state)
                    .unwrap_or_else(|_| self.session.state_preview());
                self.extend(highlight::json(&pretty));
                self.note(
                    "not a conversation — `:turn <who>: <text>` makes this text the first turn.",
                );
            }
            return;
        };
        let plural = if turns.len() == 1 { "" } else { "s" };
        self.heading(format!("conversation ({} turn{plural})", turns.len()));
        for (i, turn) in turns.iter().enumerate() {
            self.extend(turn_lines(i, turn));
        }
        self.note("`:turn drop` takes the last one back; `:json` shows it as the state it is.");
    }

    fn set_state(&mut self, value: Value) {
        self.session.state = value;
        let preview = self.session.state_preview();
        let shown = if preview.chars().count() > 120 {
            format!("{}…", preview.chars().take(120).collect::<String>())
        } else {
            preview
        };
        self.note(format!("state ← {shown}"));
        if self.session.questions.is_empty() {
            self.note(
                "now add a question: :noul, :choice or :score (`:preset triage` loads a set).",
            );
        }
    }

    fn add(&mut self, parsed: Result<(String, Question), String>) {
        match parsed {
            Ok((name, question)) => {
                let replaced = self.session.insert(name.clone(), question.clone());
                let index = self
                    .session
                    .questions
                    .iter()
                    .position(|(n, _)| *n == name)
                    .unwrap_or(0);
                self.extend(question_lines(index, &name, &question));
                if replaced {
                    self.note(format!("replaced {name}"));
                }
                if self.session.questions.len() == 1 {
                    self.note("Enter on an empty line sends the session.");
                }
            }
            Err(e) => self.bad(e),
        }
    }

    fn list_questions(&mut self) {
        self.heading(format!("questions ({})", self.session.questions.len()));
        if self.session.questions.is_empty() {
            self.note("none yet — :noul, :choice, :score, or :preset triage");
            return;
        }
        let items: Vec<(usize, String, Question)> = self
            .session
            .questions
            .iter()
            .enumerate()
            .map(|(i, (n, q))| (i, n.clone(), q.clone()))
            .collect();
        for (i, name, q) in items {
            self.extend(question_lines(i, &name, &q));
        }
    }

    /// The names the session already holds, so the builder can say what it would overwrite.
    fn question_names(&self) -> Vec<String> {
        self.session
            .questions
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Builder mode: the same question, built in a form, with the JSON shown as it is typed.
    fn open_builder(&mut self, name: &str) {
        let state = match &self.session.state {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let existing = self.question_names();
        self.builder = Some(Builder::new(state, name.trim()).over(existing));
        self.note("builder mode — Tab moves, Ctrl-S adds the question, Esc closes.");
    }

    fn builder_key(&mut self, key: KeyEvent) {
        let Some(builder) = self.builder.as_mut() else {
            return;
        };
        match builder.key(key) {
            Outcome::Open => {}
            Outcome::Cancel => {
                self.builder = None;
                self.note("builder closed");
            }
            Outcome::Commit(name, question, state) => {
                let command = builder.as_command();
                let kind = builder.kind;
                if !state.trim().is_empty() && Value::String(state.clone()) != self.session.state {
                    self.session.state = Value::String(state.clone());
                }
                self.blank();
                self.push(Line::from(vec![
                    Span::styled("› ", Style::new().fg(ACCENT)),
                    Span::raw(command),
                ]));
                self.note("(what builder mode just built — the one-line form does the same thing)");
                self.add(Ok((name, *question)));
                // Stay in the form, on the same type: a rubric is usually several questions of one
                // shape, and re-picking `choice` for every one of them is what made the form
                // slower than typing the command.
                let existing = self.question_names();
                self.builder = Some(Builder::new(state, "").of_kind(kind).over(existing));
                self.note(format!(
                    "still in the builder, type still `{}` — name the next one, or Esc to close.",
                    kind.label()
                ));
            }
        }
    }

    /// Sketch mode: the whole session on one page, parsed as it is typed.
    fn open_sketch(&mut self) {
        let mut editor = Editor::new(&sketch::render(&self.session));
        if self.session.state_is_empty() && self.session.questions.is_empty() {
            editor.preview = editor::Preview::Answers;
        }
        self.sketch = Some(editor);
        self.note("sketch mode — write the state, a --- line, then questions. ^S applies, ^G applies and sends, Esc closes.");
    }

    fn sketch_key(&mut self, key: KeyEvent) {
        let Some(editor) = self.sketch.as_mut() else {
            return;
        };
        let outcome = editor.key(key);
        let send = match outcome {
            editor::Outcome::Open => return,
            editor::Outcome::Cancel => {
                self.sketch = None;
                self.note("sketch closed, session unchanged");
                return;
            }
            editor::Outcome::Apply => false,
            editor::Outcome::ApplyAndAsk => true,
        };
        let parsed = editor.parsed();
        if !parsed.ok() {
            let n = parsed.problems.len();
            let first = &parsed.problems[0];
            editor.row = first.line.min(editor.lines.len() - 1);
            editor.col = 0;
            editor.message = Some(format!(
                "{n} problem{} to fix first — line {}: {}",
                if n == 1 { "" } else { "s" },
                first.line + 1,
                first.message
            ));
            return;
        }
        let text = editor.text();
        self.sketch = None;
        self.apply_sketch(&parsed, &text);
        if send {
            self.ask();
        }
    }

    /// Replace the session with a parsed page and say what changed.
    fn apply_sketch(&mut self, parsed: &sketch::Parsed, text: &str) {
        // The bars are on the page but not on the wire, so they count as a change of their own.
        let snapshot = |app: &Self| {
            format!(
                "{}{:?}",
                app.session.request_json(&app.model_name()),
                app.session.bars
            )
        };
        let before = snapshot(self);
        // The page is the whole request: no `@model` line means the client default.
        self.session = parsed.to_session();
        let after = snapshot(self);
        self.blank();
        self.push(Line::from(vec![
            Span::styled("› ", Style::new().fg(ACCENT)),
            dim("sketch applied"),
        ]));
        if before == after {
            self.note("nothing changed.");
            return;
        }
        self.extend(sketch::highlight(text.trim_end()));
        let n = self.session.questions.len();
        self.note(format!(
            "session ← {n} question{} from the page. Enter sends it; :json shows the body.",
            if n == 1 { "" } else { "s" }
        ));
    }

    fn show_sketch(&mut self) {
        self.heading("this session, as a sketch");
        let text = sketch::render(&self.session);
        self.extend(sketch::highlight(text.trim_end()));
        self.note("`name?` asks yes/no · `label = why` lines make a choice · `low < high` makes a score · :sketch opens it for editing.");
    }

    fn show_request(&mut self) {
        let model = self.model_name();
        let body = self.session.request_json(&model);
        self.heading("POST /v1/systemone");
        self.extend(highlight::json(&body));
        self.note("Questions are a name → {type, instructions, criteria} map; answers come back under the same names.");
    }

    fn show_last(&mut self) {
        match self.last_raw.clone() {
            Some(raw) => {
                self.heading("last response body");
                self.extend(highlight::json(&raw));
            }
            None => self.note("nothing sent yet."),
        }
    }

    /// `:cost` estimates the next call; `:cost <in>/<out>` puts a price on it, `:cost off` drops it.
    fn cost_cmd(&mut self, args: &str) {
        match args {
            "off" | "clear" | "none" => {
                self.rates = None;
                self.note("rates cleared — :cost now counts tokens only.");
                return;
            }
            "" => {}
            _ => match cost::parse_rates(args) {
                Ok(rates) => {
                    self.rates = Some(rates);
                    self.note(format!("rates ← {}", cost::format_rates(rates)));
                }
                Err(message) => {
                    self.bad(message);
                    return;
                }
            },
        }
        if self.session.questions.is_empty() {
            self.warn(
                "nothing to price yet — :noul, :choice or :score first (`:preset triage` loads a set).",
            );
            return;
        }
        let model = self.model_name();
        let estimate = cost::estimate(&self.session, &model);
        let thread = cost::thread(&self.session, &model);
        let rates = self.rates;
        self.heading(format!("cost estimate  ·  {model}"));
        self.extend(cost_lines(
            &estimate,
            rates,
            ":cost 0.20/1.00 prices it: dollars per million tokens, input then output",
            thread.as_ref(),
        ));
        self.note(
            "Tokens are estimated from the body, not counted by the API's tokenizer; `usage` on a live answer is the real thing.",
        );
        self.note(
            "Answer sizes come from the shapes you asked for: a score echoes its legend, a choice one probability per label.",
        );
        if let Some(rates) = rates {
            self.note(format!(
                "{}={} sets the same rates at startup.",
                cost::PRICE_ENV,
                cost::rates_value(rates)
            ));
        }
    }

    fn show_rust(&mut self) {
        let model = self.model_name();
        let code = codegen::rust(&self.session, &model, self.threshold);
        self.heading("this session, as Rust");
        self.extend(highlight::rust(&code));
    }

    fn threshold_cmd(&mut self, args: &str) {
        if args.is_empty() {
            self.note(format!("threshold {:.2}", self.threshold));
            return;
        }
        match args.parse::<f64>() {
            Ok(t) if (0.0..=1.0).contains(&t) => {
                self.threshold = t;
                self.note(format!(
                    "threshold {t:.2} — a noul now reads as yes at {t:.2} or above (that is `answer.is_yes({t:.2})`)"
                ));
            }
            _ => self.bad("threshold takes a number from 0 to 1, e.g. :threshold 0.8"),
        }
    }

    fn model_cmd(&mut self, args: &str) {
        if args.is_empty() {
            let model = self.model_name();
            self.note(format!("model {model}"));
            self.note("`jev-latest` moves with releases; pin a version for reproducibility.");
            return;
        }
        self.session.model = Some(args.to_owned());
        self.note(format!("model ← {args}"));
    }

    fn models_cmd(&mut self) {
        if self.mock || self.client.is_none() {
            self.heading("models (mock)");
            for m in mock::models() {
                self.push(Line::from(vec![
                    Span::raw("  "),
                    bold(m.name.clone()),
                    dim(format!("  {}  {}", m.release_date, m.description)),
                ]));
            }
            self.note("simulated — :key <api-key> to list the real ones.");
            return;
        }
        let client = self.client.clone().expect("checked");
        let tx = self.tx.clone();
        self.pending = true;
        self.note("GET /v1/models …");
        tokio::spawn(async move {
            let res = client.models().list().await;
            let _ = tx.send(Msg::Models(Box::new(res)));
        });
    }

    fn timeout_cmd(&mut self, args: &str) {
        if args.is_empty() {
            match self.timeout {
                Some(t) => self.note(format!("timeout {:.1}s per attempt", t.as_secs_f64())),
                None => self.note("timeout 10s per attempt (the SDK default)"),
            }
            return;
        }
        match args.parse::<f64>() {
            Ok(s) if s > 0.0 => {
                self.timeout = Some(Duration::from_secs_f64(s));
                self.note(format!(
                    "timeout ← {s:.1}s per attempt; retries still get their own attempts within a 30s budget"
                ));
            }
            _ => self.bad("timeout takes seconds, e.g. :timeout 3"),
        }
    }

    fn mock_cmd(&mut self, args: &str) {
        match args {
            "on" => self.mock = true,
            "off" => {
                if self.client.is_none() {
                    self.warn("no API key, so mock mode stays on. :key <api-key> to go live.");
                    return;
                }
                self.mock = false;
            }
            "" => {}
            _ => {
                self.bad("`:mock on` or `:mock off`");
                return;
            }
        }
        if self.mock {
            self.note(
                "mock on — answers are simulated locally, deterministic per state and question.",
            );
        } else {
            self.note(format!("mock off — live against {}.", self.model_name()));
        }
    }

    fn key_cmd(&mut self, args: &str) {
        if args.is_empty() {
            match std::env::var("TYPESAFE_API_KEY") {
                Ok(k) if !k.trim().is_empty() => {
                    self.note(format!("TYPESAFE_API_KEY is set ({}).", masked(&k)))
                }
                _ => self.note(
                    "TYPESAFE_API_KEY is not set — :key <api-key> sets one for this session.",
                ),
            }
            if self.client.is_some() {
                let model = self.model_name();
                self.note(format!("client ready, default model {model}"));
            }
            return;
        }
        match Client::builder().api_key(args).build() {
            Ok(client) => {
                let masked = masked(args);
                self.client = Some(client);
                self.mock = false;
                self.note(format!(
                    "key accepted ({masked}); mock off, calls go to the API now."
                ));
            }
            Err(e) => self.extend(error_lines(&e)),
        }
    }

    fn save(&mut self, path: &str) {
        if path.is_empty() {
            self.bad(":save <path>");
            return;
        }
        let body = if path.ends_with(".jev") {
            sketch::render(&self.session)
        } else {
            self.session.request_json(&self.model_name())
        };
        match std::fs::write(path, &body) {
            Ok(()) => self.note(format!("wrote {path}")),
            Err(e) => self.bad(format!("could not write {path}: {e}")),
        }
    }

    fn open(&mut self, path: &str) {
        if path.is_empty() {
            self.bad(":open <path>");
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.bad(format!("could not read {path}: {e}"));
                return;
            }
        };
        if path.ends_with(".jev") {
            let parsed = sketch::parse(&text);
            if let Some(p) = parsed.problems.first() {
                self.bad(format!("{path}:{}: {}", p.line + 1, p.message));
                return;
            }
            self.session = parsed.to_session();
            self.note(format!("loaded {path}"));
            self.list_questions();
            return;
        }
        match session::from_body(&text) {
            Ok(session) => {
                self.session = session;
                self.note(format!("loaded {path}"));
                self.list_questions();
            }
            Err(e) => self.bad(e),
        }
    }

    // ---- asking -----------------------------------------------------------------------------

    fn ask(&mut self) {
        if self.pending {
            self.warn("a request is already in flight.");
            return;
        }
        if self.session.questions.is_empty() {
            self.warn(
                "no questions yet — :noul, :choice or :score first (`:preset triage` loads a set).",
            );
            return;
        }
        if self.session.state_is_empty() {
            self.warn("no state yet — type the text to judge, or `:state <text>`.");
            return;
        }
        if self.mock || self.client.is_none() {
            self.ask_mock();
            return;
        }
        let client = self.client.clone().expect("checked");
        let state = self.session.state.clone();
        let questions = self.session.to_questions();
        let model = self.model_name();
        let timeout = self.timeout;
        let tx = self.tx.clone();
        self.pending = true;
        self.blank();
        self.push(Line::from(vec![
            dim("  POST /v1/systemone  "),
            dim(model.clone()),
            dim(format!("  {} question(s)", self.session.questions.len())),
        ]));
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let mut req = client.system_one(state, questions).model(model);
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            let res = req.await;
            let _ = tx.send(Msg::Answered(Box::new(res), started.elapsed()));
        });
    }

    fn ask_mock(&mut self) {
        let state = self.session.state.clone();
        let answers: Vec<(String, Option<Answer>)> = self
            .session
            .questions
            .iter()
            .map(|(name, q)| {
                let json = serde_json::to_value(q).unwrap_or(Value::Null);
                (name.clone(), mock::answer(&state, name, &json))
            })
            .collect();

        self.blank();
        self.push(Line::from(vec![
            Span::styled(
                "  answers  ",
                Style::new().fg(WARN).add_modifier(Modifier::BOLD),
            ),
            dim(format!("simulated · {}", self.model_name())),
        ]));
        for (name, answer) in &answers {
            let threshold = self.session.threshold_of(name, self.threshold);
            match answer {
                Some(a) => self.extend(answer_lines(name, a, threshold)),
                None => self.warn(format!(
                    "{name}: mock mode cannot simulate this question shape."
                )),
            }
        }
        self.last_raw = Some(mock::body(&answers, &self.model_name()));
        let estimate = cost::estimate(&self.session, &self.model_name());
        let money = match self.rates {
            Some(rates) => format!(
                " · {}",
                cost::usd(cost::price_estimate(&estimate, rates).total)
            ),
            None => String::new(),
        };
        self.note(format!(
            "≈ {} in / {} out tokens{money} — estimated, since nothing was sent (:cost breaks it down).",
            estimate.input_tokens, estimate.output_tokens
        ));
        self.note(
            "Mock numbers are deterministic noise, not judgement. :key <api-key> for real answers.",
        );
    }

    fn show_response(&mut self, res: &SystemOneResponse, elapsed: Duration) {
        let money = match self
            .rates
            .and_then(|rates| cost::price_usage(&res.usage, rates))
        {
            Some(cost) => format!(" · {}", cost::usd(cost.total)),
            None => String::new(),
        };
        self.blank();
        self.push(Line::from(vec![
            Span::styled(
                "  answers  ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            dim(format!(
                "{} · {:.0} ms · {} attempt(s){}",
                res.model,
                elapsed.as_secs_f64() * 1000.0,
                res.meta.attempts,
                match (res.usage.input_tokens, res.usage.output_tokens) {
                    (Some(i), Some(o)) => format!(" · {i} in / {o} out tokens{money}"),
                    _ => String::new(),
                }
            )),
        ]));
        let answers: Vec<(String, Answer)> = res
            .answers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, answer) in &answers {
            let threshold = self.session.threshold_of(name, self.threshold);
            self.extend(answer_lines(name, answer, threshold));
        }
        if let Some(id) = res.request_id() {
            self.note(format!("request_id {id}"));
        }
        self.last_raw = serde_json::to_string_pretty(&res.raw).ok();
    }
}

fn masked(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}
