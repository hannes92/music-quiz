//! Curation screen: running order, rounds and clip marking for one quiz.

use crate::app::{Ctx, Transition, centered, header_line, hint_line, music_badge};
use crate::model::{Entry, Quiz, Song, SyncPlan};
use crate::music::Track;
use crate::settings::{self, Settings};
use crate::music;
use crate::picker::truncate;
use crate::questions::{self, Editor, Outcome, fmt_points};
use crate::theme::{equalizer, mmss};
use crate::worker::Phase;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

enum Input {
    NewRound(String),
    Rename(String),
    /// `/`: the query and where the cursor was before searching.
    Find(String, usize),
}

/// Where we are in matching the quiz to its Music.app playlist.
enum Link {
    Loading,
    /// Loaded; `None` when in sync, otherwise changes waiting for review.
    Loaded(Option<(SyncPlan, Vec<Track>)>),
    /// The playlist ID didn't resolve; looking the playlist up by name.
    ByName,
    Missing(String),
}

enum Save {
    Saved,
    Dirty(Instant),
    Failed(String),
}

type Snapshot = (Vec<Entry>, Vec<Song>);

pub struct Curation {
    quiz: Quiz,
    path: PathBuf,
    /// Index into entries, then removed songs.
    sel: usize,
    list: ListState,
    new_ids: HashSet<String>,
    save: Save,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    clipboard: Option<Entry>,
    input: Option<Input>,
    order_menu: bool,
    /// Selected question while the questions pane has focus.
    q_sel: Option<usize>,
    add_picker: Option<usize>,
    editor: Option<Editor>,
    confirm_remove: bool,
    help: bool,
    auto_preview: bool,
    preview_due: Option<Instant>,
    link: Link,
    review: bool,
    /// `R`: choosing another playlist; the index into the playlist list.
    relink: Option<usize>,
    settings: Option<Settings>,
    focus_at: Instant,
    msg: Option<(String, Instant)>,
}

impl Curation {
    pub fn new(quiz: Quiz, path: PathBuf, ctx: &Ctx) -> Self {
        ctx.lib.load_tracks(&quiz.meta.source.playlist_id, true);
        Curation {
            quiz,
            path,
            sel: 0,
            list: ListState::default(),
            new_ids: HashSet::new(),
            save: Save::Saved,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
            input: None,
            order_menu: false,
            q_sel: None,
            add_picker: None,
            editor: None,
            confirm_remove: false,
            help: false,
            auto_preview: false,
            preview_due: None,
            link: Link::Loading,
            review: false,
            relink: None,
            settings: None,
            focus_at: Instant::now(),
            msg: None,
        }
    }

    fn playlist_id(&self) -> String {
        self.quiz.meta.source.playlist_id.clone()
    }

    fn rows(&self) -> usize {
        self.quiz.entries.len() + self.quiz.removed.len()
    }

    fn song(&self) -> Option<&Song> {
        match self.quiz.entries.get(self.sel) {
            Some(Entry::Song(s)) => Some(s),
            Some(Entry::Heading(_)) => None,
            None => self.quiz.removed.get(self.sel - self.quiz.entries.len()),
        }
    }

    fn song_mut(&mut self) -> Option<&mut Song> {
        match self.quiz.entries.get_mut(self.sel) {
            Some(Entry::Song(s)) => Some(s),
            _ => None,
        }
    }

    fn clip_len(&self, s: &Song) -> u32 {
        s.clip_length.unwrap_or(self.quiz.meta.defaults.clip_length)
    }

