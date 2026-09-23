//! Sketch notation: the whole request written as one page of plain text, the way you would
//! scribble a rubric on paper. The shape of each question is read off its punctuation, so there
//! is nothing to select — you write what you mean and the gutter tells you what it became.
//!
//! ```text
//! The payout failed again, third time this month. I'm done waiting.
//! ---
//! is_urgent? The message conveys urgency or time-sensitivity
//!   yes: A deadline, a threat to leave, or "ASAP"
//!   no: Routine, no time pressure
//!
//! department: Which team should handle this
//!   billing = Payment or subscription issues
//!   technical = Bugs or integration problems
//!   sales
//!
//! frustration: How frustrated the customer appears
//!   Calm < Frustrated but civil < Very angry
//! ```
//!
//! - Everything above the first `---` line is the state (JSON if it parses as JSON).
//! - `name? instructions` is a yes/no question (noul); `yes:` / `no:` lines describe the outcomes.
//! - `name: instructions` followed by `label = description` lines (or bare labels) is a choice.
//! - `name: instructions` followed by levels joined with `<` is a score, lowest first.
//! - `name! {json}` sends a hand-built question object, like [`Question::Raw`].
//! - The parts of a question can also go on its first line, separated by `|`.
//! - `@model jev-2` pins the model; `#` starts a comment. Indentation is only for reading.
//! - `@threshold 0.6` under a noul, or `@confidence 0.7` under a choice or a score, writes down the
//!   bar its answer is acted on at. It stays on the page and is never sent.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use typesafe::{Choice, Noul, Question, Score};

use crate::format::{ACCENT, CHOICE, DIM, NOUL, SCORE, WARN};
use crate::session::Session;

/// What one line of a sketch turned out to be; shown in the editor gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Blank,
    State,
    Rule,
    Comment,
    Model,
    Noul,
    Choice,
    Score,
    Raw,
    Yes,
    No,
    Option,
    Level,
    Json,
    /// `@threshold` or `@confidence`: the bar the question above is acted on at.
    Bar,
    /// A line that could not be placed; it always carries a problem.
    Stray,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Blank | Tag::Rule => "",
            Tag::State => "state",
            Tag::Comment => "#",
            Tag::Model => "model",
            Tag::Noul => "noul",
            Tag::Choice => "choice",
            Tag::Score => "score",
            Tag::Raw => "raw",
            Tag::Yes => "yes",
            Tag::No => "no",
            Tag::Option => "option",
            Tag::Level => "level",
            Tag::Json => "json",
            Tag::Bar => "bar",
            Tag::Stray => "?",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Tag::Noul | Tag::Yes | Tag::No => NOUL,
            Tag::Choice | Tag::Option => CHOICE,
            Tag::Score | Tag::Level => SCORE,
            Tag::Raw | Tag::Json => WARN,
            Tag::Bar => ACCENT,
            Tag::Stray => Color::Red,
            _ => DIM,
        }
    }

    /// Body lines carry their question's colour; heads are bold.
    pub fn is_head(self) -> bool {
        matches!(self, Tag::Noul | Tag::Choice | Tag::Score | Tag::Raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Zero-based line.
    pub line: usize,
    pub message: String,
}

/// Where a question sits on the page, so a bar can be written back without redrawing the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    /// Zero-based line of the question's head.
    pub head: usize,
    /// Zero-based line of the last line that belongs to it: a part, a criterion, its bar.
    pub last: usize,
    /// Zero-based line of its `@threshold` or `@confidence`, when it has one.
    pub bar: Option<usize>,
}

/// A parsed sketch. Questions with problems are left out of `questions` but keep their tags, so
/// the page still reads sensibly while it is being fixed.
#[derive(Debug, Default)]
pub struct Parsed {
    pub state: Value,
    pub model: Option<String>,
    pub questions: Vec<(String, Question)>,
    /// Each question's bar, from its `@threshold` or `@confidence` line.
    pub bars: Vec<(String, f64)>,
    /// Every question that parsed, by name: where it is on the page.
    pub blocks: Vec<(String, BlockSpan)>,
    /// One per line of the input.
    pub tags: Vec<Tag>,
    pub problems: Vec<Problem>,
}

