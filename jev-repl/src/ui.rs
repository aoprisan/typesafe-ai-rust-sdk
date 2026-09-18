//! Layout: a status strip, the transcript, a live view of the session, and the input line.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, COMMANDS};
use crate::builder::{Builder, Field};
use crate::editor::Preview;
use crate::format::*;
use crate::sketch::Tag;
use crate::{codegen, highlight, lessons, mock, wrap};

/// Width of the label column in builder mode.
const LABEL: usize = 14;

const SPINNER: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];

pub fn render(frame: &mut Frame, app: &mut App) {
    if app.sketch.is_some() {
        sketch(frame, frame.area(), app);
        return;
    }
    let [top, body, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let [left, right] =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(36)]).areas(body);

    status(frame, top, app);
    transcript(frame, left, app);
    panel(frame, right, app);
    prompt(frame, input, app);
    if app.builder.is_some() {
        builder(frame, frame.area(), app);
    }
}

fn status(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(
            " jev ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        dim("│ "),
        Span::raw(app.model_name()),
        dim(" │ "),
    ];
    spans.push(if app.mock {
        Span::styled("MOCK", Style::new().fg(WARN).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("LIVE", Style::new().fg(SCORE).add_modifier(Modifier::BOLD))
    });
    spans.push(dim(format!(
        " │ {} question(s) │ threshold {:.2} │ lesson {}/{}",
        app.session.questions.len(),
        app.threshold,
        app.lesson + 1,
        lessons::LESSONS.len()
    )));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn transcript(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Line::from(dim(" transcript ")));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let height = inner.height as usize;
    let lines = wrap::wrap_all(&app.transcript, width);

    // `scroll` counts lines back from the tail, so new output stays in view at rest.
    let max_scroll = lines.len().saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(height);
    frame.render_widget(
        Paragraph::new(Text::from(lines[start..end].to_vec())),
        inner,
    );

    if app.scroll > 0 {
        let hint = format!(" {} line(s) below · Esc to follow ", app.scroll);
        let w = (hint.len() as u16).min(area.width.saturating_sub(2));
        let rect = Rect::new(
            area.x + area.width.saturating_sub(w + 1),
            area.y + area.height.saturating_sub(1),
            w,
            1,
        );
        frame.render_widget(Paragraph::new(Line::from(dim(hint))), rect);
    }
}

fn panel(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Line::from(dim(" session ")));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "state",
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    if app.session.state_is_empty() {
        lines.push(Line::from(dim("(empty — type some text)")));
    } else {
        let preview = app.session.state_preview();
        for line in wrap::wrap(&Line::from(preview), inner.width as usize)
            .into_iter()
            .take(6)
        {
            lines.push(line);
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "questions",
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    if app.session.questions.is_empty() {
        lines.push(Line::from(dim("(none — :noul :choice :score)")));
    }
    for (name, question) in &app.session.questions {
        let kind = serde_json::to_value(question)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
            .unwrap_or_else(|| "raw".into());
        lines.push(Line::from(vec![
            Span::styled("• ", Style::new().fg(color_for(&kind))),
            Span::raw(name.clone()),
            dim(format!("  {kind}")),
        ]));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("lesson {}", app.lesson + 1),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(dim(lessons::LESSONS[app.lesson].title)));
    if let Some(suggested) = &app.suggested {
        for line in wrap::wrap(
            &Line::from(Span::styled(
                format!("^T  {suggested}"),
                Style::new().fg(SCORE),
            )),
            inner.width as usize,
        )
        .into_iter()
        .take(4)
        {
            lines.push(line);
        }
    }

    lines.push(Line::default());
    for hint in [
        "Enter   send the session",
        "^T/^N   try / next lesson",
        ":help   every command",
        ":json   the request body",
        ":rust   this session as code",
    ] {
        lines.push(Line::from(dim(hint)));
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn prompt(frame: &mut Frame, area: Rect, app: &App) {
    let title = if app.pending {
        Line::from(vec![
            Span::styled(
                format!(" {} ", SPINNER[app.spinner % SPINNER.len()]),
                Style::new().fg(ACCENT),
            ),
            dim("waiting for the API "),
        ])
    } else {
        Line::from(dim(" ask "))
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if app.pending { ACCENT } else { DIM }))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width.saturating_sub(2) as usize;
    let chars: Vec<char> = app.input.chars().collect();
    let offset = app.cursor.saturating_sub(width);
    let visible: String = chars[offset.min(chars.len())..].iter().collect();

    let line = if app.input.is_empty() {
        Line::from(vec![
            Span::styled("› ", Style::new().fg(ACCENT)),
            dim("type text to set the state, :help for commands, Enter to send"),
        ])
    } else {
        let mut spans = vec![Span::styled("› ", Style::new().fg(ACCENT))];
        spans.extend(highlight::command(&visible, |cmd| {
            COMMANDS.iter().any(|(c, _)| *c == cmd)
        }));
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), inner);
    frame.set_cursor_position((inner.x + 2 + (app.cursor - offset) as u16, inner.y));
}

/// The builder-mode popup: a form on the left, the JSON it produces on the right.
fn builder(frame: &mut Frame, area: Rect, app: &App) {
    let Some(b) = app.builder.as_ref() else {
        return;
    };
    let popup = centered(area, 92, 86);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(Span::styled(
            " build a question ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(dim(
            " Tab move · ^O add row · ^X drop row · ^S add question · Esc close ",
        )));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [form_area, gutter, preview_area] = Layout::horizontal([
        Constraint::Percentage(55),
        Constraint::Length(2),
        Constraint::Min(20),
    ])
    .areas(inner);
    frame.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(DIM)),
        Rect {
            x: gutter.x + 1,
            ..gutter
        },
    );

    let (lines, cursor) = form(b, form_area.width as usize);
    frame.render_widget(Paragraph::new(Text::from(lines)), form_area);
    if let Some((col, row)) = cursor
        && row < form_area.height as usize
    {
        frame.set_cursor_position((
            form_area.x + (col as u16).min(form_area.width.saturating_sub(1)),
            form_area.y + row as u16,
        ));
    }

    let [json_area, command_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(6)]).areas(preview_area);

    let pretty = serde_json::to_string_pretty(&b.preview()).unwrap_or_default();
    let mut json_lines = vec![Line::from(dim("questions"))];
    json_lines.extend(highlight::json(&pretty));
    frame.render_widget(Paragraph::new(Text::from(json_lines)), json_area);

    let mut tail = vec![Line::from(dim("same thing, one line"))];
    let command = b.as_command();
    tail.extend(wrap::wrap(
        &Line::from(highlight::command(&command, |c| {
            COMMANDS.iter().any(|(k, _)| *k == c)
        })),
        command_area.width as usize,
    ));
    if let Some(message) = &b.message {
        tail.push(Line::default());
        tail.push(Line::from(Span::styled(
            message.clone(),
            Style::new().fg(BAD),
        )));
    }
    frame.render_widget(Paragraph::new(Text::from(tail)), command_area);
}

