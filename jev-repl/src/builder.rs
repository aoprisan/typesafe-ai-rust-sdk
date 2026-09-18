//! Builder mode: compose a question in a form instead of a one-line command, with the JSON it
//! will send rendered beside it as you type.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value};
use typesafe::{Choice, Noul, Question, Score};

use crate::session::value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Noul,
    Choice,
    Score,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Noul => "noul",
            Kind::Choice => "choice",
            Kind::Score => "score",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Kind::Noul => "probability that the statement is true (0–1)",
            Kind::Choice => "one label out of the set you define",
            Kind::Score => "a weighted position along ordered levels",
        }
    }

    fn next(self) -> Self {
        match self {
            Kind::Noul => Kind::Choice,
            Kind::Choice => Kind::Score,
            Kind::Score => Kind::Noul,
        }
    }

    fn prev(self) -> Self {
        self.next().next()
    }
}

/// Which widget the keyboard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    State,
    Name,
    Kind,
    Instructions,
    /// A noul's `yes:` / `no:` criteria.
    Yes,
    No,
    /// A choice option: row index, and whether the cursor is in the label or the description.
    OptionLabel(usize),
    OptionDesc(usize),
    /// A score level.
    Level(usize),
}

/// What the app should do after a key press.
pub enum Outcome {
    /// Stay open.
    Open,
    /// Close without adding anything.
    Cancel,
    /// Add this question (and any edited state) to the session.
    Commit(String, Box<Question>, String),
}

pub struct Builder {
    pub state: String,
    pub name: String,
    pub kind: Kind,
    pub instructions: String,
    pub yes: String,
    pub no: String,
    pub options: Vec<(String, String)>,
    pub levels: Vec<String>,
    pub focus: usize,
    pub cursor: usize,
    pub message: Option<String>,
}

impl Builder {
    /// Opens on the first field that still needs an answer: the state if there is none, the name
    /// if there is no name, otherwise the type.
    pub fn new(state: String, name: &str) -> Self {
        let focus = if !name.is_empty() {
            2
        } else if state.trim().is_empty() {
            0
        } else {
            1
        };
        Self {
            state: state.clone(),
            name: name.to_owned(),
            kind: Kind::Noul,
            instructions: String::new(),
            yes: String::new(),
            no: String::new(),
            options: vec![
                (String::new(), String::new()),
                (String::new(), String::new()),
            ],
            levels: vec![String::new(), String::new(), String::new()],
            focus,
            cursor: if focus == 0 {
                state.chars().count()
            } else {
                name.chars().count()
            },
            message: None,
        }
    }

    /// The focusable fields, in tab order, for the current question type.
    pub fn fields(&self) -> Vec<Field> {
        let mut f = vec![Field::State, Field::Name, Field::Kind, Field::Instructions];
        match self.kind {
            Kind::Noul => f.extend([Field::Yes, Field::No]),
            Kind::Choice => {
                for i in 0..self.options.len() {
                    f.push(Field::OptionLabel(i));
                    f.push(Field::OptionDesc(i));
                }
            }
            Kind::Score => f.extend((0..self.levels.len()).map(Field::Level)),
        }
        f
    }