impl Parsed {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn to_session(&self) -> Session {
        Session {
            state: self.state.clone(),
            questions: self.questions.clone(),
            model: self.model.clone(),
            bars: self.bars.clone(),
        }
    }

    /// Where a question that parsed sits on the page.
    pub fn block(&self, name: &str) -> Option<BlockSpan> {
        self.blocks
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, block)| *block)
    }

    /// The bar a question's line gave it.
    pub fn bar(&self, name: &str) -> Option<f64> {
        self.bars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, bar)| *bar)
    }

    /// The first problem on or after a line — what the status line shows for the cursor.
    pub fn problem_at(&self, line: usize) -> Option<&Problem> {
        self.problems.iter().find(|p| p.line == line)
    }
}

/// A question block under construction: its head, and the parts collected from the head line
/// (after `|`) and the lines below it.
struct Block {
    line: usize,
    name: String,
    marker: char,
    /// The head line after the marker, unsplit — raw questions need it whole.
    rest: String,
    parts: Vec<(usize, String)>,
    /// `@threshold` / `@confidence` lines: the line, which of the two, and the number.
    bars: Vec<(usize, Directive, f64)>,
}

/// The two directives that set a question's bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    Threshold,
    Confidence,
}

pub fn parse(text: &str) -> Parsed {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Parsed {
        tags: vec![Tag::Blank; lines.len()],
        ..Default::default()
    };

    let rule = lines.iter().position(|l| l.trim() == "---");
    let state_end = rule.unwrap_or(lines.len());
    for (i, line) in lines[..state_end].iter().enumerate() {
        out.tags[i] = if line.trim().is_empty() {
            Tag::Blank
        } else {
            Tag::State
        };
    }
    out.state = state_value(&lines[..state_end].join("\n"));

    let Some(rule) = rule else {
        if lines.iter().any(|l| !l.trim().is_empty()) {
            out.problems.push(Problem {
                line: lines.len() - 1,
                message: "no `---` yet — the questions go below one".into(),
            });
        }
        return out;
    };
    out.tags[rule] = Tag::Rule;

    let mut block: Option<Block> = None;
    for (i, raw) in lines.iter().enumerate().skip(rule + 1) {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            out.tags[i] = Tag::Comment;
            continue;
        }
        if let Some(directive) = line.strip_prefix('@') {
            let (key, arg) = directive
                .split_once(char::is_whitespace)
                .map(|(k, a)| (k, a.trim()))
                .unwrap_or((directive, ""));
            match key {
                "threshold" | "confidence" => {
                    let directive = if key == "threshold" {
                        Directive::Threshold
                    } else {
                        Directive::Confidence
                    };
                    match (parse_bar(arg), block.as_mut()) {
                        (None, _) => {
                            out.tags[i] = Tag::Stray;
                            out.problems.push(Problem {
                                line: i,
                                message: format!(
                                    "`@{key}` takes a number from 0 to 1, e.g. `@{key} 0.6`"
                                ),
                            });
                        }
                        (Some(_), None) => {
                            out.tags[i] = Tag::Stray;
                            out.problems.push(Problem {
                                line: i,
                                message: match directive {
                                    Directive::Threshold => "`@threshold` belongs under a question — put it below the `name?` line it sets",
                                    Directive::Confidence => "`@confidence` belongs under a question — put it below the choice or score it gates",
                                }
                                .into(),
                            });
                        }
                        (Some(bar), Some(b)) => b.bars.push((i, directive, bar)),
                    }
                }
                "model" if !arg.is_empty() => {
                    out.tags[i] = Tag::Model;
                    out.model = Some(arg.to_owned());
                }
                "model" => {
                    out.tags[i] = Tag::Stray;
                    out.problems.push(Problem {
                        line: i,
                        message: "`@model` needs a name, e.g. `@model jev-latest`".into(),
                    });
                }
                other => {
                    out.tags[i] = Tag::Stray;
                    out.problems.push(Problem {
                        line: i,
                        message: format!(
                            "unknown directive `@{other}`; there is `@model`, and `@threshold` or `@confidence` under a question"
                        ),
                    });
                }
            }
            continue;
        }
        if let Some((name, marker, rest)) = head(line) {
            if let Some(done) = block.take() {
                finish(done, &mut out);
            }
            block = Some(Block {
                line: i,
                name,
                marker,
                rest: rest.to_owned(),
                parts: Vec::new(),
                bars: Vec::new(),
            });
            continue;
        }
        match block.as_mut() {
            Some(b) => b.parts.push((i, line.to_owned())),
            None => {
                out.tags[i] = Tag::Stray;
                out.problems.push(Problem {
                    line: i,
                    message: "not a question — start one with `name?` (yes/no) or `name:` (options or levels)".into(),
                });
            }
        }
    }
    if let Some(done) = block.take() {
        finish(done, &mut out);
    }
    out
}