    fn flash(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), Instant::now()));
    }

    /// Call before every edit: records undo and schedules the autosave.
    fn checkpoint(&mut self) {
        self.undo.push((self.quiz.entries.clone(), self.quiz.removed.clone()));
        self.redo.clear();
        self.save = Save::Dirty(Instant::now());
        if let Some(id) = self.song().map(|s| s.track_id.clone()) {
            self.new_ids.remove(&id);
        }
    }

    pub fn flush(&mut self) {
        if !matches!(self.save, Save::Saved) {
            self.save = match self.quiz.save(&self.path) {
                Ok(()) => Save::Saved,
                Err(e) => Save::Failed(format!("{e:#}")),
            };
        }
    }

    pub fn tick(&mut self, ctx: &Ctx) {
        self.tick_link(ctx);
        if let Save::Dirty(at) = self.save
            && at.elapsed() > Duration::from_secs(1)
        {
            self.flush();
        }
        if let Some(due) = self.preview_due
            && Instant::now() >= due
        {
            self.preview_due = None;
            if self.sel < self.quiz.entries.len() {
                self.play_clip(ctx);
            }
        }
    }

    /// Re-read the playlist on open. Changes wait for review (`S`); a playlist
    /// whose ID no longer resolves is looked up by name, then left to `R`.
    fn tick_link(&mut self, ctx: &Ctx) {
        match &self.link {
            Link::Loading => {
                let Some(r) = ctx.lib.tracks(&self.playlist_id()) else { return };
                self.link = match r {
                    Ok(tracks) => {
                        let plan = self.quiz.plan_sync(&tracks);
                        Link::Loaded((!plan.is_empty()).then_some((plan, tracks)))
                    }
                    Err(_) => {
                        ctx.lib.load_playlists();
                        Link::ByName
                    }
                };
            }
            Link::ByName => {
                let Some(r) = ctx.lib.playlists() else { return };
                let name = self.quiz.meta.source.playlist.clone();
                match r.map(|all| all.into_iter().find(|p| p.name == name)) {
                    Ok(Some(p)) => {
                        self.flash(format!("Playlist found by name: {}", p.path()));
                        self.link_playlist(ctx, &p);
                    }
                    Ok(None) => self.link = Link::Missing(format!("playlist \"{name}\" not found")),
                    Err(e) => self.link = Link::Missing(e),
                }
            }
            Link::Loaded(_) | Link::Missing(_) => {}
        }
    }

    fn link_playlist(&mut self, ctx: &Ctx, p: &crate::music::Playlist) {
        let src = &mut self.quiz.meta.source;
        src.playlist = p.name.clone();
        src.playlist_id = p.id.clone();
        src.folder = p.folder.clone();
        self.save = Save::Dirty(Instant::now());
        ctx.lib.load_tracks(&p.id, true);
        self.link = Link::Loading;
    }

    fn apply_review(&mut self) {
        let Link::Loaded(Some((plan, tracks))) = std::mem::replace(&mut self.link, Link::Loaded(None)) else { return };
        self.checkpoint();
        let added = self.quiz.apply_sync(&plan, &tracks);
        self.new_ids.extend(added);
        self.flash(format!(
            "Playlist synced: {} added, {} moved to \"not in playlist\", {} re-linked",
            plan.added.len(),
            plan.removed.len(),
            plan.relinks.len()
        ));
    }

    fn set_focus(&mut self, q: Option<usize>) {
        if q.is_some() != self.q_sel.is_some() {
            self.focus_at = Instant::now();
        }
        self.q_sel = q;
    }

    fn moved(&mut self) {
        self.set_focus(None);
        if self.auto_preview {
            self.preview_due = Some(Instant::now() + Duration::from_millis(700));
        }
    }

    fn is_playing(&self, ctx: &Ctx, s: &Song) -> bool {
        ctx.player.is_active() && ctx.player.view().track_id.as_deref() == Some(&s.track_id)
    }

    fn play_clip(&mut self, ctx: &Ctx) {
        let Some(s) = self.song().cloned() else { return };
        if self.sel >= self.quiz.entries.len() {
            return self.flash("That song is no longer in the playlist");
        }
        let len = self.clip_len(&s);
        ctx.player.play(&self.playlist_id(), &s.track_id, s.clip_start.unwrap_or(0) as f64, Some(len as f64));
    }

    pub fn key(&mut self, k: KeyEvent, ctx: &Ctx) -> Transition {
        if let Some(Input::Find(query, from)) = &mut self.input {
            let from = *from;
            match k.code {
                KeyCode::Esc => {
                    self.sel = from;
                    self.input = None;
                }
                KeyCode::Enter => self.input = None,
                KeyCode::Backspace => drop(query.pop()),
                KeyCode::Char(c) => query.push(c),
                _ => {}
            }
            if let Some(Input::Find(query, _)) = &self.input {
                let q = query.to_lowercase();
                let n = self.quiz.entries.len();
                let hit = (0..n).map(|i| (from + i) % n).find(|&i| match &self.quiz.entries[i] {
                    Entry::Song(s) => !q.is_empty() && (s.title.to_lowercase().contains(&q) || s.artist.to_lowercase().contains(&q)),
                    Entry::Heading(_) => false,
                });
                self.sel = hit.unwrap_or(from);
            }
            return Transition::None;
        }
        if let Some(input) = &mut self.input {
            let text = match input {
                Input::NewRound(t) | Input::Rename(t) | Input::Find(t, _) => t,
            };
            match k.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => drop(text.pop()),
                KeyCode::Char(c) => text.push(c),
                KeyCode::Enter => {
                    let input = self.input.take().unwrap();
                    self.checkpoint();
                    match input {
                        Input::NewRound(t) => {
                            self.quiz.entries.insert(self.sel.min(self.quiz.entries.len()), Entry::Heading(t));
                            self.sel += 1;
                        }
                        Input::Rename(t) => self.quiz.entries[self.sel] = Entry::Heading(t),
                        Input::Find(..) => {}
                    }
                }
                _ => {}
            }
            return Transition::None;
        }
        if self.help {
            self.help = false;
            return Transition::None;
        }
        if self.editor.is_some() {
            self.editor_key(k);
            return Transition::None;
        }
        if let Some(st) = &mut self.settings {
            match st.key(k) {
                settings::Outcome::Editing => {}
                settings::Outcome::Cancel => self.settings = None,
                settings::Outcome::Save => {
                    let st = self.settings.take().unwrap();
                    st.apply(&mut self.quiz.meta);
                    self.save = Save::Dirty(Instant::now());
                    self.flash("Settings saved");
                }
            }
            return Transition::None;
        }
        if self.review {
            self.review = false;
            if k.code == KeyCode::Enter {
                self.apply_review();
            }
            return Transition::None;
        }
        if let Some(i) = self.relink {
            let lists: Vec<_> = match ctx.lib.playlists() {
                Some(Ok(l)) => l,
                _ => Vec::new(),
            };
            match k.code {
                KeyCode::Esc => self.relink = None,
                KeyCode::Up | KeyCode::Char('k') => self.relink = Some(i.saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => self.relink = Some((i + 1).min(lists.len().saturating_sub(1))),
                KeyCode::Enter => {
                    if let Some(p) = lists.get(i) {
                        self.relink = None;
                        self.flash(format!("Linked to {}: changes show up for review (S)", p.path()));
                        self.link_playlist(ctx, p);
                    }
                }
                _ => {}
            }
            return Transition::None;
        }
        if let Some(i) = self.add_picker {
            let choice = match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.add_picker = Some(i.saturating_sub(1));
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.add_picker = Some((i + 1).min(questions::KINDS.len() - 1));
                    None
                }
                KeyCode::Enter => Some(i),
                KeyCode::Char(c @ '1'..='5') => Some(c as usize - '1' as usize),
                _ => {
                    self.add_picker = None;
                    None
                }
            };
            if let (Some(c), Some(s)) = (choice, self.song().cloned()) {
                self.add_picker = None;
                let album = self.albums(ctx).remove(&s.track_id).unwrap_or_default();
                let q = questions::prefilled(questions::KINDS[c].0, &s, &album, &self.quiz.meta.defaults.prompts);
                self.editor = Some(Editor::new(q, None));
            }
            return Transition::None;
        }
        if self.confirm_remove {
            self.confirm_remove = false;
            if k.code == KeyCode::Char('y') {
                self.remove_from_playlist(ctx);
            }
            return Transition::None;
        }
        if let Some(qi) = self.q_sel
            && self.question_key(k, qi)
        {
            return Transition::None;
        }
        if self.order_menu {
            self.order_menu = false;
            match k.code {
                KeyCode::Char('1') => self.reorder_round(ctx, Order::Shuffle),
                KeyCode::Char('2') => self.reorder_round(ctx, Order::Year),
                KeyCode::Char('3') => self.reorder_round(ctx, Order::Playlist),
                _ => {}
            }
            return Transition::None;
        }

        let n = self.rows();
        let in_entries = self.sel < self.quiz.entries.len();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') => {
                self.flush();
                return Transition::Home;
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::F(5) => {
                self.flush();
                return Transition::Run(Box::new(self.quiz.clone()), self.path.clone());
            }
            KeyCode::Char('/') => self.input = Some(Input::Find(String::new(), self.sel.min(self.quiz.entries.len().saturating_sub(1)))),
            KeyCode::Char(',') => self.settings = Some(Settings::new(&self.quiz.meta)),
            KeyCode::Char('S') => match &self.link {
                Link::Loaded(Some(_)) => self.review = true,
                Link::Loaded(None) => self.flash("The playlist has no changes"),
                _ => self.flash("Still reading the playlist"),
            },
            KeyCode::Char('R') => {
                ctx.lib.load_playlists();
                self.relink = Some(0);
            }
            KeyCode::Up | KeyCode::Char('k') if self.sel > 0 => {
                self.sel -= 1;
                self.moved();
            }
            KeyCode::Down | KeyCode::Char('j') if self.sel + 1 < n => {
                self.sel += 1;
                self.moved();
            }
            KeyCode::Char('g') => {
                self.sel = 0;
                self.moved();
            }
            KeyCode::Char('G') => {
                self.sel = n.saturating_sub(1);
                self.moved();
            }

            // Order
            KeyCode::Char('K') if in_entries && self.sel > 0 => {
                self.checkpoint();
                self.quiz.entries.swap(self.sel, self.sel - 1);
                self.sel -= 1;
            }
            KeyCode::Char('J') if in_entries && self.sel + 1 < self.quiz.entries.len() => {
                self.checkpoint();
                self.quiz.entries.swap(self.sel, self.sel + 1);
                self.sel += 1;
            }
            KeyCode::Char('x') if in_entries => {
                self.checkpoint();
                let e = self.quiz.entries.remove(self.sel);
                self.flash(format!("Cut {}: move and press p to paste", entry_name(&e)));
                self.clipboard = Some(e);
                self.sel = self.sel.min(self.quiz.entries.len().saturating_sub(1));
            }
            KeyCode::Char('p') if self.clipboard.is_some() => {
                self.checkpoint();
                let at = if self.quiz.entries.is_empty() { 0 } else { (self.sel + 1).min(self.quiz.entries.len()) };
                self.quiz.entries.insert(at, self.clipboard.take().unwrap());
                self.sel = at;
            }
            KeyCode::Char('s') if self.song_mut().is_some() => {
                self.checkpoint();
                let s = self.song_mut().unwrap();
                s.skip = !s.skip;
            }
            KeyCode::Char('o') if in_entries => self.order_menu = true,
            KeyCode::Char('u') => self.undo_redo(true),
            KeyCode::Char('r') if ctrl => self.undo_redo(false),

            // Rounds
            KeyCode::Char('n') => self.input = Some(Input::NewRound(String::new())),
            KeyCode::Char('r') => {
                if let Some(Entry::Heading(t)) = self.quiz.entries.get(self.sel) {
                    self.input = Some(Input::Rename(t.clone()));
                }
            }
            KeyCode::Char('d') if matches!(self.quiz.entries.get(self.sel), Some(Entry::Heading(_))) => {
                self.checkpoint();
                self.quiz.entries.remove(self.sel);
            }
            KeyCode::Char('D') if !in_entries && self.sel < n => {
                self.checkpoint();
                let s = self.quiz.removed.remove(self.sel - self.quiz.entries.len());
                self.flash(format!("Deleted {} for good (u undoes)", s.title));
                self.sel = self.sel.min(self.rows().saturating_sub(1));
            }

            // Questions
            KeyCode::Char('A') => self.add_default_questions(ctx),
            KeyCode::Char('a') if in_entries && self.song().is_some() => self.add_picker = Some(0),
            KeyCode::Tab | KeyCode::Enter if in_entries && self.song().is_some() => {
                if self.song().is_some_and(|s| s.questions.is_empty()) {
                    self.flash("No questions yet: a adds one");
                } else {
                    self.set_focus(Some(0));
                }
            }
            KeyCode::Char('X') if in_entries && self.song().is_some() => self.confirm_remove = true,

            // Playback and clip
            KeyCode::Char(' ') => {
                if self.song().is_some_and(|s| self.is_playing(ctx, s)) {
                    ctx.player.stop();
                } else {
                    self.play_clip(ctx);
                }
            }
            KeyCode::Char('p') => {
                if let Some(s) = self.song().filter(|_| in_entries) {
                    ctx.player.play(&self.playlist_id(), &s.track_id, 0.0, None);
                }
            }
            KeyCode::Char('m') => self.mark(ctx),
            KeyCode::Char('[') => self.nudge(-1),
            KeyCode::Char(']') => self.nudge(1),
            KeyCode::Char('{') => self.nudge(-5),
            KeyCode::Char('}') => self.nudge(5),
            KeyCode::Char('-') => self.change_length(-5),
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_length(5),
            KeyCode::Left => ctx.player.seek_by(-5.0),
            KeyCode::Right => ctx.player.seek_by(5.0),
            KeyCode::Char('P') => {
                self.auto_preview = !self.auto_preview;
                self.moved();
            }
            // Esc stops playback first; with nothing playing it goes back home.
            KeyCode::Esc if ctx.player.is_active() => ctx.player.stop(),
            KeyCode::Esc => {
                self.flush();
                return Transition::Home;
            }
            _ => {}
        }
        Transition::None
    }

    fn undo_redo(&mut self, undo: bool) {
        let (from, to) = if undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(snap) = from.pop() else { return };
        to.push((std::mem::replace(&mut self.quiz.entries, snap.0), std::mem::replace(&mut self.quiz.removed, snap.1)));
        self.sel = self.sel.min(self.rows().saturating_sub(1));
        self.save = Save::Dirty(Instant::now());
    }

    /// `m`: set the clip start to where the song is right now.
    fn mark(&mut self, ctx: &Ctx) {
        let Some(s) = self.song() else { return };
        let v = ctx.player.view();
        if !(self.is_playing(ctx, s) && v.phase == Phase::Playing) {
            return self.flash("Play the song first (p), then press m at the right moment");
        }
        self.checkpoint();
        let s = self.song_mut().unwrap();
        s.clip_start = Some(v.position.floor() as u32);
        let msg = format!("Clip starts at {}", mmss(v.position.floor()));
        self.flash(msg);
    }

    fn nudge(&mut self, d: i64) {
        if self.song_mut().is_none() {
            return;
        }
        self.checkpoint();
        let s = self.song_mut().unwrap();
        let max = s.duration.saturating_sub(1) as i64;
        s.clip_start = Some((s.clip_start.unwrap_or(0) as i64 + d).clamp(0, max.max(0)) as u32);
    }

    fn change_length(&mut self, d: i64) {
        let default = self.quiz.meta.defaults.clip_length;
        let Some(s) = self.song_mut() else { return };
        let len = (s.clip_length.unwrap_or(default) as i64 + d).clamp(5, 600) as u32;
        self.checkpoint();
        let s = self.song_mut().unwrap();
        s.clip_length = (len != default).then_some(len);
    }

    /// Album per track ID, from the playlist as last read from Music.app.
    fn albums(&self, ctx: &Ctx) -> HashMap<String, String> {
        match ctx.lib.tracks(&self.playlist_id()) {
            Some(Ok(t)) => t.into_iter().map(|t| (t.id, t.album)).collect(),
            _ => HashMap::new(),
        }
    }

    /// Keys while the questions pane has focus. Returns false to let the list handle the key.
    fn question_key(&mut self, k: KeyEvent, qi: usize) -> bool {
        let count = self.song().map_or(0, |s| s.questions.len());
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Tab | KeyCode::Esc => self.set_focus(None),
            KeyCode::Up if alt && qi > 0 => {
                self.checkpoint();
                self.song_mut().unwrap().questions.swap(qi, qi - 1);
                self.q_sel = Some(qi - 1);
            }
            KeyCode::Down if alt && qi + 1 < count => {
                self.checkpoint();
                self.song_mut().unwrap().questions.swap(qi, qi + 1);
                self.q_sel = Some(qi + 1);
            }
            KeyCode::Up | KeyCode::Char('k') => self.q_sel = Some(qi.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.q_sel = Some((qi + 1).min(count.saturating_sub(1))),
            KeyCode::Enter => {
                if let Some(q) = self.song().and_then(|s| s.questions.get(qi)) {
                    self.editor = Some(Editor::new(q.clone(), Some(qi)));
                }
            }
            KeyCode::Delete | KeyCode::Backspace if qi < count => {
                self.checkpoint();
                self.song_mut().unwrap().questions.remove(qi);
                self.set_focus(if count > 1 { Some(qi.min(count - 2)) } else { None });
            }
            _ => return false,
        }
        true
    }

    fn editor_key(&mut self, k: KeyEvent) {
        let Some(ed) = self.editor.as_mut() else { return };
        match ed.key(k) {
            Outcome::Editing => {}
            Outcome::Cancel => self.editor = None,
            Outcome::Save => {
                let ed = self.editor.take().unwrap();
                self.checkpoint();
                let qs = &mut self.song_mut().unwrap().questions;
                let i = match ed.index {
                    Some(i) => {
                        qs[i] = ed.q;
                        i
                    }
                    None => {
                        qs.push(ed.q);
                        qs.len() - 1
                    }
                };
                self.set_focus(Some(i));
            }
        }
    }

    /// `X`: take the song out of the Music.app playlist. It moves to the
    /// "not in playlist" section with its questions, like any removed song.
    fn remove_from_playlist(&mut self, ctx: &Ctx) {
        let Some(s) = self.song().cloned() else { return };
        if self.is_playing(ctx, &s) {
            ctx.player.stop();
        }
        match music::remove_from_playlist(&self.playlist_id(), &s.track_id) {
            Ok(()) => {
                self.checkpoint();
                self.quiz.entries.remove(self.sel);
                self.quiz.removed.push(s.clone());
                self.sel = self.sel.min(self.quiz.entries.len().saturating_sub(1));
                self.moved();
                ctx.lib.load_tracks(&self.playlist_id(), true);
                self.flash(format!("Removed {} from {} in Music.app (D deletes it here too)", s.title, self.quiz.meta.source.playlist));
            }
            Err(e) => self.flash(format!("Could not remove from the playlist: {e}")),
        }
    }

    fn add_default_questions(&mut self, ctx: &Ctx) {
        let albums = self.albums(ctx);
        let kinds = self.quiz.meta.defaults.questions.clone();
        let prompts = self.quiz.meta.defaults.prompts.clone();
        let todo = self.quiz.songs().filter(|s| s.questions.is_empty() && !s.skip).count();
        if todo == 0 {
            return self.flash("Every song already has questions");
        }
        self.checkpoint();
        for e in &mut self.quiz.entries {
            if let Entry::Song(s) = e
                && s.questions.is_empty()
                && !s.skip
            {
                let album = albums.get(&s.track_id).map_or("", String::as_str);
                s.questions = kinds.iter().map(|k| questions::prefilled(k, s, album, &prompts)).collect();
            }
        }
        self.flash(format!("Added {} to {todo} songs", kinds.join(" + ")));
    }

    fn reorder_round(&mut self, ctx: &Ctx, order: Order) {
        // The round is the run of songs between the headings around the cursor.
        let e = &self.quiz.entries;
        let start = (0..=self.sel.min(e.len() - 1)).rev().find(|&i| matches!(e[i], Entry::Heading(_))).map_or(0, |i| i + 1);
        let end = (start..e.len()).find(|&i| matches!(e[i], Entry::Heading(_))).unwrap_or(e.len());
        if end <= start {
            return;
        }
        let playlist: Vec<String> = match ctx.lib.tracks(&self.playlist_id()) {
            Some(Ok(t)) => t.into_iter().map(|t| t.id).collect(),
            _ if matches!(order, Order::Playlist) => return self.flash("Playlist not loaded yet"),
            _ => Vec::new(),
        };
        self.checkpoint();
        let songs = &mut self.quiz.entries[start..end];
        match order {
            Order::Shuffle => {
                let mut seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 | 1;
                for i in (1..songs.len()).rev() {
                    // xorshift is plenty for shuffling a quiz round.
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    songs.swap(i, (seed % (i as u64 + 1)) as usize);
                }
            }
            Order::Year => songs.sort_by_key(|e| if let Entry::Song(s) = e { s.year } else { 0 }),
            Order::Playlist => songs.sort_by_key(|e| match e {
                Entry::Song(s) => playlist.iter().position(|id| *id == s.track_id).unwrap_or(usize::MAX),
                _ => usize::MAX,
            }),
        }
        self.flash(match order {
            Order::Shuffle => "Round shuffled (u undoes)",
            Order::Year => "Round sorted by year",
            Order::Playlist => "Round back in playlist order",
        });
    }

    // ---------------------------------------------------------------- drawing

    pub fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let [head, body, now, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(f.area());

        let g = ctx.glyphs;
        let mut right = vec![music_badge(ctx)];
        right.push(match &self.link {
            Link::Loading | Link::ByName => Span::styled(format!("{} reading playlist", g.sync), t.dim()),
            Link::Loaded(None) => Span::styled(format!("{} playlist in sync", g.sync), t.dim()),
            Link::Loaded(Some((p, _))) => {
                let mut m = format!("{} playlist changed +{} −{}", g.sync, p.added.len(), p.removed.len());
                if !p.relinks.is_empty() {
                    m += &format!(" ↻{}", p.relinks.len());
                }
                Span::styled(m + "  S review", t.warn())
            }
            Link::Missing(e) => Span::styled(format!("{} {e}  R relink", g.error), t.warn()),
        });
        right.push(match &self.save {
            Save::Saved => Span::styled(format!("saved {}", g.check), t.dim()),
            Save::Dirty(_) => Span::styled("saving…", t.dim()),
            Save::Failed(e) => Span::styled(format!("{} not saved: {e}", g.error), t.warn()),
        });
        f.render_widget(header_line(ctx, vec![Span::raw(self.quiz.meta.title.clone())], right, head.width), head);

        let (dir, split) = if body.width >= 100 {
            (Direction::Horizontal, [Constraint::Percentage(45), Constraint::Percentage(55)])
        } else {
            (Direction::Vertical, [Constraint::Percentage(55), Constraint::Percentage(45)])
        };
        let [left, detail] = Layout::default().direction(dir).constraints(split).areas(body);
        self.draw_list(f, ctx, left);
        self.draw_detail(f, ctx, detail);
        self.draw_now_playing(f, ctx, now);

        let hint = match (&self.input, &self.msg) {
            (Some(Input::Find(q, _)), _) => Line::from(vec![
                Span::styled(" find ", t.key()),
                Span::raw(q.clone()),
                Span::styled("▏", t.accent()),
                Span::styled("   Enter keep   Esc back", t.dim()),
            ]),
            (_, Some((m, at))) if at.elapsed() < Duration::from_secs(4) => Line::from(Span::styled(format!(" {m}"), t.clip())),
            _ => hint_line(
                ctx,
                &[
                    ("space", "clip"),
                    ("p", if self.clipboard.is_some() { "paste" } else { "play" }),
                    ("m", "mark start"),
                    ("[ ]", "nudge"),
                    ("- +", "length"),
                    ("J K", "move"),
                    ("n", "round"),
                    ("a", "question"),
                    ("Tab", "questions"),
                    ("/", "find"),
                    (",", "settings"),
                    ("?", "help"),
                    ("Esc", "home"),
                ],
            ),
        };
        f.render_widget(Paragraph::new(hint).style(t.base()), foot);

        if let Some(input) = self.input.as_ref().filter(|i| !matches!(i, Input::Find(..))) {
            let (title, text) = match input {
                Input::NewRound(s) => (" New round above this song ", s),
                Input::Rename(s) | Input::Find(s, _) => (" Rename round ", s),
            };
            let area = centered(f.area(), 50, 5);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![Span::raw(text.clone()), Span::styled("▏", t.accent())]),
                    Line::from(vec![Span::styled("Enter", t.key()), Span::raw(" ok   "), Span::styled("Esc", t.key()), Span::raw(" cancel")]),
                ])
                .block(t.frame(title, true)),
                area,
            );
        }
        if self.order_menu {
            let area = centered(f.area(), 40, 7);
            f.render_widget(Clear, area);
            let item = |k: &str, what: &str| Line::from(vec![Span::styled(format!(" {k}  "), t.key()), Span::raw(what.to_string())]);
            f.render_widget(
                Paragraph::new(vec![
                    item("1", "Shuffle"),
                    item("2", "Sort by year"),
                    item("3", "Reset to playlist order"),
                    Line::styled(" Esc  keep", t.dim()),
                ])
                .block(t.frame(" Order this round ", true)),
                area,
            );
        }
        if let Some(ed) = &self.editor {
            let number = self.quiz.numbers().get(self.sel).copied().flatten();
            let count = self.song().map_or(0, |s| s.questions.len());
            let label = match ed.index {
                Some(i) => q_label(number, count, i),
                None => format!("New question for {}", number.map_or("this song".into(), |n| n.to_string())),
            };
            ed.draw(f, ctx, &label);
        }
        if let Some(i) = self.add_picker {
            let number = self.quiz.numbers().get(self.sel).copied().flatten();
            questions::draw_picker(f, ctx, &number.map_or("song".into(), |n| n.to_string()), i);
        }
        if self.confirm_remove
            && let Some(s) = self.song()
        {
            let area = centered(f.area(), 64, 6);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(vec![
                    Line::raw(format!("Remove \"{}\"", truncate(&s.title, 50))),
                    Line::raw(format!("from the playlist \"{}\" in Music.app?", self.quiz.meta.source.playlist)),
                    Line::styled("It stays in your library; its questions are kept here.", t.dim()),
                    Line::from(vec![Span::styled("y", t.key()), Span::raw(" remove   "), Span::styled("any other key", t.key()), Span::raw(" cancel")]),
                ])
                .block(t.frame(" Remove from playlist ", true)),
                area,
            );
        }
        if let Some(st) = &self.settings {
            st.draw(f, ctx);
        }
        if self.review {
            self.draw_review(f, ctx);
        }
        if let Some(i) = self.relink {
            self.draw_relink(f, ctx, i);
        }
        if self.help {
            draw_help(f, ctx);
        }
    }

    fn draw_review(&self, f: &mut Frame, ctx: &Ctx) {
        let t = ctx.theme;
        let Link::Loaded(Some((plan, _))) = &self.link else { return };
        let dest = match self.quiz.entries.iter().rev().find_map(|e| if let Entry::Heading(h) = e { Some(h) } else { None }) {
            Some(h) => format!("→ end of {h}"),
            None => "→ end".into(),
        };
        let row = |sign: &str, style, title: &str, artist: &str, note: String| {
            Line::from(vec![
                Span::styled(format!(" {sign} "), style),
                Span::raw(format!("{:<34}", truncate(title, 34))),
                Span::styled(format!("{:<20}", truncate(artist, 20)), t.dim()),
                Span::styled(note, t.dim()),
            ])
        };
        let mut lines = Vec::new();
        for tr in &plan.added {
            lines.push(row("+", t.clip(), &tr.title, &tr.artist, dest.clone()));
        }
        for s in &plan.removed {
            let note = match s.questions.len() {
                0 => "→ not in playlist".to_string(),
                n => format!("{n} question{} kept", if n == 1 { "" } else { "s" }),
            };
            lines.push(row("−", t.warn(), &s.title, &s.artist, note));
        }
        for (_, tr) in &plan.relinks {
            lines.push(row("↻", t.accent(), &tr.title, &tr.artist, "re-link (new ID)".into()));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled(" Enter", t.key()), Span::raw(" accept   "), Span::styled("Esc", t.key()), Span::raw(" later")]));
        let area = centered(f.area(), 80, lines.len() as u16 + 2);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(t.frame(" Playlist changed since last time ", true)), area);
    }

    fn draw_relink(&self, f: &mut Frame, ctx: &Ctx, sel: usize) {
        let t = ctx.theme;
        let mut lines = vec![Line::styled(format!(" Which playlist is \"{}\" now?", self.quiz.meta.source.playlist), t.dim()), Line::raw("")];
        match ctx.lib.playlists() {
            None => lines.push(Line::styled(" Loading playlists…", t.dim())),
            Some(Err(e)) => lines.push(Line::styled(format!(" {e}"), t.warn())),
            Some(Ok(all)) => {
                let first = sel.saturating_sub(12);
                for (i, p) in all.iter().enumerate().skip(first).take(14) {
                    let style = if i == sel { t.selected() } else { t.base() };
                    lines.push(Line::from(vec![
                        Span::styled(if i == sel { format!(" {} ", ctx.glyphs.cursor) } else { "   ".into() }, t.accent()),
                        Span::styled(format!("{:<44}", truncate(&p.path(), 44)), style),
                        Span::styled(format!("{:>4} songs", p.count), t.dim()),
                    ]));
                }
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled(" Enter", t.key()), Span::raw(" link   "), Span::styled("Esc", t.key()), Span::raw(" cancel")]));
        let area = centered(f.area(), 64, lines.len() as u16 + 2);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(t.frame(" Relink playlist ", true)), area);
    }

    fn draw_list(&mut self, f: &mut Frame, ctx: &Ctx, area: Rect) {
        let t = ctx.theme;
        let numbers = self.quiz.numbers();
        let w = area.width.saturating_sub(4) as usize;
        let v = ctx.player.view();
        let mut items = Vec::new();
        let mut round = 0;
        for (i, e) in self.quiz.entries.iter().enumerate() {
            items.push(match e {
                Entry::Heading(title) => {
                    round += 1;
                    let label = format!("── Round {round} · {title} ");
                    ListItem::new(Line::styled(format!("{label}{}", "─".repeat(w.saturating_sub(label.chars().count()))), t.accent()))
                }
                Entry::Song(s) => {
                    let playing = ctx.player.is_active() && v.track_id.as_deref() == Some(&s.track_id);
                    let num = numbers[i].map_or("   ".into(), |n| format!("{n:>3}"));
                    let q = if s.questions.is_empty() { Span::styled(" · ", t.dim()) } else { Span::raw(format!("{}{:<2}", ctx.glyphs.check, s.questions.len())) };
                    let clip = match s.clip_start {
                        Some(c) => Span::styled(format!("{:>5}", mmss(c as f64)), t.clip()),
                        None => Span::styled(" ────", t.faint()),
                    };
                    let problem = s.questions.is_empty() || self.overruns(s);
                    let mut tag = Span::raw("");
                    if s.skip {
                        tag = Span::styled(" skip", t.dim());
                    } else if self.new_ids.contains(&s.track_id) {
                        tag = Span::styled(" NEW", t.clip());
                    }
                    let eq = if playing { Span::styled(format!("{} ", equalizer(ctx.tick)), t.accent()) } else { Span::raw("") };
                    let fixed = 3 + 2 + eq.width() + tag.width() + 2 + 3 + 1 + 5;
                    let tw = w.saturating_sub(fixed);
                    let title_style = if s.skip { t.dim() } else if playing { t.accent() } else { t.base() };
                    ListItem::new(Line::from(vec![
                        Span::styled(num, t.dim()),
                        Span::raw("  "),
                        Span::styled(format!("{:<tw$}", truncate(&s.title, tw)), title_style),
                        eq,
                        tag,
                        if problem && !s.skip { Span::styled(" !", t.warn()) } else { Span::raw("  ") },
                        q,
                        Span::raw(" "),
                        clip,
                    ]))
                }
            });
        }
        if !self.quiz.removed.is_empty() {
            let label = "── Not in playlist any more ";
            items.push(ListItem::new(Line::styled(format!("{label}{}", "─".repeat(w.saturating_sub(label.chars().count()))), t.dim())));
            for s in &self.quiz.removed {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("  {}  ", ctx.glyphs.removed), t.dim()),
                    Span::styled(truncate(&s.title, w.saturating_sub(10)), t.dim()),
                    Span::styled(if s.questions.is_empty() { String::new() } else { format!("  {}{}", ctx.glyphs.check, s.questions.len()) }, t.dim()),
                ])));
            }
        }
        let row = if self.sel < self.quiz.entries.len() { self.sel } else { self.sel + 1 };
        self.list.select(Some(row));
        let count = self.quiz.songs().filter(|s| !s.skip).count();
        let block = t.frame_fading(Line::from(vec![Span::raw(" Songs "), Span::styled(format!("{count} "), t.dim())]), self.q_sel.is_none(), self.focus_at);
        f.render_stateful_widget(List::new(items).block(block).highlight_style(t.selected()).highlight_symbol(ctx.glyphs.cursor), area, &mut self.list);
    }

    fn overruns(&self, s: &Song) -> bool {
        s.clip_start.is_some_and(|c| c + self.clip_len(s) > s.duration)
    }

    fn draw_detail(&self, f: &mut Frame, ctx: &Ctx, area: Rect) {
        let t = ctx.theme;
        let Some(s) = self.song() else {
            let text = match self.quiz.entries.get(self.sel) {
                Some(Entry::Heading(h)) => format!("Round heading \"{h}\".\n\nr rename   d remove (its songs join the round above)"),
                _ => "No songs".into(),
            };
            f.render_widget(Paragraph::new(Span::styled(text, t.dim())).block(t.frame(" Round ", false)).wrap(Wrap { trim: false }), area);
            return;
        };
        let number = self.quiz.numbers().get(self.sel).copied().flatten();
        let title = match number {
            Some(n) => format!(" {n} · {} ", s.title),
            None => format!(" {} ", s.title),
        };
        let album = match ctx.lib.tracks(&self.playlist_id()) {
            Some(Ok(tr)) => tr.into_iter().find(|x| x.id == s.track_id).map(|x| x.album),
            _ => None,
        };
        let mut meta = vec![s.artist.clone()];
        meta.extend(album.filter(|a| !a.is_empty()));
        if s.year > 0 {
            meta.push(s.year.to_string());
        }
        meta.push(mmss(s.duration as f64));

        let inner_w = area.width.saturating_sub(4) as usize;
        let mut lines = vec![Line::styled(truncate(&meta.join(" · "), inner_w), t.dim()), Line::raw("")];
        let len = self.clip_len(s);
        lines.push(match s.clip_start {
            Some(c) => Line::from(vec![
                Span::styled("Clip  ", t.dim()),
                Span::styled(format!("{} → {}", mmss(c as f64), mmss((c + len) as f64)), t.clip()),
                Span::styled(format!("  ({len} s)"), t.dim()),
                if self.overruns(s) { Span::styled("  runs past the end", t.warn()) } else { Span::raw("") },
            ]),
            None => Line::from(vec![
                Span::styled("Clip  ", t.dim()),
                Span::styled(format!("not set ({len} s from 0:00)"), t.faint()),
                Span::styled("   p play, m mark", t.dim()),
            ]),
        });
        let (bar, marker) = self.timeline(ctx, s, inner_w);
        lines.push(bar);
        lines.push(marker);
        if s.skip {
            lines.push(Line::styled("Skipped: not numbered and left out of the quiz (s toggles)", t.dim()));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("Questions", t.base()));
        if s.questions.is_empty() {
            lines.push(Line::styled("  none yet: a adds one, A adds artist + title to every song", t.dim()));
        }
        for (i, q) in s.questions.iter().enumerate() {
            let selected = self.q_sel == Some(i);
            let label = q_label(number, s.questions.len(), i);
            let pw = inner_w.saturating_sub(label.len() + 12) / 2;
            let mut prompt = q.prompt.lines();
            let row = Line::from(vec![
                Span::styled(if selected { ctx.glyphs.cursor } else { " " }, t.accent()),
                Span::styled(format!("{label:<4}"), t.dim()),
                Span::raw(format!("{:<pw$} ", truncate(prompt.next().unwrap_or(""), pw))),
                Span::styled(format!("{:<pw$}", truncate(&q.answer, pw)), t.clip()),
                Span::styled(format!(" {} p", fmt_points(q.points)), t.dim()),
            ]);
            lines.push(if selected { row.style(t.selected()) } else { row });
            for hint in prompt {
                lines.push(Line::styled(format!("     {}", truncate(hint, pw)), t.dim()));
            }
        }
        if self.q_sel.is_some() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(" Enter edit  a add  Del remove  Alt-↑↓ reorder  Tab back", t.dim()));
        }
        f.render_widget(Paragraph::new(lines).block(t.frame_fading(title, self.q_sel.is_some(), self.focus_at)), area);
    }

    /// `0:00 ──────[████▶░░░░]────── 3:00` plus a `▲ 1:27` line under the playhead.
    fn timeline(&self, ctx: &Ctx, s: &Song, width: usize) -> (Line<'static>, Line<'static>) {
        let t = ctx.theme;
        let dur = (s.duration as f64).max(1.0);
        let (l, r) = ("0:00 ".to_string(), format!(" {}", mmss(dur)));
        let n = width.saturating_sub(l.len() + r.len()).max(10);
        let col = |secs: f64| ((secs / dur) * n as f64).clamp(0.0, n as f64 - 1.0) as usize;
        let start = s.clip_start.unwrap_or(0) as f64;
        let (cs, ce) = (col(start), col(start + self.clip_len(s) as f64).max(col(start) + 1));
        let v = ctx.player.view();
        let head = self.is_playing(ctx, s).then(|| col(v.position));
        let mut spans = vec![Span::styled(l.clone(), t.dim())];
        for i in 0..n {
            let in_clip = (cs..=ce.min(n - 1)).contains(&i);
            let (ch, style) = if Some(i) == head {
                (ctx.glyphs.cursor, t.accent())
            } else if in_clip && i == cs {
                ("[", t.clip())
            } else if in_clip && i == ce.min(n - 1) {
                ("]", t.clip())
            } else if in_clip {
                (if head.is_some_and(|h| i < h) { "█" } else { "░" }, t.clip())
            } else {
                ("─", if head.is_some_and(|h| i < h) { t.dim() } else { t.faint() })
            };
            spans.push(Span::styled(ch, style));
        }
        spans.push(Span::styled(r, t.dim()));
        let marker = match head {
            Some(h) => Line::from(vec![Span::raw(" ".repeat(l.len() + h)), Span::styled(format!("▲ {}", mmss(v.position)), t.accent())]),
            None => Line::raw(""),
        };
        (Line::from(spans), marker)
    }

    fn draw_now_playing(&self, f: &mut Frame, ctx: &Ctx, area: Rect) {
        let t = ctx.theme;
        let v = ctx.player.view();
        let title = |id: &str| self.quiz.songs().find(|s| s.track_id == id).map_or("?".to_string(), |s| s.title.clone());
        let mut spans = match (&v.phase, &v.track_id) {
            (Phase::Starting, Some(id)) => vec![Span::styled(format!(" … {}", title(id)), t.accent()), Span::styled("  starting", t.dim())],
            (Phase::Playing, Some(id)) => {
                let mut s = vec![Span::styled(format!(" {} {}  ", ctx.glyphs.playing, title(id)), t.accent()), Span::raw(mmss(v.position))];
                if let Some((start, len)) = v.clip {
                    s.push(Span::styled(format!(" / clip {} of {}", mmss(v.position - start), mmss(len)), t.clip()));
                }
                s
            }
            (Phase::Error(e), _) => vec![Span::styled(format!(" {} {e}", ctx.glyphs.error), t.warn())],
            _ => vec![Span::styled(format!(" {} stopped", ctx.glyphs.stopped), t.faint())],
        };
        let right = format!("auto-preview {} ", if self.auto_preview { "on" } else { "off" });
        let used: usize = spans.iter().map(|s| s.width()).sum();
        spans.push(Span::raw(" ".repeat((area.width as usize).saturating_sub(used + right.len()))));
        spans.push(Span::styled(right, if self.auto_preview { t.accent() } else { t.dim() }));
        f.render_widget(Paragraph::new(Line::from(spans)).style(t.base()), area);
    }
}

