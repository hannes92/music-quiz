//! Quiz settings dialog (`,`): title, sheet texts, clip defaults and prompts.

use crate::app::{Ctx, centered};
use crate::model::QuizFile;
use crate::questions::{KINDS, default_prompt};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

enum Field {
    Text { label: &'static str, value: String, placeholder: String },
    Number { label: &'static str, value: u32, step: u32, min: u32 },
    Kinds { label: &'static str, on: Vec<bool> },
    Toggle { label: &'static str, on: bool },
}

pub enum Outcome {
    Editing,
    Save,
    Cancel,
}

pub struct Settings {
    fields: Vec<Field>,
    sel: usize,
}

/// The four kinds that can be prefilled (Custom has no default).
const PREFILL: usize = 4;
/// Index of the first prompt field.
const FIRST_PROMPT: usize = 7;

impl Settings {
    pub fn new(meta: &QuizFile) -> Self {
        let d = &meta.defaults;
        let mut fields = vec![
            Field::Text { label: "Title", value: meta.title.clone(), placeholder: String::new() },
            Field::Text { label: "Subtitle", value: meta.subtitle.clone().unwrap_or_default(), placeholder: "theme, e.g. Film/TV-serier".into() },
            Field::Text {
                label: "Instructions",
                value: meta.instructions.clone().unwrap_or_default(),
                placeholder: "line above the questions on the sheet".into(),
            },
            Field::Number { label: "Clip length", value: d.clip_length, step: 5, min: 5 },
            Field::Number { label: "Re-run length", value: d.rerun_length, step: 1, min: 3 },
            Field::Kinds { label: "Default questions", on: KINDS[..PREFILL].iter().map(|(k, _)| d.questions.iter().any(|q| q == k)).collect() },
            Field::Toggle { label: "Fade out clips", on: d.fade_out },
        ];
        for (kind, name) in &KINDS[..PREFILL] {
            fields.push(Field::Text {
                label: match *name {
                    "Artist" => "Prompt: artist",
                    "Title" => "Prompt: title",
                    "Year" => "Prompt: year",
                    _ => "Prompt: album",
                },
                value: d.prompts.get(*kind).cloned().unwrap_or_default(),
                placeholder: default_prompt(kind).into(),
            });
        }
        Settings { fields, sel: 0 }
    }

    pub fn apply(&self, meta: &mut QuizFile) {
        let text = |i: usize| match &self.fields[i] {
            Field::Text { value, .. } => value.trim().to_string(),
            _ => unreachable!(),
        };
        let num = |i: usize| match &self.fields[i] {
            Field::Number { value, .. } => *value,
            _ => unreachable!(),
        };
        if !text(0).is_empty() {
            meta.title = text(0);
        }
        meta.subtitle = Some(text(1)).filter(|s| !s.is_empty());
        meta.instructions = Some(text(2)).filter(|s| !s.is_empty());
        meta.defaults.clip_length = num(3);
        meta.defaults.rerun_length = num(4);
        if let Field::Toggle { on, .. } = &self.fields[6] {
            meta.defaults.fade_out = *on;
        }
        if let Field::Kinds { on, .. } = &self.fields[5] {
            meta.defaults.questions = KINDS[..PREFILL].iter().zip(on).filter(|(_, on)| **on).map(|((k, _), _)| k.to_string()).collect();
        }
        for (i, (kind, _)) in KINDS[..PREFILL].iter().enumerate() {
            let p = text(FIRST_PROMPT + i);
            if p.is_empty() || p == default_prompt(kind) {
                meta.defaults.prompts.remove(*kind);
            } else {
                meta.defaults.prompts.insert(kind.to_string(), p);
            }
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.fields.len();
        match (k.code, &mut self.fields[self.sel]) {
            (KeyCode::Esc, _) => return Outcome::Cancel,
            (KeyCode::Char('s'), _) if ctrl => return Outcome::Save,
            (KeyCode::Enter, _) => return Outcome::Save,
            (KeyCode::Down | KeyCode::Tab, _) => self.sel = (self.sel + 1) % n,
            (KeyCode::Up | KeyCode::BackTab, _) => self.sel = (self.sel + n - 1) % n,
            (KeyCode::Backspace, Field::Text { value, .. }) => drop(value.pop()),
            (KeyCode::Char(c), Field::Text { value, .. }) if !ctrl => value.push(c),
            (KeyCode::Left | KeyCode::Char('-'), Field::Number { value, step, min, .. }) => *value = value.saturating_sub(*step).max(*min),
            (KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('='), Field::Number { value, step, .. }) => *value += *step,
            (KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right, Field::Toggle { on, .. }) => *on = !*on,
            (KeyCode::Char(c @ '1'..='4'), Field::Kinds { on, .. }) => {
                let i = c as usize - '1' as usize;
                on[i] = !on[i];
            }
            _ => {}
        }
        Outcome::Editing
    }

    pub fn draw(&self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let mut lines = Vec::new();
        for (i, field) in self.fields.iter().enumerate() {
            let sel = i == self.sel;
            let (label, value): (&str, Vec<Span>) = match field {
                Field::Text { label, value, placeholder } => {
                    let v = if value.is_empty() && !sel {
                        Span::styled(placeholder.clone(), t.faint())
                    } else {
                        Span::raw(value.clone())
                    };
                    let mut spans = vec![v];
                    if sel {
                        spans.push(Span::styled("▏", t.accent()));
                        if value.is_empty() {
                            spans.push(Span::styled(placeholder.clone(), t.faint()));
                        }
                    }
                    (label, spans)
                }
                Field::Number { label, value, .. } => {
                    (label, vec![Span::styled("◀ ", t.dim()), Span::styled(format!("{value} s"), t.clip()), Span::styled(" ▶", t.dim())])
                }
                Field::Toggle { label, on } => {
                    (label, vec![Span::styled(if *on { "[x] on" } else { "[ ] off" }, if *on { t.clip() } else { t.dim() })])
                }
                Field::Kinds { label, on } => (
                    label,
                    KINDS[..PREFILL]
                        .iter()
                        .zip(on)
                        .enumerate()
                        .flat_map(|(i, ((_, name), on))| {
                            vec![
                                Span::styled(format!("{} ", i + 1), t.key()),
                                Span::styled(format!("[{}] {name}   ", if *on { "x" } else { " " }), if *on { t.clip() } else { t.dim() }),
                            ]
                        })
                        .collect(),
                ),
            };
            let mut spans = vec![
                Span::styled(if sel { format!("{} ", ctx.glyphs.cursor) } else { "  ".into() }, t.accent()),
                Span::styled(format!("{label:<19}"), if sel { t.key() } else { t.dim() }),
            ];
            spans.extend(value);
            lines.push(Line::from(spans));
            if i == FIRST_PROMPT - 1 {
                lines.push(Line::raw(""));
            }
        }
        lines.push(Line::raw(""));
        let hint = match self.fields[self.sel] {
            Field::Number { .. } => "← → change   ",
            Field::Kinds { .. } => "1-4 toggle   ",
            Field::Toggle { .. } => "space toggle   ",
            Field::Text { .. } => "type to edit   ",
        };
        lines.push(Line::from(vec![
            Span::styled(hint, t.dim()),
            Span::styled("↑↓", t.key()),
            Span::styled(" field   ", t.dim()),
            Span::styled("Enter", t.key()),
            Span::styled(" save   ", t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" cancel", t.dim()),
        ]));
        let area = centered(f.area(), 84, lines.len() as u16 + 2);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(t.frame(" Quiz settings ", true)), area);
    }
}