/// `name?` / `name:` / `name!` at the start of a line, with the name a plain identifier.
/// `yes:` and `no:` are a noul's body, never a head.
fn head(line: &str) -> Option<(String, char, &str)> {
    let end = line.find(['?', ':', '!'])?;
    let (name, rest) = line.split_at(end);
    if !is_name(name) {
        return None;
    }
    let marker = rest.chars().next()?;
    let after = &rest[1..];
    if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
        return None;
    }
    if marker == ':' && is_criterion(name) {
        return None;
    }
    Some((name.to_owned(), marker, after.trim()))
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn is_criterion(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "yes" | "no" | "true" | "false"
    )
}

/// A bar as the page writes it: a plain decimal from 0 to 1. Plain, so `1e-1` and `0x1` are
/// refused the same way in every port rather than read however a runtime happens to read them.
pub fn parse_bar(text: &str) -> Option<f64> {
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let plain = match text.split_once('.') {
        Some(("", fraction)) => !fraction.is_empty() && digits(fraction),
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => !text.is_empty() && digits(text),
    };
    if !plain {
        return None;
    }
    let n: f64 = text.parse().ok()?;
    (0.0..=1.0).contains(&n).then_some(n)
}

/// The wire `type` of a question that parsed, with a hand-built object as `raw`.
fn kind_of(question: &Question) -> &'static str {
    match question {
        Question::Noul(_) => "noul",
        Question::Choice(_) => "choice",
        Question::Score(_) => "score",
        _ => "raw",
    }
}

/// Turn a finished block into a question and its bar, or into problems.
fn finish(b: Block, out: &mut Parsed) {
    let before = out.questions.len();
    let (line, name, parts, bars) = (b.line, b.name.clone(), b.parts.clone(), b.bars.clone());
    finish_question(b, out);
    let kind = if out.questions.len() > before {
        out.questions.last().map(|(_, q)| kind_of(q))
    } else {
        None
    };
    let mut last = parts.iter().map(|(i, _)| *i).fold(line, usize::max);
    let mut bar_at: Option<usize> = None;
    for (i, directive, value) in bars {
        last = last.max(i);
        out.tags[i] = Tag::Bar;
        // A question that did not parse has problems enough; its bar waits until it does.
        let Some(kind) = kind else { continue };
        let refused = if let Some(at) = bar_at {
            Some(format!("`{name}` already has a bar on line {}", at + 1))
        } else if kind == "raw" {
            Some("a raw question takes no bar — jev cannot read its answer".to_owned())
        } else if kind == "noul" && directive == Directive::Confidence {
            Some("a yes/no question takes `@threshold`, not `@confidence`".to_owned())
        } else if kind != "noul" && directive == Directive::Threshold {
            Some("a choice or a score takes `@confidence`, not `@threshold`".to_owned())
        } else {
            None
        };
        match refused {
            Some(message) => {
                out.tags[i] = Tag::Stray;
                out.problems.push(Problem { line: i, message });
            }
            None => {
                bar_at = Some(i);
                out.bars.push((name.clone(), value));
            }
        }
    }
    if kind.is_some() {
        out.blocks.push((
            name,
            BlockSpan {
                head: line,
                last,
                bar: bar_at,
            },
        ));
    }
}

