//! Run view: the projected screen on quiz night.
//!
//! Setup/check → Listen (each song with its questions) → quick re-run (a short
//! snippet of each song) → reveal (each song again, answers shown) → done.
//! The audience sees exactly this screen, so nothing about a song (title,
//! artist, what comes next) is shown before the reveal, and errors are neutral.
//! Audience-facing text is Swedish, like the answer sheet.

use crate::app::{Ctx, Transition, centered};
use crate::check::{self, Item, Level};
use crate::model::{Entry, Quiz, Song};
use crate::questions::fmt_points;
use crate::theme::mmss;
use crate::worker::Phase;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tui_big_text::{BigText, PixelSize};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cycle {
    Listen,
    Rerun,
    Reveal,
}

impl Cycle {
    fn name(self) -> &'static str {
        match self {
            Cycle::Listen => "Lyssna",
            Cycle::Rerun => "Snabb repris",
            Cycle::Reveal => "Facit",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Slide {
    Title,
    Cycle(Cycle),
    Round { cycle: Cycle, number: usize, title: String },
    Song { cycle: Cycle, entry: usize, number: usize, round: Option<String> },
    Done,
}

fn slides(quiz: &Quiz) -> Vec<Slide> {
    let numbers = quiz.numbers();
    let mut out = vec![Slide::Title];
    for cycle in [Cycle::Listen, Cycle::Rerun, Cycle::Reveal] {
        out.push(Slide::Cycle(cycle));
        let (mut round_no, mut round) = (0, None);
        for (i, e) in quiz.entries.iter().enumerate() {
            match (e, numbers[i]) {
                (Entry::Heading(t), _) => {
                    round_no += 1;
                    round = Some(t.clone());
                    // A heading only gets a card if a song follows it.
                    let has_song = quiz.entries[i + 1..]
                        .iter()
                        .take_while(|e| matches!(e, Entry::Song(_)))
                        .any(|e| matches!(e, Entry::Song(s) if !s.skip));
                    if has_song {
                        out.push(Slide::Round { cycle, number: round_no, title: t.clone() });
                    }
                }
                (Entry::Song(_), Some(n)) => out.push(Slide::Song { cycle, entry: i, number: n, round: round.clone() }),
                _ => {}
            }
        }
    }
    out.push(Slide::Done);
    out
}

/// Where the run is, saved next to the quiz so a crash or quit can resume.
#[derive(Serialize, Deserialize)]
struct Saved {
    slide: usize,
    slides: usize,
}

fn state_path(quiz: &Path) -> PathBuf {
    let name = quiz.file_name().map(|n| n.to_string_lossy().replace(".quiz.toml", "")).unwrap_or_default();
    quiz.with_file_name(format!(".{name}.run.toml"))
}

/// Muted play-check progress: (done, total, results when finished).
type PlayCheck = Arc<Mutex<(usize, usize, Option<Vec<Item>>)>>;

struct Setup {
    items: Vec<Item>,
    loaded: bool,
    resume: Option<usize>,
    play_check: Option<PlayCheck>,
}

pub struct Run {
    quiz: Quiz,
    path: PathBuf,
    slides: Vec<Slide>,
    idx: usize,
    setup: Option<Setup>,
    hints: bool,
    blank: bool,
    jump: Option<String>,
    /// Start the current slide's clip at this moment (short pause after arriving).
    play_at: Option<Instant>,
    /// Re-run: go to the next slide at this moment.
    next_at: Option<Instant>,
    seen_finished: u64,
    /// The clip on the current slide has been started at least once.
    played: bool,
}

impl Run {
    pub fn new(quiz: Quiz, path: PathBuf, ctx: &Ctx) -> Self {
        ctx.lib.load_tracks(&quiz.meta.source.playlist_id, true);
        let slides = slides(&quiz);
        let resume = std::fs::read_to_string(state_path(&path))
            .ok()
            .and_then(|t| toml::from_str::<Saved>(&t).ok())
            .filter(|s| s.slides == slides.len() && s.slide > 0 && s.slide < slides.len() - 1)
            .map(|s| s.slide);
        Run {
            quiz,
            path,
            slides,
            idx: 0,
            setup: Some(Setup { items: Vec::new(), loaded: false, resume, play_check: None }),
            hints: true,
            blank: false,
            jump: None,
            play_at: None,
            next_at: None,
            seen_finished: ctx.player.view().finished,
            played: false,
        }
    }

    fn save_position(&self) {
        let p = state_path(&self.path);
        if matches!(self.slides[self.idx], Slide::Done) {
            let _ = std::fs::remove_file(p);
        } else if let Ok(t) = toml::to_string(&Saved { slide: self.idx, slides: self.slides.len() }) {
            let _ = std::fs::write(p, t);
        }
    }

    fn song(&self, entry: usize) -> &Song {
        match &self.quiz.entries[entry] {
            Entry::Song(s) => s,
            Entry::Heading(_) => unreachable!("slides only point at songs"),
        }
    }

    fn go(&mut self, idx: usize, ctx: &Ctx) {
        ctx.player.stop();
        self.idx = idx.min(self.slides.len() - 1);
        self.played = false;
        self.next_at = None;
        self.play_at = match &self.slides[self.idx] {
            Slide::Song { .. } => Some(Instant::now() + Duration::from_millis(800)),
            _ => None,
        };
        // In the re-run, round cards move on by themselves.
        if let Slide::Round { cycle: Cycle::Rerun, .. } = self.slides[self.idx] {
            self.next_at = Some(Instant::now() + Duration::from_millis(2500));
        }
        self.seen_finished = ctx.player.view().finished;
        self.save_position();
    }

    fn play_current(&mut self, ctx: &Ctx) {
        let Slide::Song { cycle, entry, .. } = self.slides[self.idx] else { return };
        let s = self.song(entry).clone();
        let d = &self.quiz.meta.defaults;
        let length = match cycle {
            Cycle::Rerun => d.rerun_length,
            _ => s.clip_length.unwrap_or(d.clip_length),
        };
        ctx.player.play_faded(
            &self.quiz.meta.source.playlist_id,
            &s.track_id,
            s.clip_start.unwrap_or(0) as f64,
            Some(length as f64),
            d.fade_out,
        );
        self.played = true;
        self.play_at = None;
        self.next_at = None;
    }

    pub fn tick(&mut self, ctx: &Ctx) {
        if let Some(setup) = &mut self.setup {
            if !setup.loaded
                && let Some(r) = ctx.lib.tracks(&self.quiz.meta.source.playlist_id)
            {
                setup.loaded = true;
                setup.items = check::check(&self.quiz, &r, false);
            }
            return;
        }
        if self.play_at.is_some_and(|t| Instant::now() >= t) {
            self.play_current(ctx);
        }
        let v = ctx.player.view();
        if v.finished != self.seen_finished {
            self.seen_finished = v.finished;
            if let Slide::Song { cycle: Cycle::Rerun, .. } = self.slides[self.idx] {
                self.next_at = Some(Instant::now() + Duration::from_millis(1500));
            }
        }
        if self.next_at.is_some_and(|t| Instant::now() >= t) {
            self.go(self.idx + 1, ctx);
        }
    }

    pub fn key(&mut self, k: KeyEvent, ctx: &Ctx) -> Transition {
        if self.setup.is_some() {
            return self.setup_key(k, ctx);
        }
        if let Some(j) = &mut self.jump {
            match k.code {
                KeyCode::Esc => self.jump = None,
                KeyCode::Backspace => drop(j.pop()),
                KeyCode::Char(c) if c.is_ascii_digit() => j.push(c),
                KeyCode::Enter => {
                    let n: Option<usize> = j.parse().ok();
                    self.jump = None;
                    let cycle = self.cycle();
                    if let Some(i) = self.slides.iter().position(|s| matches!(s, Slide::Song { cycle: c, number, .. } if Some(*number) == n && Some(*c) == cycle)) {
                        self.go(i, ctx);
                    }
                }
                _ => {}
            }
            return Transition::None;
        }
        let v = ctx.player.view();
        match k.code {
            KeyCode::Char('q') => {
                ctx.player.stop();
                return Transition::Home;
            }
            KeyCode::Right | KeyCode::Char('n') | KeyCode::Enter if self.idx + 1 < self.slides.len() => self.go(self.idx + 1, ctx),
            KeyCode::Left | KeyCode::Char('N') if self.idx > 0 => self.go(self.idx - 1, ctx),
            KeyCode::Char(' ') => match v.phase {
                Phase::Playing | Phase::Starting => {
                    ctx.player.pause();
                    self.next_at = None;
                }
                Phase::Paused => ctx.player.resume(),
                _ => self.play_current(ctx),
            },
            KeyCode::Char('r') => self.play_current(ctx),
            KeyCode::Esc => {
                ctx.player.stop();
                self.play_at = None;
                self.next_at = None;
            }
            KeyCode::Char('g') => self.jump = Some(String::new()),
            KeyCode::Char('R') => {
                if let Some(i) = self.slides.iter().position(|s| *s == Slide::Cycle(Cycle::Reveal)) {
                    self.go(i, ctx);
                }
            }
            KeyCode::Char('b') => self.blank = !self.blank,
            KeyCode::Char('h') => self.hints = !self.hints,
            _ => {}
        }
        Transition::None
    }

    fn setup_key(&mut self, k: KeyEvent, ctx: &Ctx) -> Transition {
        let setup = self.setup.as_mut().unwrap();
        let checking = setup.play_check.as_ref().is_some_and(|p| p.lock().unwrap().2.is_none());
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc if !checking => return Transition::Home,
            KeyCode::Char('c') if !checking => {
                let progress = Arc::new(Mutex::new((0, 0, None)));
                let p = progress.clone();
                let quiz = self.quiz.clone();
                std::thread::spawn(move || {
                    let items = check::play_check(&quiz, |done, total| {
                        let mut g = p.lock().unwrap();
                        g.0 = done;
                        g.1 = total;
                    });
                    p.lock().unwrap().2 = Some(items);
                });
                setup.play_check = Some(progress);
            }
            KeyCode::Enter if !checking => {
                let start = setup.resume.unwrap_or(1);
                self.setup = None;
                self.go(start, ctx);
            }
            KeyCode::Char('s') if !checking && setup.resume.is_some() => {
                self.setup = None;
                self.go(1, ctx);
            }
            _ => {}
        }
        Transition::None
    }

    fn cycle(&self) -> Option<Cycle> {
        match &self.slides[self.idx] {
            Slide::Cycle(c) | Slide::Round { cycle: c, .. } | Slide::Song { cycle: c, .. } => Some(*c),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- drawing

    pub fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        f.render_widget(Paragraph::new("").style(Style::new().bg(t.bg)), f.area());
        if self.setup.is_some() {
            return self.draw_setup(f, ctx);
        }
        if self.blank {
            return;
        }
        let [top, body, bar, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(f.area().inner(ratatui::layout::Margin::new(2, 1)));

        // Top: which part of the evening, and the round. Never the song.
        let left = match (&self.slides[self.idx], self.cycle()) {
            (Slide::Song { round: Some(r), .. }, Some(c)) => format!("{}  ·  {r}", c.name()),
            (_, Some(c)) => c.name().to_string(),
            _ => String::new(),
        };
        f.render_widget(Paragraph::new(Span::styled(left, t.dim())), top);
        f.render_widget(Paragraph::new(Span::styled(self.quiz.meta.title.clone(), t.faint())).alignment(Alignment::Right), top);

        let mut blocks = Vec::new();
        let w = body.width;
        match self.slides[self.idx].clone() {
            Slide::Title => {
                blocks.push(big(&self.quiz.meta.title, t.accent(), w, true));
                if let Some(s) = &self.quiz.meta.subtitle {
                    blocks.push(Blk::Gap(1));
                    blocks.push(big(s, t.clip(), w, false));
                }
                if let Some(i) = &self.quiz.meta.instructions {
                    blocks.push(Blk::Gap(2));
                    blocks.push(text(Line::styled(i.clone(), t.base().add_modifier(Modifier::ITALIC))));
                }
            }
            Slide::Cycle(c) => {
                let label = match c {
                    Cycle::Listen => "Del 1",
                    Cycle::Rerun => "Del 2",
                    Cycle::Reveal => "Del 3",
                };
                blocks.push(big(label, t.dim(), w, false));
                blocks.push(Blk::Gap(1));
                blocks.push(big(c.name(), t.accent(), w, true));
            }
            Slide::Round { number, title, .. } => {
                blocks.push(big(&format!("Runda {number}"), t.dim(), w, false));
                blocks.push(Blk::Gap(1));
                blocks.push(big(&title, t.accent(), w, true));
            }
            Slide::Song { cycle, entry, number, .. } => {
                let s = self.song(entry).clone();
                blocks.push(big(&number.to_string(), t.accent(), w, true));
                if cycle != Cycle::Rerun {
                    blocks.push(Blk::Gap(1));
                    let many = s.questions.len() > 1;
                    for (i, q) in s.questions.iter().enumerate() {
                        let label = if many { format!("{}  ", (b'a' + i as u8) as char) } else { String::new() };
                        let mut prompt = q.prompt.lines();
                        let first = prompt.next().unwrap_or("");
                        blocks.push(big(&format!("{label}{first}"), t.base(), w, false));
                        for hint in prompt {
                            blocks.push(text(Line::styled(hint.to_string(), t.dim())));
                        }
                        if cycle == Cycle::Reveal {
                            let pts = format!("  ({} p)", fmt_points(q.points));
                            blocks.push(big(&q.answer, t.clip(), w, false));
                            blocks.push(text(Line::styled(pts, t.dim())));
                        }
                        blocks.push(Blk::Gap(1));
                    }
                    if cycle == Cycle::Reveal {
                        blocks.push(text(Line::styled(format!("{} – {}", s.title, s.artist), t.dim())));
                    }
                }
            }
            Slide::Done => {
                blocks.push(big("Tack!", t.accent(), w, true));
                blocks.push(Blk::Gap(1));
                blocks.push(text(Line::styled("Lämna in svaren", t.dim())));
            }
        }
        render_blocks(f, body, blocks);

        // Clip progress, or a neutral note if playback failed.
        let v = ctx.player.view();
        if let Phase::Error(_) = v.phase {
            f.render_widget(Paragraph::new(Span::styled("Uppspelningsproblem – tryck r för att försöka igen", t.warn())).alignment(Alignment::Center), bar);
        } else if let (Slide::Song { .. }, Some((_, len))) = (&self.slides[self.idx], v.clip) {
            let done = (v.clip_elapsed / len).clamp(0.0, 1.0);
            let label = format!(" {} / {}", mmss(v.clip_elapsed), mmss(len));
            let n = (bar.width as usize).saturating_sub(label.len());
            let filled = (done * n as f64).round() as usize;
            let style = if v.phase == Phase::Paused { t.dim() } else { t.clip() };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("━".repeat(filled), style),
                    Span::styled("─".repeat(n - filled), t.faint()),
                    Span::styled(label, t.dim()),
                ])),
                bar,
            );
        }
        if self.hints {
            let keys = "space play/pause   → ← next/prev   r replay   g jump   R reveal   b blank   h hide keys   q end";
            f.render_widget(Paragraph::new(Span::styled(keys, t.faint())).alignment(Alignment::Center), hint);
        }
        if let Some(j) = &self.jump {
            let area = centered(f.area(), 30, 3);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(j.clone()), Span::styled("▏", t.accent())])).block(t.frame(" Go to song ", true)),
                area,
            );
        }
    }

    fn draw_setup(&self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let setup = self.setup.as_ref().unwrap();
        let v = ctx.player.view();
        let mut lines = vec![Line::raw("")];
        let mark = |l: Level| match l {
            Level::Ok => Span::styled(format!("  {} ", ctx.glyphs.check), t.clip()),
            Level::Warn => Span::styled("  ! ", t.warn()),
            Level::Fail => Span::styled(format!("  {} ", ctx.glyphs.error), t.warn()),
        };
        lines.push(Line::from(match v.connection {
            crate::worker::Connection::Error(e) => vec![mark(Level::Fail), Span::raw(e)],
            crate::worker::Connection::NotRunning => vec![mark(Level::Warn), Span::raw("Music.app is not running; it starts with the first song")],
            _ => vec![mark(Level::Ok), Span::raw("Music.app can be controlled")],
        }));
        if !setup.loaded {
            lines.push(Line::styled("    reading the playlist…", t.dim()));
        }
        for i in &setup.items {
            lines.push(Line::from(vec![mark(i.level), Span::raw(i.text.clone())]));
        }
        if let Some(p) = &setup.play_check {
            let g = p.lock().unwrap();
            match &g.2 {
                None => lines.push(Line::styled(format!("    playing every song muted: {}/{}", g.0, g.1), t.dim())),
                Some(items) => {
                    for i in items {
                        lines.push(Line::from(vec![mark(i.level), Span::raw(i.text.clone())]));
                    }
                }
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("  Keep Music.app off the projected screen: it shows the song title.", t.warn()));
        lines.push(Line::raw(""));
        let resume = setup.resume.map(|i| describe(&self.slides[i]));
        let mut keys = vec![Span::raw("  ")];
        match &resume {
            Some(r) => keys.extend([
                Span::styled("Enter", t.key()),
                Span::raw(format!(" resume at {r}   ")),
                Span::styled("s", t.key()),
                Span::raw(" start over   "),
            ]),
            None => keys.extend([Span::styled("Enter", t.key()), Span::raw(" start   ")]),
        }
        keys.extend([Span::styled("c", t.key()), Span::raw(" check every song plays (muted)   "), Span::styled("q", t.key()), Span::raw(" back")]);
        lines.push(Line::from(keys));
        let area = centered(f.area(), 100, lines.len() as u16 + 2);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(t.frame(format!(" Before we start · {} ", self.quiz.meta.title), true)), area);
    }
}