/// The form rows, plus where the terminal cursor belongs.
fn form(b: &Builder, width: usize) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor = None;
    let focused = b.focused();
    let field_width = width.saturating_sub(LABEL + 1).max(8);

    let row = |lines: &mut Vec<Line<'static>>,
               cursor: &mut Option<(usize, usize)>,
               label: &str,
               field: Field,
               hint: &str| {
        let is_focused = field == focused;
        let text = b.text(field);
        let (shown, offset) = view(text, b.cursor, field_width, is_focused);
        let mut spans = vec![Span::styled(
            format!("{label:LABEL$}"),
            Style::new().fg(if is_focused { ACCENT } else { DIM }),
        )];
        if shown.is_empty() && !hint.is_empty() {
            spans.push(dim(hint.to_owned()));
        } else {
            spans.push(Span::styled(
                shown,
                if is_focused {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                },
            ));
        }
        if is_focused {
            *cursor = Some((LABEL + b.cursor.saturating_sub(offset), lines.len()));
        }
        lines.push(Line::from(spans));
    };

    row(
        &mut lines,
        &mut cursor,
        "state",
        Field::State,
        "the text being judged",
    );
    lines.push(Line::default());
    row(
        &mut lines,
        &mut cursor,
        "name",
        Field::Name,
        "answers come back under this",
    );

    // The type row is a cycler, not a text field.
    let kind_focused = focused == Field::Kind;
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:LABEL$}", "type"),
            Style::new().fg(if kind_focused { ACCENT } else { DIM }),
        ),
        Span::styled(
            format!("‹ {} ›", b.kind.label()),
            Style::new()
                .fg(color_for(b.kind.label()))
                .add_modifier(Modifier::BOLD),
        ),
        dim(format!("  {}", b.kind.about())),
    ]));
    if kind_focused {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(LABEL)),
            dim("← → or n/c/s to switch"),
        ]));
    }
    row(
        &mut lines,
        &mut cursor,
        "instructions",
        Field::Instructions,
        "what the model should decide",
    );
    lines.push(Line::default());

    match b.kind {
        crate::builder::Kind::Noul => {
            row(&mut lines, &mut cursor, "yes means", Field::Yes, "optional");
            row(&mut lines, &mut cursor, "no means", Field::No, "optional");
        }
        crate::builder::Kind::Choice => {
            for i in 0..b.options.len() {
                row(
                    &mut lines,
                    &mut cursor,
                    &format!("option {}", i + 1),
                    Field::OptionLabel(i),
                    "label",
                );
                row(
                    &mut lines,
                    &mut cursor,
                    "  describe",
                    Field::OptionDesc(i),
                    "optional, but this is what sharpens it",
                );
            }
        }
        crate::builder::Kind::Score => {
            for i in 0..b.levels.len() {
                row(
                    &mut lines,
                    &mut cursor,
                    &format!("level {i}"),
                    Field::Level(i),
                    if i == 0 { "lowest" } else { "" },
                );
            }
        }
    }
    (lines, cursor)
}

