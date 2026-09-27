//! Playlist picker: choose the Music.app playlist that becomes the quiz.

use crate::app::{Ctx, Transition, centered, header_line, hint_line, music_badge};
use crate::model::{self, Quiz};
use crate::music::Playlist;
use crate::theme::mmss;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(PartialEq)]
enum Focus {
    Playlists,
    Tracks,
}

struct NameDialog {
    title: String,
}

pub struct Picker {
    filter: String,
    filtering: bool,
    list: ListState,
    tracks: TableState,
    focus: Focus,
    moved_at: Instant,
    dialog: Option<NameDialog>,
    error: Option<String>,
    help: bool,
}

const PREVIEW_SECS: f64 = 15.0;

impl Picker {
    pub fn new(ctx: &Ctx) -> Self {
        ctx.lib.load_playlists();
        Picker {
            filter: String::new(),
            filtering: false,
            list: ListState::default().with_selected(Some(0)),
            tracks: TableState::default(),
            focus: Focus::Playlists,
            moved_at: Instant::now(),
            dialog: None,
            error: None,
            help: false,
        }
    }

    fn visible(&self, ctx: &Ctx) -> Vec<Playlist> {
        let Some(Ok(all)) = ctx.lib.playlists() else { return Vec::new() };
        let f = self.filter.to_lowercase();
        all.into_iter().filter(|p| fuzzy(&p.path().to_lowercase(), &f)).collect()
    }

    fn current(&self, ctx: &Ctx) -> Option<Playlist> {
        self.visible(ctx).get(self.list.selected()?).cloned()
    }

    pub fn tick(&mut self, ctx: &Ctx) {
        // Debounce: load the highlighted playlist's tracks once the cursor rests.
        if self.moved_at.elapsed() > Duration::from_millis(150)
            && let Some(p) = self.current(ctx)
        {
            ctx.lib.load_tracks(&p.id, false);
        }
    }

    pub fn key(&mut self, k: KeyEvent, ctx: &Ctx) -> Transition {
        if let Some(d) = &mut self.dialog {
            match k.code {
                KeyCode::Esc => self.dialog = None,
                KeyCode::Backspace => drop(d.title.pop()),
                KeyCode::Char(c) => d.title.push(c),
                KeyCode::Enter => return self.create(ctx),
                _ => {}
            }
            return Transition::None;
        }
        if self.help {
            self.help = false;
            return Transition::None;
        }
        if self.filtering {
            match k.code {
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                }
                KeyCode::Enter | KeyCode::Down => self.filtering = false,
                KeyCode::Backspace => drop(self.filter.pop()),
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.list.select(Some(0));
            self.moved_at = Instant::now();
            return Transition::None;
        }
        let n_lists = self.visible(ctx).len();
        let n_tracks = self
            .current(ctx)
            .and_then(|p| ctx.lib.tracks(&p.id))
            .and_then(|r| r.ok())
            .map_or(0, |t| t.len());
        match (k.code, &self.focus) {
            (KeyCode::Char('q'), _) => return Transition::Quit,
            (KeyCode::Char('?'), _) => self.help = true,
            (KeyCode::Esc, Focus::Tracks) | (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
                self.focus = if self.focus == Focus::Playlists { Focus::Tracks } else { Focus::Playlists };
                if self.tracks.selected().is_none() {
                    self.tracks.select(Some(0));
                }
            }
            (KeyCode::Esc, Focus::Playlists) => {
                ctx.player.stop();
                return Transition::Home;
            }
            (KeyCode::Char('/'), _) => {
                self.filtering = true;
                self.focus = Focus::Playlists;
            }
            (KeyCode::Up | KeyCode::Char('k'), Focus::Playlists) => self.move_list(-1, n_lists),
            (KeyCode::Down | KeyCode::Char('j'), Focus::Playlists) => self.move_list(1, n_lists),
            (KeyCode::Up | KeyCode::Char('k'), Focus::Tracks) => step(&mut self.tracks, -1, n_tracks),
            (KeyCode::Down | KeyCode::Char('j'), Focus::Tracks) => step(&mut self.tracks, 1, n_tracks),
            (KeyCode::Char(' '), _) => self.preview(ctx),
            (KeyCode::Char('r'), _) => {
                ctx.lib.load_playlists();
                if let Some(p) = self.current(ctx) {
                    ctx.lib.load_tracks(&p.id, true);
                }
            }
            (KeyCode::Enter, _) => {
                if let Some(p) = self.current(ctx) {
                    self.dialog = Some(NameDialog { title: p.name.clone() });
                }
            }
            _ => {}
        }
        Transition::None
    }

