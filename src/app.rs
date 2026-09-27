//! Screen switching, the event loop and helpers shared by the screens.

use crate::curation::Curation;
use crate::model::Quiz;
use crate::picker::Picker;
use crate::theme::{DARK, Theme};
use crate::worker::{Connection, Library, Player};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::path::PathBuf;
use std::time::Duration;

pub struct Ctx {
    pub player: Player,
    pub lib: Library,
    pub theme: &'static Theme,
    pub tick: u64,
}

pub enum Transition {
    None,
    Open(Box<Quiz>, PathBuf),
    Quit,
}

enum Screen {
    Picker(Picker),
    Curation(Box<Curation>),
}

pub fn run(open: Option<PathBuf>) -> Result<()> {
    let mut ctx = Ctx { player: Player::spawn(), lib: Library::spawn(), theme: &DARK, tick: 0 };
    let mut screen = match open {
        Some(path) => Screen::Curation(Box::new(Curation::new(Quiz::load(&path)?, path, &ctx))),
        None => Screen::Picker(Picker::new(&ctx)),
    };
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            ctx.tick += 1;
            match &mut screen {
                Screen::Picker(p) => p.tick(&ctx),
                Screen::Curation(c) => c.tick(&ctx),
            }
            terminal.draw(|f| {
                f.render_widget(Paragraph::new("").style(ctx.theme.base()), f.area());
                match &mut screen {
                    Screen::Picker(p) => p.draw(f, &ctx),
                    Screen::Curation(c) => c.draw(f, &ctx),
                }
            })?;
            if !event::poll(Duration::from_millis(80))? {
                continue;
            }
            let Event::Key(k) = event::read()? else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }
            let t = match &mut screen {
                Screen::Picker(p) => p.key(k, &ctx),
                Screen::Curation(c) => c.key(k, &ctx),
            };
            match t {
                Transition::None => {}
                Transition::Quit => break,
                Transition::Open(q, path) => screen = Screen::Curation(Box::new(Curation::new(*q, path, &ctx))),
            }
        }
        Ok(())
    })();
    if let Screen::Curation(c) = &mut screen {
        c.flush();
    }
    ctx.player.stop();
    ratatui::restore();
    // Give the player thread a moment to pause Music.app before we exit.
    std::thread::sleep(Duration::from_millis(300));
    result
}

pub fn music_badge(ctx: &Ctx) -> Span<'static> {
    let t = ctx.theme;
    match ctx.player.view().connection {
        Connection::Connected => Span::styled("● Music.app", t.accent()),
        Connection::Unknown => Span::styled("○ Music.app", t.dim()),
        Connection::NotRunning => Span::styled("○ Music.app not running", t.dim()),
        Connection::Error(e) => Span::styled(format!("✗ {e}"), t.warn()),
    }
}

/// ` mq › crumbs …                         right-hand badges`
pub fn header_line<'a>(ctx: &Ctx, crumbs: Vec<Span<'a>>, right: Vec<Span<'a>>, width: u16) -> Paragraph<'a> {
    let t = ctx.theme;
    let mut left = vec![Span::styled(" mq", t.key()), Span::styled(" › ", t.dim())];
    left.extend(crumbs);
    let used: usize = left.iter().chain(&right).map(|s| s.width()).sum::<usize>() + 2 * right.len();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat((width as usize).saturating_sub(used + 1))));
    for s in right {
        spans.push(s);
        spans.push(Span::raw("  "));
    }
    Paragraph::new(Line::from(spans)).style(t.base())
}

