//! Before the event: is Music.app reachable, does every song resolve and play.
//! Shared by `mq check` and the run view's setup screen.

use crate::model::{Entry, Quiz};
use crate::music::{self, Track};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub level: Level,
    pub text: String,
}

fn item(level: Level, text: impl Into<String>) -> Item {
    Item { level, text: text.into() }
}

/// `titles` names songs in the messages; the setup screen is projected, so it
/// passes false and only shows song numbers.
pub fn check(quiz: &Quiz, tracks: &Result<Vec<Track>, String>, titles: bool) -> Vec<Item> {
    let mut out = Vec::new();
    let tracks = match tracks {
        Ok(t) => {
            out.push(item(Level::Ok, format!("Playlist \"{}\" found: {} songs", quiz.meta.source.playlist, t.len())));
            t
        }
        Err(e) => {
            out.push(item(Level::Fail, format!("Playlist \"{}\": {e}", quiz.meta.source.playlist)));
            return out;
        }
    };
    let numbers = quiz.numbers();
    let songs: Vec<(usize, &crate::model::Song)> = quiz
        .entries
        .iter()
        .zip(&numbers)
        .filter_map(|(e, n)| match (e, n) {
            (Entry::Song(s), Some(n)) => Some((*n, s)),
            _ => None,
        })
        .collect();
    let name = |n: usize, s: &crate::model::Song| if titles { format!("{n} ({})", s.title) } else { n.to_string() };
    let list = |v: Vec<String>| v.join(", ");

    let missing: Vec<String> = songs.iter().filter(|(_, s)| !tracks.iter().any(|t| t.id == s.track_id)).map(|(n, s)| name(*n, s)).collect();
    if missing.is_empty() {
        out.push(item(Level::Ok, format!("All {} songs are in the playlist", songs.len())));
    } else {
        out.push(item(Level::Fail, format!("Not in the playlist: {} (open the quiz to review)", list(missing))));
    }
    if quiz.matched_by_name() > 0 {
        out.push(item(Level::Ok, format!("{} songs found by title, artist and length (other IDs in this library)", quiz.matched_by_name())));
    }
    let no_clip: Vec<String> = songs.iter().filter(|(_, s)| s.clip_start.is_none()).map(|(n, s)| name(*n, s)).collect();
    if !no_clip.is_empty() {
        out.push(item(Level::Warn, format!("No clip start, will play from 0:00: {}", list(no_clip))));
    }
    let default = quiz.meta.defaults.clip_length;
    let overrun: Vec<String> = songs
        .iter()
        .filter(|(_, s)| s.clip_start.is_some_and(|c| c + s.clip_length.unwrap_or(default) > s.duration))
        .map(|(n, s)| name(*n, s))
        .collect();
    if !overrun.is_empty() {
        out.push(item(Level::Warn, format!("Clip runs past the end of the song: {}", list(overrun))));
    }
    let no_q: Vec<String> = songs.iter().filter(|(_, s)| s.questions.is_empty()).map(|(n, s)| name(*n, s)).collect();
    if !no_q.is_empty() {
        out.push(item(Level::Warn, format!("No questions: {}", list(no_q))));
    }
    let extra = tracks.iter().filter(|t| !quiz.songs().any(|s| s.track_id == t.id)).count();
    if extra > 0 {
        out.push(item(Level::Warn, format!("{extra} new songs in the playlist are not in the quiz yet (open the quiz to review)")));
    }
    out
}

/// Start every song muted, seek to its clip and stop again, to prove it plays
/// (and gets it cached). Calls `progress(done, total)` after each song.
pub fn play_check(quiz: &Quiz, mut progress: impl FnMut(usize, usize)) -> Vec<Item> {
    let pl = quiz.meta.source.playlist_id.clone();
    let songs: Vec<_> = quiz.songs().filter(|s| !s.skip).cloned().collect();
    let numbers: Vec<usize> = quiz.numbers().into_iter().flatten().collect();
    let volume = music::volume().unwrap_or(100);
    let _ = music::set_volume(0);
    let mut failed = Vec::new();
    for (i, s) in songs.iter().enumerate() {
        let ok = music::play(&pl, &s.track_id).is_ok() && {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut ok = false;
            while Instant::now() < deadline {
                if let Ok(st) = music::status()
                    && st.state == "playing"
                    && st.track_id.as_deref() == Some(&s.track_id)
                    && music::seek(s.clip_start.unwrap_or(0) as f64).is_ok()
                {
                    ok = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            ok
        };
        let _ = music::pause();
        if !ok {
            failed.push(numbers.get(i).copied().unwrap_or(i + 1).to_string());
        }
        progress(i + 1, songs.len());
    }
    let _ = music::set_volume(volume);
    if failed.is_empty() {
        vec![item(Level::Ok, format!("All {} songs started playing", songs.len()))]
    } else {
        vec![item(Level::Fail, format!("Did not start: {}", failed.join(", ")))]
    }
}

/// `mq check <file> [--play]`
pub fn cli(path: &std::path::Path, play: bool) -> anyhow::Result<bool> {
    let mut quiz = Quiz::load(path)?;
    let status = music::status();
    match &status {
        Ok(s) if s.state == "not running" => println!("  ○ Music.app is not running (it starts on the first song)"),
        Ok(_) => println!("  ✓ Music.app can be controlled"),
        Err(e) => println!("  ✗ Music.app: {e}"),
    }
    let mut tracks = music::tracks(&quiz.meta.source.playlist_id).map_err(|e| e.to_string());
    if tracks.is_err()
        && let Ok(all) = music::playlists()
        && let Some(p) = all.iter().find(|p| p.name == quiz.meta.source.playlist)
    {
        quiz.use_playlist(&p.id);
        tracks = music::tracks(&p.id).map_err(|e| e.to_string());
    }
    if let Ok(t) = &tracks {
        quiz.use_library(t);
    }
    let mut items = check(&quiz, &tracks, true);
    if play && tracks.is_ok() {
        items.extend(play_check(&quiz, |done, total| eprint!("\r  playing {done}/{total} muted…")));
        eprintln!();
    }
    for i in &items {
        let mark = match i.level {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => "✗",
        };
        println!("  {mark} {}", i.text);
    }
    if !play {
        println!("  (add --play to start every song muted and prove it plays)");
    }
    Ok(status.is_ok() && items.iter().all(|i| i.level != Level::Fail))
}
