//! Word wrapping that keeps span styles, so the transcript can be scrolled by exact line count.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Wrap one styled line to `width` columns. Continuation lines keep the original indentation.
pub fn wrap(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }
    let indent = line
        .spans
        .first()
        .map(|s| s.content.len() - s.content.trim_start().len())
        .unwrap_or(0)
        .min(width / 2);

    let mut w = Wrapper {
        width,
        indent,
        lines: Vec::new(),
        cur: Vec::new(),
        col: 0,
    };
    for span in &line.spans {
        let mut rest: &str = &span.content;
        while !rest.is_empty() {
            let end = rest.find(' ').unwrap_or(rest.len());
            if end == 0 {
                w.space(span.style);
                rest = &rest[1..];
            } else {
                w.word(&rest[..end], span.style);
                rest = &rest[end..];
            }
        }
    }
    w.finish(line.style)
}

/// Wrap every line and return them in order — what the transcript pane actually draws.
pub fn wrap_all(lines: &[Line<'static>], width: usize) -> Vec<Line<'static>> {
    lines.iter().flat_map(|l| wrap(l, width)).collect()
}

struct Wrapper {
    width: usize,
    indent: usize,
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    col: usize,
}

impl Wrapper {
    fn word(&mut self, word: &str, style: Style) {
        let len = word.chars().count();
        if self.col + len > self.width && self.col > self.indent {
            self.newline();
        }
        if len > self.width {
            // A single word longer than the pane: hard-break it.
            let mut chars = word.chars().peekable();
            while chars.peek().is_some() {
                let room = self.width.saturating_sub(self.col).max(1);
                let chunk: String = chars.by_ref().take(room).collect();
                self.col += chunk.chars().count();
                self.cur.push(Span::styled(chunk, style));
                if chars.peek().is_some() {
                    self.newline();
                }
            }
            return;
        }
        self.cur.push(Span::styled(word.to_owned(), style));
        self.col += len;
    }

    fn space(&mut self, style: Style) {
        if self.col == 0 || self.col >= self.width {
            // Leading indentation is re-applied by `newline`; trailing spaces are dropped.
            if self.col == 0 && self.cur.is_empty() && self.lines.is_empty() {
                self.cur.push(Span::styled(" ".to_owned(), style));
                self.col += 1;
            }
            return;
        }
        self.cur.push(Span::styled(" ".to_owned(), style));
        self.col += 1;
    }

    fn newline(&mut self) {
        let spans = std::mem::take(&mut self.cur);
        self.lines.push(Line::from(spans));
        self.col = 0;
        if self.indent > 0 {
            self.cur.push(Span::raw(" ".repeat(self.indent)));
            self.col = self.indent;
        }
    }

    fn finish(mut self, style: Style) -> Vec<Line<'static>> {
        let spans = std::mem::take(&mut self.cur);
        if !spans.is_empty() || self.lines.is_empty() {
            self.lines.push(Line::from(spans));
        }
        self.lines
            .into_iter()
            .map(|l| l.patch_style(style))
            .collect()
    }
}
