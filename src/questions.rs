//! Question editor: the add picker's prefilled questions and the edit dialog.

use crate::app::{Ctx, centered};
use crate::model::{Question, Song};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::collections::BTreeMap;

/// Kinds offered by the `a` picker, in order.
pub const KINDS: [(&str, &str); 5] =
    [("artist", "Artist"), ("title", "Title"), ("year", "Year"), ("album", "Album"), ("custom", "Custom")];

/// Built-in prompt for a question kind; a quiz can override it in its settings.
pub fn default_prompt(kind: &str) -> &'static str {
    match kind {
        "artist" => "Vem är artisten?",
        "title" => "Vad heter låten?",
        "year" => "Vilket år släpptes låten?",
        "album" => "Vilket album kommer låten från?",
        _ => "",
    }
}

/// A question of `kind` with the quiz's prompt and the answer from Music.app metadata.
pub fn prefilled(kind: &str, s: &Song, album: &str, prompts: &BTreeMap<String, String>) -> Question {
    let answer = match kind {
        "artist" => s.artist.clone(),
        "title" => s.title.clone(),
        "year" if s.year > 0 => s.year.to_string(),
        "album" => album.to_string(),
        _ => String::new(),
    };
    let prompt = prompts.get(kind).map_or(default_prompt(kind), String::as_str);
    Question { kind: kind.into(), prompt: prompt.into(), answer, points: 1.0 }
}

pub enum Outcome {
    Editing,
    Save,
    Cancel,
}

pub struct Editor {
    pub q: Question,
    /// Position in the song's questions; `None` for a new one.
    pub index: Option<usize>,
    field: usize,
}

const PROMPT: usize = 0;
const ANSWER: usize = 1;
const POINTS: usize = 2;

impl Editor {
    pub fn new(q: Question, index: Option<usize>) -> Self {
        // A custom question starts in the empty prompt; prefilled ones in the answer.
        let field = if q.prompt.is_empty() { PROMPT } else { ANSWER };
        Editor { q, index, field }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Char('s') if ctrl => return Outcome::Save,
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + 2) % 3,
            // The prompt may be several lines (a hint under the question).
            KeyCode::Enter if self.field == PROMPT => self.q.prompt.push('\n'),
            KeyCode::Enter => return Outcome::Save,
            KeyCode::Left | KeyCode::Char('-') if self.field == POINTS => self.q.points = (self.q.points - 0.5).max(0.0),
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') if self.field == POINTS => self.q.points += 0.5,
            KeyCode::Backspace => drop(self.text().map(|t| t.pop())),
            KeyCode::Char(c) if !ctrl => {
                if let Some(t) = self.text() {
                    t.push(c)
                }
            }
            _ => {}
        }
        Outcome::Editing
    }

    fn text(&mut self) -> Option<&mut String> {
        match self.field {
            PROMPT => Some(&mut self.q.prompt),
            ANSWER => Some(&mut self.q.answer),
            _ => None,
        }
    }

    pub fn draw(&self, f: &mut Frame, ctx: &Ctx, label: &str) {
        let t = ctx.theme;
        let field = |i: usize, name: &str| {
            Span::styled(format!("{name:<8}"), if self.field == i { t.key() } else { t.dim() })
        };
        let cursor = |i: usize| if self.field == i { Span::styled("▏", t.accent()) } else { Span::raw("") };
        let mut lines = Vec::new();
        let prompt: Vec<&str> = self.q.prompt.split('\n').collect();
        for (n, part) in prompt.iter().enumerate() {
            let mut spans = vec![if n == 0 { field(PROMPT, "Prompt") } else { Span::raw(" ".repeat(8)) }, Span::raw(part.to_string())];
            if n == prompt.len() - 1 {
                spans.push(cursor(PROMPT));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(vec![field(ANSWER, "Answer"), Span::styled(self.q.answer.clone(), t.clip()), cursor(ANSWER)]));
        lines.push(Line::from(vec![
            field(POINTS, "Points"),
            Span::styled("◀ ", t.dim()),
            Span::raw(fmt_points(self.q.points)),
            Span::styled(" ▶", t.dim()),
        ]));
        lines.push(Line::raw(""));
        if self.q.kind == "year" {
            lines.push(Line::styled("Check the year: remasters and compilations often carry the wrong one.", t.warn()));
        }
        let hint = if self.field == PROMPT { " Enter new line  " } else { " Enter save  " };
        lines.push(Line::from(vec![
            Span::styled("Tab", t.key()),
            Span::styled(" next field ", t.dim()),
            Span::styled(if self.field == POINTS { " ← →" } else { "" }, t.key()),
            Span::styled(if self.field == POINTS { " points " } else { "" }, t.dim()),
            Span::styled(" Ctrl-S", t.key()),
            Span::styled(" save", t.dim()),
            Span::styled(hint, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" cancel", t.dim()),
        ]));
        let area = centered(f.area(), 76, lines.len() as u16 + 2);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(t.frame(format!(" {label} "), true)), area);
    }
}

pub fn fmt_points(p: f64) -> String {
    if p.fract() == 0.0 { format!("{p:.0}") } else { format!("{p:.1}") }
}

/// The `a` picker: which kind of question to add.
pub fn draw_picker(f: &mut Frame, ctx: &Ctx, label: &str, sel: usize) {
    let t = ctx.theme;
    let mut lines: Vec<Line> = KINDS
        .iter()
        .enumerate()
        .map(|(i, (kind, name))| {
            let style = if i == sel { t.selected() } else { t.base() };
            Line::from(vec![
                Span::styled(if i == sel { format!("{} ", ctx.glyphs.cursor) } else { "  ".into() }, t.accent()),
                Span::styled(format!("{}  ", i + 1), t.key()),
                Span::styled(format!("{name:<10}"), style),
                Span::styled(if *kind == "custom" { "" } else { "(prefilled)" }, t.dim()),
            ])
        })
        .collect();
    lines.push(Line::styled("Enter or 1-5 add   Esc cancel", t.dim()));
    let area = centered(f.area(), 40, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(t.frame(format!(" Add question to {label} "), true)), area);
}