fn finish_question(b: Block, out: &mut Parsed) {
    let mut problem = |line: usize, message: String| {
        out.problems.push(Problem { line, message });
    };
    if out.questions.iter().any(|(n, _)| *n == b.name) {
        out.tags[b.line] = Tag::Stray;
        problem(
            b.line,
            format!("another question is already named `{}`", b.name),
        );
        return;
    }

    if b.marker == '!' {
        out.tags[b.line] = Tag::Raw;
        let mut text = b.rest.clone();
        for (i, part) in &b.parts {
            out.tags[*i] = Tag::Json;
            text.push('\n');
            text.push_str(part);
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.get("type").and_then(Value::as_str).is_some_and(|t| !t.is_empty()) => {
                out.questions.push((b.name, Question::Raw(v)));
            }
            Ok(_) => problem(
                b.line,
                "a raw question is a JSON object with a `type`, e.g. {\"type\": \"noul\", \"instructions\": \"…\"}"
                    .into(),
            ),
            Err(e) => problem(b.line, format!("raw question is not valid JSON: {e}")),
        }
        return;
    }

    // `name: instructions | part | part` — inline parts join the ones below.
    let mut parts: Vec<(usize, String)> = Vec::new();
    let mut inline = split_top(&b.rest, '|').into_iter().map(str::trim);
    let instructions = inline.next().unwrap_or("").to_owned();
    parts.extend(
        inline
            .filter(|p| !p.is_empty())
            .map(|p| (b.line, p.to_owned())),
    );
    parts.extend(b.parts.iter().cloned());
    for (_, p) in &mut parts {
        if let Some(rest) = p
            .strip_prefix("- ")
            .or_else(|| p.strip_prefix("* "))
            .or_else(|| p.strip_prefix("• "))
        {
            *p = rest.trim().to_owned();
        }
    }

    if instructions.is_empty() {
        out.tags[b.line] = Tag::Stray;
        problem(
            b.line,
            format!(
                "`{}` needs instructions after the `{}` — what should the model decide?",
                b.name, b.marker
            ),
        );
        return;
    }
    let instructions = value(&instructions);

    let all_criteria = !parts.is_empty()
        && parts.iter().all(|(_, p)| {
            p.split_once(':')
                .is_some_and(|(k, _)| is_criterion(k.trim()))
        });

    if b.marker == '?' || all_criteria {
        out.tags[b.line] = Tag::Noul;
        let mut q = Noul::new(instructions);
        let mut bad = false;
        for (i, part) in &parts {
            match part.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
                Some((k, v)) if is_criterion(k) => {
                    let yes = matches!(k.to_ascii_lowercase().as_str(), "yes" | "true");
                    out.tags[*i] = if yes { Tag::Yes } else { Tag::No };
                    q = if yes {
                        q.when_true(value(v))
                    } else {
                        q.when_false(value(v))
                    };
                }
                _ => {
                    bad = true;
                    out.tags[*i] = Tag::Stray;
                    problem(
                        *i,
                        "a yes/no question only takes `yes: …` and `no: …` lines".into(),
                    );
                }
            }
        }
        if !bad {
            out.questions.push((b.name, q.into()));
        }
        return;
    }

    if parts.is_empty() {
        out.tags[b.line] = Tag::Stray;
        problem(
            b.line,
            "add options (`billing = Payments`) or ordered levels (`Calm < Annoyed < Furious`), or end the name with `?` for yes/no"
                .into(),
        );
        return;
    }

    let is_choice = parts.iter().any(|(_, p)| split_top(p, '=').len() > 1);
    // A `<` only means "level" in a part that is not an option; descriptions may contain one.
    let has_levels = parts
        .iter()
        .any(|(_, p)| split_top(p, '=').len() == 1 && split_top(p, '<').len() > 1);
    if is_choice && has_levels {
        out.tags[b.line] = Tag::Stray;
        for (i, part) in &parts {
            out.tags[*i] = if split_top(part, '=').len() > 1 {
                Tag::Option
            } else {
                Tag::Level
            };
        }
        problem(
            b.line,
            "options (`label = why`) and levels (`low < high`) are mixed — a question is a choice or a score, not both"
                .into(),
        );
        return;
    }
    let is_score = !is_choice && has_levels;

    if is_score {
        out.tags[b.line] = Tag::Score;
        let mut levels = Vec::new();
        for (i, part) in &parts {
            out.tags[*i] = Tag::Level;
            levels.extend(
                split_top(part, '<')
                    .into_iter()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(value),
            );
        }
        if levels.len() < 2 {
            problem(
                b.line,
                "a score needs at least two levels, lowest first".into(),
            );
            return;
        }
        out.questions
            .push((b.name, Score::new(instructions, levels).into()));
        return;
    }

    out.tags[b.line] = Tag::Choice;
    let mut q = Choice::new(instructions);
    let mut count = 0;
    let mut bad = false;
    for (i, part) in &parts {
        out.tags[*i] = Tag::Option;
        let (label, desc) = match split_top(part, '=').as_slice() {
            [l, d, ..] => (l.trim(), d.trim()),
            _ => (part.as_str(), ""),
        };
        if label.is_empty() {
            bad = true;
            out.tags[*i] = Tag::Stray;
            problem(*i, "an option needs a label before the `=`".into());
            continue;
        }
        q = if desc.is_empty() {
            q.label(label)
        } else {
            q.option(label, value(desc))
        };
        count += 1;
    }
    if bad {
        return;
    }
    if count < 2 {
        problem(
            b.line,
            "one option is not a choice — add another, or write ordered levels as `a < b < c`"
                .into(),
        );
        return;
    }
    out.questions.push((b.name, q.into()));
}