pub fn hint_line(ctx: &Ctx, keys: &[(&str, &str)]) -> Line<'static> {
    let t = ctx.theme;
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in keys {
        spans.push(Span::styled(k.to_string(), t.key()));
        spans.push(Span::styled(format!(" {what}  "), t.dim()));
    }
    Line::from(spans)
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;
    use crate::curation::Curation;
    use ratatui::{Terminal, backend::TestBackend};

    fn dump(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wait(what: impl Fn() -> bool) {
        for _ in 0..100 {
            if what() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("timed out");
    }

    /// Drives both screens against the real Music.app library and prints them.
    /// Run with: cargo test screens -- --ignored --nocapture
    #[test]
    #[ignore]
    fn screens_against_music_app() {
        let mut ctx = Ctx { player: Player::spawn(), lib: Library::spawn(), theme: &DARK, tick: 3 };
        let mut term = Terminal::new(TestBackend::new(120, 32)).unwrap();
        let key = |c: KeyCode| KeyEvent::from(c);

        let mut picker = Picker::new(&ctx);
        wait(|| ctx.lib.playlists().is_some());
        picker.key(key(KeyCode::Char('/')), &ctx);
        for c in "midsom".chars() {
            picker.key(key(KeyCode::Char(c)), &ctx);
        }
        picker.key(key(KeyCode::Enter), &ctx);
        std::thread::sleep(Duration::from_millis(200));
        picker.tick(&ctx);
        let id = ctx.lib.playlists().unwrap().unwrap().into_iter().find(|p| p.name == "Midsommarquiz 2024").unwrap().id;
        wait(|| ctx.lib.tracks(&id).is_some());
        term.draw(|f| picker.draw(f, &ctx)).unwrap();
        println!("{}\n", dump(&term));

        picker.key(key(KeyCode::Enter), &ctx);
        term.draw(|f| picker.draw(f, &ctx)).unwrap();
        println!("{}\n", dump(&term));
        for _ in 0.."Midsommarquiz 2024".len() {
            picker.key(key(KeyCode::Backspace), &ctx);
        }
        for c in "mq test quiz".chars() {
            picker.key(key(KeyCode::Char(c)), &ctx);
        }
        let _ = std::fs::remove_file("mq-test-quiz.quiz.toml");
        let Transition::Open(q, path) = picker.key(key(KeyCode::Enter), &ctx) else { panic!("no quiz") };
        let mut cur = Curation::new(*q, path.clone(), &ctx);
        wait(|| ctx.lib.tracks(&id).is_some());
        cur.tick(&ctx);
        for c in ['j', 'j', 'J', 'n'] {
            cur.key(key(KeyCode::Char(c)), &ctx);
        }
        for c in "Filmmusik".chars() {
            cur.key(key(KeyCode::Char(c)), &ctx);
        }
        cur.key(key(KeyCode::Enter), &ctx);
        for c in ['A', 'j', ']', ']', '}', '+', 's'] {
            cur.key(key(KeyCode::Char(c)), &ctx);
        }
        cur.key(key(KeyCode::Char('k')), &ctx);
        cur.key(key(KeyCode::Char(' ')), &ctx);
        wait(|| ctx.player.view().phase == crate::worker::Phase::Playing);
        std::thread::sleep(Duration::from_millis(1500));
        ctx.tick = 7;
        term.draw(|f| cur.draw(f, &ctx)).unwrap();
        println!("{}\n", dump(&term));
        ctx.player.stop();
        cur.flush();
        println!("{}", std::fs::read_to_string(&path).unwrap().lines().take(40).collect::<Vec<_>>().join("\n"));
        std::fs::remove_file(&path).unwrap();
    }

    fn fake_quiz() -> Quiz {
        let track = |id: &str, title: &str| crate::music::Track {
            id: id.into(),
            title: title.into(),
            artist: "Harold Faltermeyer".into(),
            album: "Beverly Hills Cop".into(),
            year: 1984,
            duration: 180.0,
        };
        let pl = crate::music::Playlist { id: "NOPE".into(), name: "Fake".into(), folder: String::new(), smart: false, duration: 0.0, count: 2 };
        Quiz::new("Fake", &pl, &[track("1", "Axel F"), track("2", "The Heat Is On")])
    }

    #[test]
    fn question_editor_adds_edits_and_reorders() {
        let ctx = Ctx { player: Player::spawn(), lib: Library::spawn(), theme: &DARK, tick: 0 };
        let dir = std::env::temp_dir().join(format!("mq-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.quiz.toml");
        let mut cur = Curation::new(fake_quiz(), path.clone(), &ctx);
        let key = |c: KeyCode| KeyEvent::from(c);
        let press = |cur: &mut Curation, keys: &[KeyEvent]| {
            for k in keys {
                cur.key(*k, &ctx);
            }
        };
        let chars = |s: &str| s.chars().map(|c| key(KeyCode::Char(c))).collect::<Vec<_>>();
        // Artist question, accepted as prefilled.
        press(&mut cur, &[key(KeyCode::Char('a')), key(KeyCode::Char('1')), key(KeyCode::Enter)]);
        // Custom question with a hint line, answer and 1.5 points.
        press(&mut cur, &[key(KeyCode::Char('a')), key(KeyCode::Char('5'))]);
        press(&mut cur, &chars("Vilken film?"));
        press(&mut cur, &[key(KeyCode::Enter)]);
        press(&mut cur, &chars("(hint: Eddie Murphy)"));
        press(&mut cur, &[key(KeyCode::Tab)]);
        press(&mut cur, &chars("Snuten i Hollywood"));
        press(&mut cur, &[key(KeyCode::Tab), key(KeyCode::Right)]);
        press(&mut cur, &[KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)]);
        // Move the custom question first, then add a year question and cancel it.
        press(&mut cur, &[KeyEvent::new(KeyCode::Up, KeyModifiers::ALT), key(KeyCode::Esc)]);
        press(&mut cur, &[key(KeyCode::Char('a')), key(KeyCode::Char('3')), key(KeyCode::Esc)]);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        press(&mut cur, &[key(KeyCode::Tab)]);
        term.draw(|f| cur.draw(f, &ctx)).unwrap();
        println!("{}", dump(&term));
        cur.flush();
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{text}");
        let back = Quiz::load(&path).unwrap();
        let qs = &back.songs().next().unwrap().questions;
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].prompt, "Vilken film?\n(hint: Eddie Murphy)");
        assert_eq!(qs[0].answer, "Snuten i Hollywood");
        assert_eq!(qs[0].points, 1.5);
        assert_eq!(qs[1].answer, "Harold Faltermeyer");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Removes a song from a scratch playlist in Music.app, then deletes the scratch playlist.
    /// Run with: cargo test removes -- --ignored --nocapture
    #[test]
    #[ignore]
    fn removes_song_from_scratch_playlist() {
        let script = |js: &str| {
            let out = std::process::Command::new("osascript").args(["-l", "JavaScript", "-e", js]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let pl = script(r#"const M = Application("Music");
            const src = M.userPlaylists.whose({name: "Midsommarquiz 2024"})[0];
            const tmp = M.make({new: "userPlaylist", withProperties: {name: "mq scratch test"}});
            M.duplicate(src.tracks[0], {to: tmp}); M.duplicate(src.tracks[1], {to: tmp});
            tmp.persistentID()"#);
        let ctx = Ctx { player: Player::spawn(), lib: Library::spawn(), theme: &DARK, tick: 0 };
        ctx.lib.load_playlists();
        wait(|| ctx.lib.playlists().is_some());
        let playlist = ctx.lib.playlists().unwrap().unwrap().into_iter().find(|p| p.id == pl).unwrap();
        let tracks = crate::music::tracks(&pl).unwrap();
        let path = std::env::temp_dir().join(format!("mq-rm-{}.quiz.toml", std::process::id()));
        let mut cur = Curation::new(Quiz::new("Scratch", &playlist, &tracks), path.clone(), &ctx);
        wait(|| ctx.lib.tracks(&pl).is_some());
        cur.tick(&ctx);
        cur.key(KeyEvent::from(KeyCode::Char('X')), &ctx);
        cur.key(KeyEvent::from(KeyCode::Char('y')), &ctx);
        let left = crate::music::tracks(&pl).unwrap();
        script(&format!(r#"Application("Music").userPlaylists.whose({{persistentID: "{pl}"}})[0].delete()"#));
        cur.flush();
        let back = Quiz::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, tracks[1].id);
        assert_eq!(back.songs().count(), 1);
        assert_eq!(back.removed.len(), 1);
    }
}
