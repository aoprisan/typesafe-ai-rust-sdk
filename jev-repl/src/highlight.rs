//! Small hand-rolled highlighters for the three languages this REPL shows: the JSON going over
//! the wire, the Rust it generates, and the command line you are typing.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const KEY: Color = Color::Cyan;
const STRING: Color = Color::Green;
const NUMBER: Color = Color::Yellow;
const LITERAL: Color = Color::Magenta;
const PUNCT: Color = Color::DarkGray;
const KEYWORD: Color = Color::Magenta;
const TYPE: Color = Color::Cyan;
const MACRO: Color = Color::LightBlue;
const COMMENT: Color = Color::DarkGray;

fn span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color))
}

/// Highlight pretty-printed JSON, indented into the transcript.
pub fn json(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(json_spans(line));
            Line::from(spans)
        })
        .collect()
}

/// One line of JSON. Keys are told from strings by the colon that follows them.
pub fn json_spans(line: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let (text, next) = read_string(&chars, i);
                let is_key = chars[next..]
                    .iter()
                    .find(|c| !c.is_whitespace())
                    .is_some_and(|c| *c == ':');
                spans.push(span(text, if is_key { KEY } else { STRING }));
                i = next;
            }
            '-' | '0'..='9' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || "-+.eE".contains(chars[i])) {
                    i += 1;
                }
                spans.push(span(collect(&chars, start, i), NUMBER));
            }
            c if c.is_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_alphabetic() {
                    i += 1;
                }
                let word = collect(&chars, start, i);
                let color = match word.as_str() {
                    "true" | "false" | "null" => LITERAL,
                    _ => Color::Reset,
                };
                spans.push(span(word, color));
            }
            '{' | '}' | '[' | ']' | ':' | ',' => {
                spans.push(span(c.to_string(), PUNCT));
                i += 1;
            }
            _ => {
                let start = i;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                spans.push(Span::raw(collect(&chars, start, i)));
            }
        }
    }
    spans
}

/// Highlight generated Rust, indented into the transcript.
pub fn rust(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(rust_spans(line));
            Line::from(spans)
        })
        .collect()
}

fn rust_spans(line: &str) -> Vec<Span<'static>> {
    const KEYWORDS: &[&str] = &[
        "async", "await", "else", "fn", "for", "if", "impl", "in", "let", "match", "mut", "pub",
        "return", "struct", "use", "while", "true", "false",
    ];
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            spans.push(span(collect(&chars, i, chars.len()), COMMENT));
            break;
        }
        match c {
            '"' => {
                let (text, next) = read_string(&chars, i);
                spans.push(span(text, STRING));
                i = next;
            }
            '0'..='9' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                spans.push(span(collect(&chars, start, i), NUMBER));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word = collect(&chars, start, i);
                let macro_call = chars.get(i) == Some(&'!');
                let color = if macro_call {
                    i += 1;
                    MACRO
                } else if KEYWORDS.contains(&word.as_str()) {
                    KEYWORD
                } else if word.starts_with(char::is_uppercase) {
                    TYPE
                } else {
                    Color::Reset
                };
                spans.push(span(
                    if macro_call { format!("{word}!") } else { word },
                    color,
                ));
            }
            '(' | ')' | '{' | '}' | '[' | ']' | ';' | ',' | '.' | ':' | '?' | '&' | '<' | '>' => {
                spans.push(span(c.to_string(), PUNCT));
                i += 1;
            }
            _ => {
                spans.push(Span::raw(c.to_string()));
                i += 1;
            }
        }
    }
    spans
}

/// Highlight the input line: the command, the question name, the `|` separators, and the
/// `label=description` / `yes:` criteria inside them.
pub fn command(input: &str, known: impl Fn(&str) -> bool) -> Vec<Span<'static>> {
    if input.is_empty() {
        return Vec::new();
    }
    if !input.starts_with(':') {
        // Bare text becomes the state.
        return vec![Span::raw(input.to_owned())];
    }
    let (cmd, rest) = match input.split_once(char::is_whitespace) {
        Some((c, r)) => (c, Some(r)),
        None => (input, None),
    };
    let cmd_style = if known(cmd) {
        Style::new()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Red)
    };
    let mut spans = vec![Span::styled(cmd.to_owned(), cmd_style)];
    let Some(rest) = rest else { return spans };
    spans.push(Span::raw(" "));

    let takes_name = matches!(
        cmd,
        ":noul" | ":choice" | ":score" | ":raw" | ":rm" | ":drop"
    );
    let mut body = rest;
    if takes_name {
        if let Some((name, tail)) = rest.split_once(char::is_whitespace) {
            spans.push(Span::styled(
                name.to_owned(),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            body = tail;
        } else {
            spans.push(Span::styled(
                rest.to_owned(),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            return spans;
        }
    }

    for (i, part) in body.split('|').enumerate() {
        if i > 0 {
            spans.push(span("|", PUNCT));
        }
        if part.trim_start().starts_with('{') || part.trim_start().starts_with('[') {
            spans.extend(json_spans(part));
            continue;
        }
        match part.split_once('=') {
            Some((label, desc)) if i > 0 => {
                spans.push(span(label.to_owned(), LITERAL));
                spans.push(span("=", PUNCT));
                spans.push(Span::raw(desc.to_owned()));
            }
            _ => match part.split_once(':') {
                Some((tag, desc)) if matches!(tag.trim(), "yes" | "no" | "true" | "false") => {
                    spans.push(span(tag.to_owned(), KEY));
                    spans.push(span(":", PUNCT));
                    spans.push(Span::raw(desc.to_owned()));
                }
                _ => spans.push(Span::raw(part.to_owned())),
            },
        }
    }
    spans
}

fn read_string(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => {
                i += 1;
                break;
            }
            _ => i += 1,
        }
    }
    let end = i.min(chars.len());
    (collect(chars, start, end), end)
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end.min(chars.len())].iter().collect()
}