    fn move_list(&mut self, d: i32, n: usize) {
        step(&mut self.list, d, n);
        self.tracks.select(Some(0));
        self.moved_at = Instant::now();
    }

    /// Space plays 15 s from 30% into the track, to check it's the right playlist.
    fn preview(&mut self, ctx: &Ctx) {
        let Some(p) = self.current(ctx) else { return };
        let Some(Ok(tracks)) = ctx.lib.tracks(&p.id) else { return };
        let i = if self.focus == Focus::Tracks { self.tracks.selected().unwrap_or(0) } else { 0 };
        let Some(t) = tracks.get(i) else { return };
        let v = ctx.player.view();
        if ctx.player.is_active() && v.track_id.as_deref() == Some(&t.id) {
            ctx.player.stop();
        } else {
            ctx.player.play(&p.id, &t.id, (t.duration * 0.3).floor(), Some(PREVIEW_SECS));
        }
    }

    fn create(&mut self, ctx: &Ctx) -> Transition {
        let Some(d) = self.dialog.take() else { return Transition::None };
        let Some(p) = self.current(ctx) else { return Transition::None };
        let path = PathBuf::from(model::file_name_for(&d.title));
        if path.exists() {
            return match Quiz::load(&path) {
                Ok(q) => Transition::Open(Box::new(q), path),
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    Transition::None
                }
            };
        }
        let Some(Ok(tracks)) = ctx.lib.tracks(&p.id) else {
            self.error = Some("tracks still loading, try again".into());
            return Transition::None;
        };
        let quiz = Quiz::new(&d.title, &p, &tracks);
        match quiz.save(&path) {
            Ok(()) => Transition::Open(Box::new(quiz), path),
            Err(e) => {
                self.error = Some(format!("could not save {}: {e:#}", path.display()));
                Transition::None
            }
        }
    }

    pub fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let [head, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
        f.render_widget(header_line(ctx, vec![Span::raw("New quiz › Pick a playlist")], vec![music_badge(ctx)], head.width), head);

        let dir = if body.width >= 100 { Direction::Horizontal } else { Direction::Vertical };
        let [left, right] = Layout::default()
            .direction(dir)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .areas(body);

        // Left: playlists.
        let lists = self.visible(ctx);
        let filter_title = if self.filtering || !self.filter.is_empty() {
            Span::styled(format!(" /{}{} ", self.filter, if self.filtering { "▏" } else { "" }), t.accent())
        } else {
            Span::styled(" / filter ", t.dim())
        };
        let block = t
            .frame(Line::from(vec![Span::raw(" Playlists "), filter_title]), self.focus == Focus::Playlists);
        let inner_w = left.width.saturating_sub(4) as usize;
        let items: Vec<ListItem> = match ctx.lib.playlists() {
            None => vec![ListItem::new(Span::styled("Loading playlists from Music.app…", t.dim()))],
            Some(Err(e)) => vec![ListItem::new(Span::styled(e, t.warn()))],
            Some(Ok(_)) if lists.is_empty() => vec![ListItem::new(Span::styled("No playlists match", t.dim()))],
            Some(Ok(_)) => lists
                .iter()
                .map(|p| {
                    let meta = format!("{:>3} {:>6}", p.count, hours(p.duration));
                    let name = if p.smart { format!("{} ⚙", p.path()) } else { p.path() };
                    let w = inner_w.saturating_sub(meta.len() + 1);
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<w$} ", truncate(&name, w))),
                        Span::styled(meta, t.dim()),
                    ]))
                })
                .collect(),
        };
        f.render_stateful_widget(
            List::new(items).block(block).highlight_style(t.selected()).highlight_symbol(format!("{} ", ctx.glyphs.cursor)),
            left,
            &mut self.list,
        );

        // Right: tracks of the highlighted playlist.
        let cur = self.current(ctx);
        let title = cur.as_ref().map_or(" Tracks ".to_string(), |p| format!(" {} ", p.name));
        let block = t.frame(title, self.focus == Focus::Tracks);
        let v = ctx.player.view();
        match cur.as_ref().map(|p| ctx.lib.tracks(&p.id)) {
            Some(Some(Ok(tracks))) => {
                let rows: Vec<Row> = tracks
                    .iter()
                    .enumerate()
                    .map(|(i, tr)| {
                        let playing = ctx.player.is_active() && v.track_id.as_deref() == Some(&tr.id);
                        let num = if playing { format!("{}{}", ctx.glyphs.playing, i + 1) } else { format!("{}", i + 1) };
                        let row = Row::new(vec![
                            num,
                            tr.title.clone(),
                            tr.artist.clone(),
                            tr.year.to_string(),
                            mmss(tr.duration),
                        ]);
                        if playing { row.style(t.accent()) } else { row }
                    })
                    .collect();
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(4),
                        Constraint::Fill(3),
                        Constraint::Fill(2),
                        Constraint::Length(4),
                        Constraint::Length(5),
                    ],
                )
                .header(Row::new(vec!["#", "Title", "Artist", "Year", "Len"]).style(t.dim()))
                .block(block)
                .row_highlight_style(if self.focus == Focus::Tracks { t.selected() } else { t.base() });
                f.render_stateful_widget(table, right, &mut self.tracks);
            }
            Some(Some(Err(e))) => f.render_widget(Paragraph::new(Span::styled(e, t.warn())).block(block), right),
            Some(None) => f.render_widget(Paragraph::new(Span::styled("Loading…", t.dim())).block(block), right),
            None => f.render_widget(block, right),
        }

        let hint = if let Some(e) = &self.error {
            Line::from(Span::styled(format!(" {e}"), t.warn()))
        } else {
            hint_line(
                ctx,
                &[("↑↓", "move"), ("/", "filter"), ("Tab", "preview pane"), ("space", "play 15s"), ("Enter", "use this playlist"), ("r", "reload"), ("Esc", "back")],
            )
        };
        f.render_widget(Paragraph::new(hint).style(t.base()), foot);

        if let (Some(d), Some(p)) = (&self.dialog, cur.as_ref()) {
            self.draw_dialog(f, ctx, d, p);
        }
        if self.help {
            crate::app::draw_keys(
                f,
                ctx,
                &[(
                    "Pick a playlist",
                    &[
                        ("↑↓ j k", "move"),
                        ("/", "filter (letters in order)"),
                        ("Tab", "switch to the track preview"),
                        ("space", "play 15 s from 30% in"),
                        ("Enter", "use this playlist"),
                        ("r", "reload from Music.app"),
                        ("Esc", "back to home"),
                        ("q", "quit"),
                    ],
                )],
            );
        }
    }

    fn draw_dialog(&self, f: &mut Frame, ctx: &Ctx, d: &NameDialog, p: &Playlist) {
        let t = ctx.theme;
        let area = centered(f.area(), 64, 9);
        let file = model::file_name_for(&d.title);
        let exists = PathBuf::from(&file).exists();
        let mut lines = vec![
            Line::from(vec![Span::styled("Title  ", t.dim()), Span::raw(d.title.clone()), Span::styled("▏", t.accent())]),
            Line::from(vec![Span::styled("File   ", t.dim()), Span::raw(file)]),
            Line::raw(""),
        ];
        if exists {
            lines.push(Line::from(Span::styled("That file exists already: Enter opens it.", t.warn())));
        }
        if p.smart {
            lines.push(Line::from(Span::styled("Smart playlist: its songs can change on their own.", t.warn())));
        }
        lines.push(Line::from(vec![
            Span::styled("Enter", t.key()),
            Span::raw(" create   "),
            Span::styled("Esc", t.key()),
            Span::raw(" cancel"),
        ]));
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(t.frame(format!(" New quiz from {} ", p.name), true)), area);
    }
}

