//! Sketch mode: a small text editor over one page of [`sketch`](crate::sketch) notation. The
//! page is parsed on every keystroke, so the gutter and the preview pane always show what the
//! text currently means.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::sketch::{self, Parsed};
use crate::words;

/// What the right-hand pane shows next to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// The request body, exactly as it would be POSTed.
    Json,
    /// Simulated answers, so the shape of what comes back is visible while writing.
    Answers,
    /// The same request as a program against the SDK.
    Rust,
    /// What a call would cost: tokens per question, priced when rates are set.
    Cost,
}

impl Preview {
    pub const ALL: [Preview; 4] = [
        Preview::Json,
        Preview::Answers,
        Preview::Rust,
        Preview::Cost,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preview::Json => "json",
            Preview::Answers => "answers",
            Preview::Rust => "rust",
            Preview::Cost => "cost",
        }
    }

    fn next(self) -> Self {
        match self {
            Preview::Json => Preview::Answers,
            Preview::Answers => Preview::Rust,
            Preview::Rust => Preview::Cost,
            Preview::Cost => Preview::Json,
        }
    }
}

/// What the app should do after a key press.
pub enum Outcome {
    /// Stay open.
    Open,
    /// Close without touching the session.
    Cancel,
    /// Replace the session with the page.
    Apply,
    /// Replace the session with the page, then send it.
    ApplyAndAsk,
}

pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    /// Column as a character index into the current line.
    pub col: usize,
    /// First visible row; the UI adjusts it to keep the cursor in view.
    pub top: usize,
    pub preview: Preview,
    pub dirty: bool,
    /// Esc on a dirty page asks once before discarding it.
    esc_armed: bool,
    /// The last line cut with Ctrl-X, ready for Ctrl-U.
    cut: Option<String>,
    pub message: Option<String>,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        Self {
            lines,
            row: 0,
            col: 0,
            top: 0,
            preview: Preview::Json,
            dirty: false,
            esc_armed: false,
            cut: None,
            message: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn parsed(&self) -> Parsed {
        sketch::parse(&self.text())
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = words::is_alt(&key);
        self.message = None;
        let armed = std::mem::take(&mut self.esc_armed);
        // Alt-←/→ cross a word, Alt-Backspace takes one out; Alt-↑/↓ below move the whole line.
        if words::is_word_left(&key) {
            self.word_left();
            return Outcome::Open;
        }
        if words::is_word_right(&key) {
            self.word_right();
            return Outcome::Open;
        }
        if words::is_delete_word_left(&key) {
            self.delete_word_left();
            return Outcome::Open;
        }
        match key.code {
            KeyCode::Esc => {
                if self.dirty && !armed {
                    self.esc_armed = true;
                    self.message =
                        Some("unapplied edits — Esc again discards them, ^S applies".into());
                } else {
                    return Outcome::Cancel;
                }
            }
            KeyCode::Char('s') if ctrl => return Outcome::Apply,
            KeyCode::Char('g') if ctrl => return Outcome::ApplyAndAsk,
            KeyCode::Char('p') if ctrl => self.preview = self.preview.next(),
            KeyCode::Char('x') if ctrl => self.cut_line(),
            KeyCode::Char('u') if ctrl => self.paste_line(),
            KeyCode::Char('a') if ctrl => self.col = 0,
            KeyCode::Char('e') if ctrl => self.col = self.len(),
            KeyCode::Up if alt => self.swap(-1),
            KeyCode::Down if alt => self.swap(1),
            KeyCode::Up => self.vertical(-1),
            KeyCode::Down => self.vertical(1),
            KeyCode::PageUp => self.vertical(-10),
            KeyCode::PageDown => self.vertical(10),
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.len();
                }
            }
            KeyCode::Right => {
                if self.col < self.len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.len(),
            KeyCode::Enter => self.newline(),
            KeyCode::Tab => {
                self.insert_str("  ");
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c),
            _ => {}
        }
        Outcome::Open
    }

    fn len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn vertical(&mut self, delta: isize) {
        let last = self.lines.len() as isize - 1;
        self.row = (self.row as isize + delta).clamp(0, last) as usize;
        self.col = self.col.min(self.len());
    }

    fn insert(&mut self, c: char) {
        let at = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
        self.dirty = true;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(c);
        }
    }

    /// Split the line at the cursor; the new line keeps the indentation of the one above.
    fn newline(&mut self) {
        let at = byte_at(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(at);
        let indent: String = self.lines[self.row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let indent = if tail.trim().is_empty() && self.lines[self.row].trim().is_empty() {
            String::new()
        } else {
            indent
        };
        self.row += 1;
        self.col = indent.chars().count();
        self.lines.insert(self.row, format!("{indent}{tail}"));
        self.dirty = true;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let at = byte_at(&self.lines[self.row], self.col);
            self.lines[self.row].remove(at);
            self.dirty = true;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.len();
            self.lines[self.row].push_str(&line);
            self.dirty = true;
        }
    }

    fn delete(&mut self) {
        if self.col < self.len() {
            let at = byte_at(&self.lines[self.row], self.col);
            self.lines[self.row].remove(at);
            self.dirty = true;
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
            self.dirty = true;
        }
    }

    /// Ctrl-X: take the current line out; Ctrl-U puts it back wherever the cursor is.
    fn cut_line(&mut self) {
        let line = if self.lines.len() == 1 {
            std::mem::take(&mut self.lines[0])
        } else {
            self.lines.remove(self.row)
        };
        self.cut = Some(line);
        self.row = self.row.min(self.lines.len() - 1);
        self.col = self.col.min(self.len());
        self.dirty = true;
        self.message = Some("line cut — ^U pastes it above the cursor".into());
    }

    fn paste_line(&mut self) {
        match self.cut.clone() {
            Some(line) => {
                self.lines.insert(self.row, line);
                self.row += 1;
                self.dirty = true;
            }
            None => self.message = Some("nothing cut yet — ^X cuts the current line".into()),
        }
    }

    fn chars(&self) -> Vec<char> {
        self.lines[self.row].chars().collect()
    }

    /// Alt-←: to the start of the word before the cursor, or onto the end of the line above.
    fn word_left(&mut self) {
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = self.len();
            }
            return;
        }
        self.col = words::word_left(&self.chars(), self.col);
    }

    /// Alt-→: past the end of the word after the cursor, or onto the start of the line below.
    fn word_right(&mut self) {
        if self.col >= self.len() {
            if self.row + 1 < self.lines.len() {
                self.row += 1;
                self.col = 0;
            }
            return;
        }
        self.col = words::word_right(&self.chars(), self.col);
    }

    /// Alt-Backspace: take the word before the cursor out.
    fn delete_word_left(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        let chars = self.chars();
        let at = words::word_left(&chars, self.col);
        let kept: String = chars[..at].iter().chain(chars[self.col..].iter()).collect();
        self.lines[self.row] = kept;
        self.col = at;
        self.dirty = true;
    }

    /// Alt-Up / Alt-Down: move the current line, which is how questions and levels get reordered.
    fn swap(&mut self, delta: isize) {
        let target = self.row as isize + delta;
        if target < 0 || target >= self.lines.len() as isize {
            return;
        }
        self.lines.swap(self.row, target as usize);
        self.row = target as usize;
        self.dirty = true;
    }
}

fn byte_at(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}