/// Split on `sep`, except inside a double-quoted JSON string — so a quoted value can carry the
/// notation's own punctuation. Splits at most once for `=`, since a description may contain it.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            c if c == sep && !quoted && (sep != '=' || parts.is_empty()) => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Text becomes a JSON value when it is written as one (an object, an array or a quoted
/// string); anything else is sent as the plain string it is.
pub fn value(text: &str) -> Value {
    let t = text.trim();
    if (t.starts_with('{') || t.starts_with('[') || t.starts_with('"'))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return v;
    }
    Value::String(t.to_owned())
}

fn state_value(text: &str) -> Value {
    let t = text.trim();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return v;
    }
    Value::String(t.to_owned())
}

// ---- rendering ----------------------------------------------------------------------------

/// The session as a sketch — the inverse of [`parse`], so a page can be opened, edited and
/// applied without losing anything.
pub fn render(session: &Session) -> String {
    let mut out = String::new();
    match &session.state {
        Value::String(s) => out.push_str(s.trim_end()),
        Value::Null => {}
        other => out.push_str(&serde_json::to_string_pretty(other).unwrap_or_default()),
    }
    out.push_str("\n---\n");
    if let Some(model) = &session.model {
        out.push_str(&format!("@model {model}\n"));
    }
    for (i, (name, question)) in session.questions.iter().enumerate() {
        if i > 0 || session.model.is_some() {
            out.push('\n');
        }
        out.push_str(&render_question(name, question));
        if let (Some(bar), Some(directive)) = (session.bar(name), bar_directive(question)) {
            out.push_str(&format!("  {directive} {bar}\n"));
        }
    }
    out
}

/// Which directive holds a question's bar, by its kind; a raw question has none.
fn bar_directive(question: &Question) -> Option<&'static str> {
    match kind_of(question) {
        "noul" => Some("@threshold"),
        "choice" | "score" => Some("@confidence"),
        _ => None,
    }
}