fn describe(s: &Slide) -> String {
    match s {
        Slide::Title => "the title".into(),
        Slide::Cycle(c) => format!("\"{}\"", c.name()),
        Slide::Round { cycle, number, .. } => format!("round {number} ({})", cycle.name()),
        Slide::Song { cycle, number, .. } => format!("song {number} ({})", cycle.name()),
        Slide::Done => "the end".into(),
    }
}

enum Blk {
    Big(String, Style, PixelSize, u16),
    Text(Line<'static>),
    Gap(u16),
}

/// Large text that fits the width: 8x8 cells per letter when `huge` and it
/// fits, else 4x4, else plain bold text.
fn big(s: &str, style: Style, width: u16, huge: bool) -> Blk {
    let n = s.chars().count() as u16;
    if huge && n * 8 <= width {
        Blk::Big(s.into(), style, PixelSize::Full, 8)
    } else if n * 4 <= width {
        Blk::Big(s.into(), style, PixelSize::Quadrant, 4)
    } else {
        Blk::Text(Line::styled(s.to_string(), style.add_modifier(Modifier::BOLD)))
    }
}

fn text(l: Line<'static>) -> Blk {
    Blk::Text(l)
}

/// Stack blocks vertically, centered; drops what doesn't fit.
fn render_blocks(f: &mut Frame, area: Rect, blocks: Vec<Blk>) {
    let h = |b: &Blk| match b {
        Blk::Big(_, _, _, h) | Blk::Gap(h) => *h,
        Blk::Text(_) => 1,
    };
    let total: u16 = blocks.iter().map(h).sum();
    let mut y = area.y + area.height.saturating_sub(total) / 2;
    for b in blocks {
        let bh = h(&b);
        if y + bh > area.bottom() {
            break;
        }
        let r = Rect { x: area.x, y, width: area.width, height: bh };
        match b {
            Blk::Big(s, style, px, _) => {
                f.render_widget(BigText::builder().pixel_size(px).style(style).centered().lines(vec![s.into()]).build(), r)
            }
            Blk::Text(l) => f.render_widget(Paragraph::new(l).alignment(Alignment::Center), r),
            Blk::Gap(_) => {}
        }
        y += bh;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::{Playlist, Track};

    #[test]
    fn slides_follow_the_three_cycles_with_round_cards() {
        let track = |id: &str| Track { id: id.into(), title: id.into(), artist: "A".into(), album: "B".into(), year: 1984, duration: 180.0 };
        let pl = Playlist { id: "P".into(), name: "Q".into(), folder: String::new(), smart: false, duration: 0.0, count: 0 };
        let mut q = Quiz::new("Q", &pl, &[track("1"), track("2"), track("3")]);
        q.entries.insert(0, Entry::Heading("Film".into()));
        if let Entry::Song(s) = &mut q.entries[2] {
            s.skip = true;
        }
        let s = slides(&q);
        let songs = |c: Cycle| s.iter().filter(|x| matches!(x, Slide::Song { cycle, .. } if *cycle == c)).count();
        assert_eq!(s.first(), Some(&Slide::Title));
        assert_eq!(s.last(), Some(&Slide::Done));
        assert_eq!((songs(Cycle::Listen), songs(Cycle::Rerun), songs(Cycle::Reveal)), (2, 2, 2));
        assert_eq!(s.iter().filter(|x| matches!(x, Slide::Round { .. })).count(), 3);
        // Numbers skip the skipped song.
        let nums: Vec<usize> = s.iter().filter_map(|x| if let Slide::Song { cycle: Cycle::Listen, number, .. } = x { Some(*number) } else { None }).collect();
        assert_eq!(nums, vec![1, 2]);
    }

    /// Renders setup and a few slides of a copy of the real quiz, without playing.
    /// Run with: cargo test run_view -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_view_renders_real_quiz() {
        use crate::app::Ctx;
        use crate::worker::{Library, Player};
        use ratatui::{Terminal, backend::TestBackend};
        let src = PathBuf::from("midsommarquiz-2024.quiz.toml");
        let path = std::env::temp_dir().join("mq-run-view.quiz.toml");
        std::fs::copy(&src, &path).unwrap();
        let quiz = Quiz::load(&path).unwrap();
        let ctx = Ctx { player: Player::spawn(), lib: Library::spawn(), theme: &crate::theme::DARK, glyphs: &crate::theme::UNICODE, tick: 0 };
        let mut run = Run::new(quiz, path.clone(), &ctx);
        let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
        let dump = |term: &Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        for _ in 0..50 {
            run.tick(&ctx);
            if run.setup.as_ref().unwrap().loaded {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(1600));
        term.draw(|f| run.draw(f, &ctx)).unwrap();
        println!("{}\n", dump(&term));
        run.setup = None;
        let pick = |c: Cycle, n: usize| run.slides.iter().position(|s| matches!(s, Slide::Song { cycle, number, .. } if *cycle == c && *number == n)).unwrap();
        let wanted = [0, 1, pick(Cycle::Listen, 3), pick(Cycle::Rerun, 3), pick(Cycle::Reveal, 3), run.slides.len() - 1];
        for i in wanted {
            run.go(i, &ctx);
            run.play_at = None;
            term.draw(|f| run.draw(f, &ctx)).unwrap();
            println!("{}\n", dump(&term));
        }
        let _ = std::fs::remove_file(state_path(&path));
        std::fs::remove_file(&path).unwrap();
    }
}