fn step(s: &mut impl Selectable, d: i32, n: usize) {
    if n == 0 {
        return;
    }
    let cur = s.sel().unwrap_or(0) as i32;
    s.set(Some((cur + d).clamp(0, n as i32 - 1) as usize));
}

trait Selectable {
    fn sel(&self) -> Option<usize>;
    fn set(&mut self, i: Option<usize>);
}
impl Selectable for ListState {
    fn sel(&self) -> Option<usize> {
        self.selected()
    }
    fn set(&mut self, i: Option<usize>) {
        self.select(i)
    }
}
impl Selectable for TableState {
    fn sel(&self) -> Option<usize> {
        self.selected()
    }
    fn set(&mut self, i: Option<usize>) {
        self.select(i)
    }
}

/// Every filter character appears in order ("mid24" matches "Midsommarquiz 2024").
fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut h = hay.chars();
    needle.chars().filter(|c| !c.is_whitespace()).all(|c| h.any(|x| x == c))
}

fn hours(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m >= 60 { format!("{}:{:02}h", m / 60, m % 60) } else { format!("{m}m") }
}

pub fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_matches_in_order() {
        assert!(super::fuzzy("quizzes / midsommarquiz 2024", "mid24"));
        assert!(!super::fuzzy("midsommarquiz 2024", "42mid"));
    }
}