    pub fn focused(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    pub fn text(&self, field: Field) -> &str {
        match field {
            Field::State => &self.state,
            Field::Name => &self.name,
            Field::Instructions => &self.instructions,
            Field::Yes => &self.yes,
            Field::No => &self.no,
            Field::OptionLabel(i) => self.options.get(i).map(|o| o.0.as_str()).unwrap_or(""),
            Field::OptionDesc(i) => self.options.get(i).map(|o| o.1.as_str()).unwrap_or(""),
            Field::Level(i) => self.levels.get(i).map(String::as_str).unwrap_or(""),
            Field::Kind => self.kind.label(),
        }
    }

    fn text_mut(&mut self, field: Field) -> Option<&mut String> {
        match field {
            Field::State => Some(&mut self.state),
            Field::Name => Some(&mut self.name),
            Field::Instructions => Some(&mut self.instructions),
            Field::Yes => Some(&mut self.yes),
            Field::No => Some(&mut self.no),
            Field::OptionLabel(i) => self.options.get_mut(i).map(|o| &mut o.0),
            Field::OptionDesc(i) => self.options.get_mut(i).map(|o| &mut o.1),
            Field::Level(i) => self.levels.get_mut(i),
            Field::Kind => None,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        self.message = None;
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Char('s') if ctrl => return self.commit(),
            KeyCode::Char('x') if ctrl => self.delete_row(),
            KeyCode::Char('o') if ctrl => self.add_row(),
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Down => self.move_focus(1),
            KeyCode::Up => self.move_focus(-1),
            KeyCode::Enter => {
                if self.on_last_row() {
                    self.add_row();
                }
                self.move_focus(1);
            }
            KeyCode::Left if self.focused() == Field::Kind => self.set_kind(self.kind.prev()),
            KeyCode::Right if self.focused() == Field::Kind => self.set_kind(self.kind.next()),
            KeyCode::Char(' ') if self.focused() == Field::Kind => self.set_kind(self.kind.next()),
            KeyCode::Char(c) if self.focused() == Field::Kind => match c {
                'n' | 'N' => self.set_kind(Kind::Noul),
                'c' | 'C' => self.set_kind(Kind::Choice),
                's' | 'S' => self.set_kind(Kind::Score),
                _ => {}
            },
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let at = self.cursor - 1;
                    self.cursor = at;
                    self.remove(at);
                }
            }
            KeyCode::Delete => {
                let at = self.cursor;
                self.remove(at);
            }
            KeyCode::Char(c) => {
                let _ = shift;
                self.insert(c);
            }
            _ => {}
        }
        Outcome::Open
    }

    fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.focus = self.focus.min(self.fields().len() - 1);
    }

    fn move_focus(&mut self, delta: isize) {
        let len = self.fields().len() as isize;
        let next = (self.focus as isize + delta).rem_euclid(len) as usize;
        self.focus = next;
        self.cursor = self.len();
    }

    fn len(&self) -> usize {
        self.text(self.focused()).chars().count()
    }

    fn insert(&mut self, c: char) {
        let cursor = self.cursor;
        let field = self.focused();
        if let Some(s) = self.text_mut(field) {
            let at = byte_at(s, cursor);
            s.insert(at, c);
            self.cursor = cursor + 1;
        }
    }

    fn remove(&mut self, index: usize) {
        let field = self.focused();
        if let Some(s) = self.text_mut(field)
            && index < s.chars().count()
        {
            let at = byte_at(s, index);
            s.remove(at);
        }
    }

    fn on_last_row(&self) -> bool {
        match self.focused() {
            Field::OptionDesc(i) => i + 1 == self.options.len(),
            Field::Level(i) => i + 1 == self.levels.len(),
            _ => false,
        }
    }

    /// Ctrl-O, or Enter on the last row: one more option or level.
    pub fn add_row(&mut self) {
        match self.kind {
            Kind::Choice => self.options.push((String::new(), String::new())),
            Kind::Score => self.levels.push(String::new()),
            Kind::Noul => self.message = Some("A noul has only `yes` and `no`.".into()),
        }
    }

    /// Ctrl-X: drop the row the cursor is on.
    pub fn delete_row(&mut self) {
        match self.focused() {
            Field::OptionLabel(i) | Field::OptionDesc(i) if self.options.len() > 1 => {
                self.options.remove(i);
            }
            Field::Level(i) if self.levels.len() > 1 => {
                self.levels.remove(i);
            }
            _ => self.message = Some("Nothing to remove here.".into()),
        }
        self.focus = self.focus.min(self.fields().len() - 1);
        self.cursor = self.len();
    }

    /// The question as it would go on the wire right now, incomplete parts included.
    pub fn preview(&self) -> Value {
        let mut q = Map::new();
        q.insert("type".into(), Value::String(self.kind.label().into()));
        if !self.instructions.trim().is_empty() {
            q.insert("instructions".into(), value(&self.instructions));
        }
        match self.kind {
            Kind::Noul => {
                let mut criteria = Map::new();
                if !self.yes.trim().is_empty() {
                    criteria.insert("true".into(), value(&self.yes));
                }
                if !self.no.trim().is_empty() {
                    criteria.insert("false".into(), value(&self.no));
                }
                if !criteria.is_empty() {
                    q.insert("criteria".into(), Value::Object(criteria));
                }
            }
            Kind::Choice => {
                let criteria: Map<String, Value> = self
                    .options
                    .iter()
                    .filter(|(label, _)| !label.trim().is_empty())
                    .map(|(label, desc)| {
                        let desc = if desc.trim().is_empty() {
                            Value::Null
                        } else {
                            value(desc)
                        };
                        (label.trim().to_owned(), desc)
                    })
                    .collect();
                q.insert("criteria".into(), Value::Object(criteria));
            }
            Kind::Score => {
                let criteria: Vec<Value> = self
                    .levels
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| value(l))
                    .collect();
                q.insert("criteria".into(), Value::Array(criteria));
            }
        }
        let name = if self.name.trim().is_empty() {
            "<name>"
        } else {
            self.name.trim()
        };
        serde_json::json!({ name: Value::Object(q) })
    }

    /// The equivalent one-line command, so builder mode teaches the fast path.
    pub fn as_command(&self) -> String {
        let name = if self.name.trim().is_empty() {
            "<name>"
        } else {
            self.name.trim()
        };
        let instructions = self.instructions.trim();
        match self.kind {
            Kind::Noul => {
                let mut s = format!(":noul {name} {instructions}");
                if !self.yes.trim().is_empty() {
                    s.push_str(&format!(" | yes: {}", self.yes.trim()));
                }
                if !self.no.trim().is_empty() {
                    s.push_str(&format!(" | no: {}", self.no.trim()));
                }
                s
            }
            Kind::Choice => {
                let mut s = format!(":choice {name} {instructions}");
                for (label, desc) in self.options.iter().filter(|(l, _)| !l.trim().is_empty()) {
                    s.push_str(&format!(" | {}", label.trim()));
                    if !desc.trim().is_empty() {
                        s.push_str(&format!("={}", desc.trim()));
                    }
                }
                s
            }
            Kind::Score => {
                let mut s = format!(":score {name} {instructions}");
                for level in self.levels.iter().filter(|l| !l.trim().is_empty()) {
                    s.push_str(&format!(" | {}", level.trim()));
                }
                s
            }
        }
    }

    fn commit(&mut self) -> Outcome {
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            self.message = Some("Every question needs a name — answers come back under it.".into());
            return Outcome::Open;
        }
        if name.split_whitespace().count() > 1 {
            self.message = Some("Names cannot contain spaces.".into());
            return Outcome::Open;
        }
        let instructions = self.instructions.trim();
        if instructions.is_empty() {
            self.message = Some("Instructions are what the model actually reads.".into());
            return Outcome::Open;
        }
        let question: Question = match self.kind {
            Kind::Noul => {
                let mut q = Noul::new(value(instructions));
                if !self.yes.trim().is_empty() {
                    q = q.when_true(value(&self.yes));
                }
                if !self.no.trim().is_empty() {
                    q = q.when_false(value(&self.no));
                }
                q.into()
            }
            Kind::Choice => {
                let options: Vec<(String, String)> = self
                    .options
                    .iter()
                    .filter(|(l, _)| !l.trim().is_empty())
                    .cloned()
                    .collect();
                if options.len() < 2 {
                    self.message = Some("A choice needs at least two options.".into());
                    return Outcome::Open;
                }
                let mut q = Choice::new(value(instructions));
                for (label, desc) in options {
                    q = if desc.trim().is_empty() {
                        q.label(label.trim())
                    } else {
                        q.option(label.trim(), value(&desc))
                    };
                }
                q.into()
            }
            Kind::Score => {
                let levels: Vec<Value> = self
                    .levels
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| value(l))
                    .collect();
                if levels.len() < 2 {
                    self.message = Some("A score needs at least two ordered levels.".into());
                    return Outcome::Open;
                }
                Score::new(value(instructions), levels).into()
            }
        };
        Outcome::Commit(name, Box::new(question), self.state.clone())
    }
}

fn byte_at(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}