/// Slide a long value so the cursor stays visible; returns the text and the column it starts at.
fn view(text: &str, cursor: usize, width: usize, focused: bool) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < width {
        return (text.to_owned(), 0);
    }
    if !focused {
        let cut: String = chars[..width.saturating_sub(1)].iter().collect();
        return (format!("{cut}…"), 0);
    }
    let offset = cursor.saturating_sub(width.saturating_sub(1));
    (chars[offset.min(chars.len())..].iter().collect(), offset)
}

/// Width of the sketch gutter: a six-letter tag, a problem mark, and the rule.
const GUTTER: usize = 8;

/// Sketch mode: the page on the left with a gutter saying what each line became, a preview on
/// the right, and a status line that explains whatever the cursor is on.
fn sketch(frame: &mut Frame, area: Rect, app: &mut App) {
    let threshold = app.threshold;
    let default_model = app.model_name();
    let Some(ed) = app.sketch.as_mut() else {
        return;
    };
    let parsed = ed.parsed();

    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(Span::styled(
            " sketch · the request as one page ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(dim(
            " ^S apply · ^G apply & send · ^P preview · ^X/^U cut/paste line · Alt-↑↓ move line · Esc close ",
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [page_area, status_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(inner);
    let [edit_area, gutter, preview_area] = Layout::horizontal([
        Constraint::Percentage(56),
        Constraint::Length(2),
        Constraint::Min(24),
    ])
    .areas(page_area);
    frame.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(DIM)),
        Rect {
            x: gutter.x + 1,
            ..gutter
        },
    );

    // ---- the page ----
    let height = edit_area.height as usize;
    if height > 0 {
        if ed.row < ed.top {
            ed.top = ed.row;
        } else if ed.row >= ed.top + height {
            ed.top = ed.row + 1 - height;
        }
    }
    let text_width = (edit_area.width as usize).saturating_sub(GUTTER).max(8);
    let state_empty = parsed.tags.iter().all(|t| !matches!(t, Tag::State));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor = None;
    let mut prev = None;
    for (i, line) in ed.lines.iter().enumerate().skip(ed.top).take(height) {
        let tag = parsed.tags.get(i).copied().unwrap_or(Tag::Blank);
        let is_cur = i == ed.row;
        let problem = parsed.problem_at(i).is_some();
        // A long state reads better labelled once.
        let label = if tag == Tag::State && prev == Some(Tag::State) {
            ""
        } else {
            tag.label()
        };
        prev = Some(tag);

        let tag_style = Style::new().fg(tag.color());
        let mut spans = vec![
            Span::styled(
                format!("{label:<6}"),
                if tag.is_head() {
                    tag_style.add_modifier(Modifier::BOLD)
                } else {
                    tag_style
                },
            ),
            Span::styled(
                if problem { "!" } else { " " },
                Style::new().fg(BAD).add_modifier(Modifier::BOLD),
            ),
            Span::styled("│", Style::new().fg(if is_cur { ACCENT } else { DIM })),
        ];
        let (shown, offset) = view(line, ed.col, text_width, is_cur);
        if shown.is_empty() && i == 0 && state_empty {
            spans.push(dim("the state — the text or JSON the questions are about"));
        } else {
            let style = match tag {
                t if t.is_head() => Style::new().fg(t.color()).add_modifier(Modifier::BOLD),
                Tag::Rule | Tag::Comment => Style::new().fg(DIM),
                Tag::State | Tag::Blank => Style::new(),
                t => Style::new().fg(t.color()),
            };
            spans.push(Span::styled(shown, style));
        }
        if is_cur {
            cursor = Some((GUTTER + ed.col.saturating_sub(offset), lines.len()));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), edit_area);
    if let Some((col, row)) = cursor {
        frame.set_cursor_position((
            edit_area.x + (col as u16).min(edit_area.width.saturating_sub(1)),
            edit_area.y + row as u16,
        ));
    }

    // ---- the preview ----
    let mut tabs: Vec<Span<'static>> = Vec::new();
    for (i, p) in Preview::ALL.iter().enumerate() {
        if i > 0 {
            tabs.push(dim(" · "));
        }
        tabs.push(if *p == ed.preview {
            Span::styled(
                p.label(),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
        } else {
            dim(p.label())
        });
    }
    tabs.push(dim("   ^P"));
    if !parsed.problems.is_empty() {
        tabs.push(Span::styled(
            format!("   {} problem(s)", parsed.problems.len()),
            Style::new().fg(BAD),
        ));
    }
    let mut preview: Vec<Line<'static>> = vec![Line::from(tabs), Line::default()];
    // Problems go first: they are what to act on, and a long preview must not hide them.
    if !parsed.problems.is_empty() {
        preview.push(Line::from(Span::styled(
            "problems",
            Style::new().fg(BAD).add_modifier(Modifier::BOLD),
        )));
        for p in parsed.problems.iter().take(6) {
            preview.push(Line::from(vec![
                Span::styled(format!("  {:>3}  ", p.line + 1), Style::new().fg(BAD)),
                Span::raw(p.message.clone()),
            ]));
        }
        preview.push(Line::default());
    }

    let session = parsed.to_session();
    let model = session.model.clone().unwrap_or(default_model);
    match ed.preview {
        Preview::Json => preview.extend(highlight::json(&session.request_json(&model))),
        Preview::Answers => {
            if session.questions.is_empty() {
                preview.push(Line::from(dim(
                    "  add a question below the --- line to see the shape of its answer",
                )));
            } else {
                preview.push(Line::from(dim(
                    "  simulated answers — the shape is real, the numbers are not",
                )));
                for (name, q) in &session.questions {
                    let json = serde_json::to_value(q).unwrap_or_default();
                    match mock::answer(&session.state, name, &json) {
                        Some(a) => preview.extend(answer_lines(name, &a, threshold)),
                        None => preview.push(Line::from(dim(format!(
                            "  {name}: no simulation for this question shape"
                        )))),
                    }
                }
            }
        }
        Preview::Rust => {
            preview.extend(highlight::rust(&codegen::rust(&session, &model, threshold)))
        }
    }
    let wrapped = wrap::wrap_all(&preview, preview_area.width as usize);
    frame.render_widget(Paragraph::new(Text::from(wrapped)), preview_area);

    // ---- the status line: what the cursor is on ----
    let tag = parsed.tags.get(ed.row).copied().unwrap_or(Tag::Blank);
    let below_rule = parsed
        .tags
        .iter()
        .position(|t| *t == Tag::Rule)
        .is_some_and(|r| ed.row > r);
    let status = if let Some(m) = &ed.message {
        Span::styled(m.clone(), Style::new().fg(WARN))
    } else if let Some(p) = parsed.problem_at(ed.row) {
        Span::styled(p.message.clone(), Style::new().fg(BAD))
    } else {
        dim(hint_for(tag, below_rule))
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::raw(" "), status])),
        status_area,
    );
}

