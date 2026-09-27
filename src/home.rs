//! Home: the logo, recent quizzes, and the way into a new one.

use crate::app::{Ctx, Transition, centered, hint_line, music_badge};
use crate::config;
use crate::model::{Entry, Quiz};
use crate::picker::truncate;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};
use std::path::PathBuf;
use std::time::SystemTime;
use tui_big_text::{BigText, PixelSize};

struct Item {
    path: PathBuf,
    summary: Result<(String, String), String>,
    edited: Option<SystemTime>,
}

pub struct Home {
    items: Vec<Item>,
    list: ListState,
    open_input: Option<String>,
    help: bool,
    error: Option<String>,
}

impl Home {
    pub fn new() -> Self {
        let mut h = Home { items: Vec::new(), list: ListState::default(), open_input: None, help: false, error: None };
        h.refresh();
        h
    }

    /// Recently opened quizzes, then any other `*.quiz.toml` in this folder.
    fn refresh(&mut self) {
        let mut paths = config::recent();
        if let Ok(dir) = std::fs::read_dir(".") {
            let mut here: Vec<PathBuf> = dir
                .filter_map(|e| e.ok()?.path().canonicalize().ok())
                .filter(|p| p.to_string_lossy().ends_with(".quiz.toml") && !paths.contains(p))
                .collect();
            here.sort();
            paths.extend(here);
        }
        self.items = paths
            .into_iter()
            .map(|path| {
                let edited = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                let summary = Quiz::load(&path).map(|q| (q.meta.title.clone(), stats(&q))).map_err(|e| format!("{e:#}"));
                Item { path, summary, edited }
            })
            .collect();
        self.list.select((!self.items.is_empty()).then_some(0));
    }

    pub fn key(&mut self, k: KeyEvent) -> Transition {
        if let Some(input) = &mut self.open_input {
            match k.code {
                KeyCode::Esc => self.open_input = None,
                KeyCode::Backspace => drop(input.pop()),
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    let path = PathBuf::from(self.open_input.take().unwrap().trim());
                    return self.open(path);
                }
                _ => {}
            }
            return Transition::None;
        }
        if self.help {
            self.help = false;
            return Transition::None;
        }
        self.error = None;
        let n = self.items.len();
        let sel = self.list.selected().unwrap_or(0);
        match k.code {
            KeyCode::Char('q') => return Transition::Quit,
            KeyCode::Char('n') => return Transition::Picker,
            KeyCode::Char('o') => self.open_input = Some(String::new()),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Up | KeyCode::Char('k') if n > 0 => self.list.select(Some(sel.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.list.select(Some((sel + 1).min(n - 1))),
            KeyCode::Enter if n > 0 => return self.open(self.items[sel].path.clone()),
            _ => {}
        }
        Transition::None
    }

    fn open(&mut self, path: PathBuf) -> Transition {
        match Quiz::load(&path) {
            Ok(q) => Transition::Open(Box::new(q), path),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                Transition::None
            }
        }
    }

    pub fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let g = ctx.glyphs;
        let [main, status] = Layout::vertical([Constraint::Min(10), Constraint::Length(1)]).areas(f.area());
        let block = t.frame("", true);
        let inner = block.inner(main);
        f.render_widget(block, main);
        let [logo, _, title, list, keys] = Layout::vertical([
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(inner.inner(ratatui::layout::Margin::new(2, 1)));

        let [big, tag] = Layout::horizontal([Constraint::Length(17), Constraint::Min(10)]).areas(logo);
        f.render_widget(BigText::builder().pixel_size(PixelSize::Full).style(t.accent()).lines(vec!["MQ".into()]).build(), big);
        f.render_widget(Paragraph::new(vec![Line::raw(""), Line::raw(""), Line::raw(""), Line::styled("  music quiz", t.clip())]), tag);

        f.render_widget(Paragraph::new(Line::styled("Recent quizzes", t.dim())), title);
        let w = list.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = if self.items.is_empty() {
            vec![ListItem::new(Line::styled("No quizzes yet: press n to make one from a playlist", t.dim()))]
        } else {
            self.items
                .iter()
                .map(|it| {
                    let edited = it.edited.map_or(String::new(), |e| format!("edited {}", ago(e)));
                    let (name, detail, style) = match &it.summary {
                        Ok((title, stats)) => (title.clone(), stats.clone(), t.base()),
                        Err(e) => (it.path.file_name().map_or(String::new(), |n| n.to_string_lossy().into()), e.clone(), t.warn()),
                    };
                    let nw = (w / 3).max(12);
                    let ew = 20;
                    let dw = w.saturating_sub(nw + ew + 2);
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<nw$} ", truncate(&name, nw)), style),
                        Span::styled(format!("{:<dw$} ", truncate(&detail, dw)), t.dim()),
                        Span::styled(format!("{edited:>ew$}"), t.dim()),
                    ]))
                })
                .collect()
        };
        f.render_stateful_widget(
            List::new(items).highlight_style(t.selected()).highlight_symbol(format!("{} ", g.cursor)),
            list,
            &mut self.list,
        );
        let hint = match &self.error {
            Some(e) => Line::styled(e.clone(), t.warn()),
            None => hint_line(ctx, &[("Enter", "open"), ("n", "new quiz from a playlist"), ("o", "open file…"), ("q", "quit")]),
        };
        f.render_widget(Paragraph::new(hint), keys);

        let badge = music_badge(ctx);
        let help = " ? help ";
        let pad = (status.width as usize).saturating_sub(badge.width() + help.len() + 1);
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::raw(" "), badge, Span::raw(" ".repeat(pad)), Span::styled(help, t.dim())])).style(t.base()),
            status,
        );

        if let Some(input) = &self.open_input {
            let area = centered(f.area(), 70, 4);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![Span::raw(input.clone()), Span::styled("▏", t.accent())]),
                    Line::styled("path to a .quiz.toml file   Enter open   Esc cancel", t.dim()),
                ])
                .block(t.frame(" Open quiz ", true)),
                area,
            );
        }
        if self.help {
            crate::app::draw_keys(
                f,
                ctx,
                &[("Home", &[("↑↓ j k", "select quiz"), ("Enter", "open it"), ("n", "new quiz from a playlist"), ("o", "open a file by path"), ("r", "refresh the list"), ("q", "quit")])],
            );
        }
    }
}

fn stats(q: &Quiz) -> String {
    let songs = q.songs().filter(|s| !s.skip).count();
    let rounds = q.entries.iter().filter(|e| matches!(e, Entry::Heading(_))).count();
    let questions: usize = q.songs().filter(|s| !s.skip).map(|s| s.questions.len()).sum();
    let rounds = match rounds {
        0 => "no rounds".to_string(),
        1 => "1 round".to_string(),
        n => format!("{n} rounds"),
    };
    format!("{songs} songs · {rounds} · {questions} questions")
}

fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        86400..2_592_000 => (s / 86400, "day"),
        2_592_000..31_536_000 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}