/// Write bars into a page without redrawing it: each named question's bar line is replaced, or a
/// new one is added at the end of its block, and every other line — comments, blank lines, the way
/// someone chose to lay out their options — stays exactly as it was.
///
/// Questions the page does not have, raw ones and pages that do not parse are left alone; the
/// caller has already parsed the page and knows which it is.
pub fn set_bars(text: &str, bars: &[(String, f64)]) -> String {
    let parsed = parse(text);
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let cr = if text.contains("\r\n") { "\r" } else { "" };
    let leading = |line: &str| -> String {
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    };
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for (name, value) in bars {
        let directive = parsed
            .questions
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, q)| bar_directive(q));
        let (Some(block), Some(directive)) = (parsed.block(name), directive) else {
            continue;
        };
        if let Some(at) = block.bar {
            let old = &lines[at];
            let end = if old.ends_with('\r') { "\r" } else { "" };
            lines[at] = format!("{}{directive} {value}{end}", leading(old));
            continue;
        }
        let indent = (block.head + 1..=block.last)
            .find(|i| !matches!(parsed.tags.get(*i), None | Some(Tag::Blank | Tag::Comment)))
            .map_or_else(|| "  ".to_owned(), |i| leading(&lines[i]));
        inserts.push((block.last, format!("{indent}{directive} {value}{cr}")));
    }
    inserts.sort_by(|x, y| y.0.cmp(&x.0));
    for (after, line) in inserts {
        lines.insert(after + 1, line);
    }
    lines.join("\n")
}

fn render_question(name: &str, question: &Question) -> String {
    let v = serde_json::to_value(question).unwrap_or(Value::Null);
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let instructions = v.get("instructions").map(part).unwrap_or_default();
    let criteria = v.get("criteria");
    let mut s = String::new();
    match (question, kind) {
        (Question::Raw(raw), _) => {
            s.push_str(&format!("{name}! {raw}\n"));
        }
        (_, "noul") => {
            s.push_str(&format!("{name}? {instructions}\n"));
            if let Some(yes) = criteria.and_then(|c| c.get("true")) {
                s.push_str(&format!("  yes: {}\n", part(yes)));
            }
            if let Some(no) = criteria.and_then(|c| c.get("false")) {
                s.push_str(&format!("  no: {}\n", part(no)));
            }
        }
        (_, "choice") => {
            s.push_str(&format!("{name}: {instructions}\n"));
            if let Some(map) = criteria.and_then(Value::as_object) {
                for (label, desc) in map {
                    match desc {
                        Value::Null => s.push_str(&format!("  {label}\n")),
                        d => s.push_str(&format!("  {label} = {}\n", part(d))),
                    }
                }
            }
        }
        (_, "score") => {
            s.push_str(&format!("{name}: {instructions}\n"));
            let levels: Vec<String> = criteria
                .and_then(Value::as_array)
                .map(|a| a.iter().map(part).collect())
                .unwrap_or_default();
            let one_line = levels.join(" < ");
            if one_line.chars().count() <= 60 {
                s.push_str(&format!("  {one_line}\n"));
            } else {
                for (i, level) in levels.iter().enumerate() {
                    if i == 0 {
                        s.push_str(&format!("  {level}\n"));
                    } else {
                        s.push_str(&format!("  < {level}\n"));
                    }
                }
            }
        }
        _ => s.push_str(&format!("{name}! {v}\n")),
    }
    s
}

/// A value as it goes on a sketch line: plain text when it can be read back as itself, a JSON
/// literal when it could not (structured values, or text the notation would otherwise split).
fn part(v: &Value) -> String {
    match v {
        Value::String(s)
            if !s.contains(['|', '<', '=', '\n', '\r'])
                && !s.starts_with(['{', '[', '"', '#', '@', '-', '*', '•'])
                && head(s).is_none()
                && s.trim() == s
                && !s.is_empty() =>
        {
            s.clone()
        }
        other => other.to_string(),
    }
}

/// A sketch, highlighted for the transcript: heads bold in their kind's colour, bodies in it.
pub fn highlight(text: &str) -> Vec<Line<'static>> {
    let parsed = parse(text);
    text.split('\n')
        .zip(parsed.tags.iter())
        .map(|(line, tag)| {
            let style = match tag {
                t if t.is_head() => Style::new().fg(t.color()).add_modifier(Modifier::BOLD),
                Tag::State => Style::new(),
                Tag::Rule | Tag::Comment | Tag::Blank => Style::new().fg(DIM),
                t => Style::new().fg(t.color()),
            };
            Line::from(vec![Span::raw("  "), Span::styled(line.to_owned(), style)])
        })
        .collect()
}