enum Order {
    Shuffle,
    Year,
    Playlist,
}

fn entry_name(e: &Entry) -> String {
    match e {
        Entry::Heading(t) => format!("round \"{t}\""),
        Entry::Song(s) => s.title.clone(),
    }
}

fn draw_help(f: &mut Frame, ctx: &Ctx) {
    let groups: [(&str, &[(&str, &str)]); 6] = [
        ("Move", &[("↑↓ j k", "select"), ("g G", "top / bottom"), ("/", "find song"), ("Esc q", "back to home (Esc stops music first)")]),
        ("Order", &[("J K", "move song down / up"), ("x  p", "cut, then paste below"), ("o", "order menu for this round"), ("s", "skip song"), ("u  Ctrl-R", "undo / redo")]),
        ("Play", &[("space", "play clip / stop"), ("p", "play from the start"), ("m", "mark clip start here"), ("[ ]  { }", "nudge start 1 s / 5 s"), ("- +", "clip length ±5 s"), ("← →", "seek ±5 s"), ("P", "auto-preview"), ("Esc", "stop")]),
        ("Rounds", &[("n", "new round above song"), ("r", "rename heading"), ("d", "remove heading")]),
        ("Questions", &[("Tab Enter", "questions pane"), ("a", "add question"), ("Enter", "edit question"), ("Del", "remove question"), ("Alt-↑↓", "reorder questions"), ("A", "artist + title to all")]),
        ("Other", &[("X", "remove song from the playlist"), ("D", "delete removed song here"), ("S", "review playlist changes"), ("R", "relink to another playlist"), (",", "quiz settings"), ("F5", "run the quiz")]),
    ];
    crate::app::draw_keys(f, ctx, &groups);
}

/// `5` for a song's only question, `5a`, `5b` … when it has several.
fn q_label(number: Option<usize>, count: usize, i: usize) -> String {
    match number {
        Some(n) if count > 1 => format!("{n}{}", (b'a' + i as u8) as char),
        Some(n) => n.to_string(),
        None => "-".into(),
    }
}