/// One line about the kind of line under the cursor — the notation explains itself in place.
fn hint_for(tag: Tag, below_rule: bool) -> &'static str {
    match tag {
        Tag::State => "state — the text or JSON the questions are about; a --- line ends it",
        Tag::Rule => "--- separates the state above from the questions below",
        Tag::Blank if !below_rule => {
            "state — the text or JSON the questions are about; a --- line ends it"
        }
        Tag::Blank => {
            "name? asks yes/no · name: then `label = why` lines (choice) or `low < high` (score) · name! {json} sends it raw"
        }
        Tag::Comment => "a comment — ignored",
        Tag::Model => "@model pins the model this session sends to",
        Tag::Noul => {
            "noul — the answer is the probability of yes; `yes:` and `no:` lines say what each means"
        }
        Tag::Yes | Tag::No => "what a yes or a no means — sharper criteria, higher confidence",
        Tag::Choice => "choice — one label out of these options; `label = why` describes each",
        Tag::Option => "an option — `label = when it applies`; a bare label works but is vaguer",
        Tag::Score => {
            "score — ordered levels, lowest first; the answer is a weighted position along them"
        }
        Tag::Level => "a level — write them lowest to highest, joined with <",
        Tag::Raw => "raw — a JSON object sent as it is; it needs a `type`",
        Tag::Json => "continues the raw JSON above",
        Tag::Stray => "this line could not be placed",
    }
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}
