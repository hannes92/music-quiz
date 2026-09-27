//! Background threads so the UI never waits on Music.app: one drives playback
//! and polls player state, the other loads playlists and tracks.

use crate::music::{self, Playlist, Track};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Seconds of fade at the end of a clip when fading is on.
const FADE_SECS: f64 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Connection {
    Unknown,
    Connected,
    NotRunning,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Starting,
    Playing,
    Paused,
    Error(String),
}

/// What the UI shows about playback; refreshed by the player thread.
#[derive(Debug, Clone)]
pub struct PlayerView {
    pub connection: Connection,
    pub phase: Phase,
    /// Track we asked for (while starting, playing or paused).
    pub track_id: Option<String>,
    pub position: f64,
    /// Clip being played: (start, length). `None` when playing the full track.
    pub clip: Option<(f64, f64)>,
    /// Seconds of the clip heard so far, timed from when Music.app started playing.
    pub clip_elapsed: f64,
    /// Counts clips that played to their end, so a UI can advance after one.
    pub finished: u64,
}

enum Cmd {
    Play { playlist: String, track: String, start: f64, clip_length: Option<f64>, fade: bool },
    Seek(f64),
    Pause,
    Resume,
    Stop,
}

struct Clip {
    length: f64,
    fade: bool,
}

enum State {
    Idle,
    Starting { track: String, start: f64, clip: Option<Clip>, since: Instant },
    Playing { track: String, clip: Option<Clip>, clip_started: Instant, volume: Option<i64> },
    Paused { track: String, clip: Option<Clip>, elapsed: f64, volume: Option<i64> },
}

pub struct Player {
    tx: Sender<Cmd>,
    view: Arc<Mutex<PlayerView>>,
}

impl Player {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let view = Arc::new(Mutex::new(PlayerView {
            connection: Connection::Unknown,
            phase: Phase::Idle,
            track_id: None,
            position: 0.0,
            clip: None,
            clip_elapsed: 0.0,
            finished: 0,
        }));
        let v = view.clone();
        thread::spawn(move || player_loop(rx, v));
        Player { tx, view }
    }

    pub fn view(&self) -> PlayerView {
        self.view.lock().unwrap().clone()
    }

    /// Play `clip_length` seconds from `start`, or the whole track from `start` when `None`.
    pub fn play(&self, playlist: &str, track: &str, start: f64, clip_length: Option<f64>) {
        self.play_faded(playlist, track, start, clip_length, false);
    }

    /// Like `play`; with `fade` the clip's last seconds fade out.
    pub fn play_faded(&self, playlist: &str, track: &str, start: f64, clip_length: Option<f64>, fade: bool) {
        let _ = self.tx.send(Cmd::Play { playlist: playlist.into(), track: track.into(), start, clip_length, fade });
    }

    pub fn seek_by(&self, delta: f64) {
        let _ = self.tx.send(Cmd::Seek(delta));
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// Starting or playing (not paused).
    pub fn is_active(&self) -> bool {
        matches!(self.view().phase, Phase::Starting | Phase::Playing)
    }
}

fn restore(volume: Option<i64>) {
    if let Some(v) = volume {
        let _ = music::set_volume(v);
    }
}

fn player_loop(rx: Receiver<Cmd>, view: Arc<Mutex<PlayerView>>) {
    let mut state = State::Idle;
    let set = |f: &dyn Fn(&mut PlayerView)| f(&mut view.lock().unwrap());
    loop {
        let wait = if matches!(state, State::Idle | State::Paused { .. }) { 1500 } else { 150 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(Cmd::Play { playlist, track, start, clip_length, fade }) => {
                if let State::Playing { volume, .. } | State::Paused { volume, .. } = &state {
                    restore(*volume);
                }
                set(&|v| {
                    v.phase = Phase::Starting;
                    v.track_id = Some(track.clone());
                    v.position = start;
                    v.clip = clip_length.map(|l| (start, l));
                    v.clip_elapsed = 0.0;
                });
                let clip = clip_length.map(|length| Clip { length, fade });
                match music::play(&playlist, &track) {
                    Ok(()) => state = State::Starting { track, start, clip, since: Instant::now() },
                    Err(e) => {
                        set(&|v| v.phase = Phase::Error(e.to_string()));
                        state = State::Idle;
                    }
                }
                continue;
            }
            Ok(Cmd::Seek(delta)) => {
                if let State::Playing { .. } = state {
                    let pos = view.lock().unwrap().position;
                    let _ = music::seek((pos + delta).max(0.0));
                }
            }
            Ok(Cmd::Pause) => {
                if let State::Playing { track, clip, clip_started, volume } = state {
                    let _ = music::pause();
                    set(&|v| v.phase = Phase::Paused);
                    state = State::Paused { track, clip, elapsed: clip_started.elapsed().as_secs_f64(), volume };
                }
                continue;
            }
            Ok(Cmd::Resume) => {
                if let State::Paused { track, clip, elapsed, volume } = state {
                    let _ = music::resume();
                    set(&|v| v.phase = Phase::Playing);
                    state = State::Playing { track, clip, clip_started: Instant::now() - Duration::from_secs_f64(elapsed), volume };
                }
                continue;
            }
            Ok(Cmd::Stop) => {
                if let State::Playing { volume, .. } | State::Paused { volume, .. } = &state {
                    let _ = music::pause();
                    restore(*volume);
                } else {
                    let _ = music::pause();
                }
                state = State::Idle;
                set(&|v| {
                    v.phase = Phase::Idle;
                    v.track_id = None;
                    v.clip = None;
                });
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let State::Playing { volume, .. } | State::Paused { volume, .. } = &state {
                    let _ = music::pause();
                    restore(*volume);
                }
                return;
            }
        }

        let status = match music::status() {
            Ok(s) => s,
            Err(e) => {
                set(&|v| v.connection = Connection::Error(e.to_string()));
                continue;
            }
        };
        set(&|v| {
            v.connection = if status.state == "not running" { Connection::NotRunning } else { Connection::Connected };
        });
        let on_track = |t: &str| status.state == "playing" && status.track_id.as_deref() == Some(t);

        state = match state {
            State::Idle => State::Idle,
            State::Paused { track, clip, elapsed, volume } => {
                if status.track_id.as_deref() == Some(&track) && status.state == "paused" {
                    State::Paused { track, clip, elapsed, volume }
                } else {
                    // Changed from outside mq.
                    restore(volume);
                    State::Idle
                }
            }
            State::Starting { track, start, clip, since } => {
                if on_track(&track) {
                    // Seeking a stream that hasn't buffered yet fails with -28; retry next tick.
                    if start <= 0.0 || music::seek(start).is_ok() {
                        // The clip timer starts once Music.app is actually playing.
                        State::Playing { track, clip, clip_started: Instant::now(), volume: None }
                    } else {
                        State::Starting { track, start, clip, since }
                    }
                } else if since.elapsed() > Duration::from_secs(10) {
                    set(&|v| v.phase = Phase::Error("Music.app did not start the song".into()));
                    State::Idle
                } else {
                    State::Starting { track, start, clip, since }
                }
            }
            State::Playing { track, clip, clip_started, mut volume } => {
                if let Some(p) = status.position {
                    set(&|v| v.position = p);
                }
                let elapsed = clip_started.elapsed().as_secs_f64();
                set(&|v| v.clip_elapsed = elapsed);
                let remaining = clip.as_ref().map(|c| c.length - elapsed);
                if remaining.is_some_and(|r| r <= 0.0) {
                    let _ = music::pause();
                    restore(volume);
                    set(&|v| v.finished += 1);
                    State::Idle
                } else if !on_track(&track) {
                    // Paused, stopped or changed song from outside mq.
                    restore(volume);
                    State::Idle
                } else {
                    if let (Some(c), Some(r)) = (&clip, remaining)
                        && c.fade
                        && r < FADE_SECS
                    {
                        let from = *volume.get_or_insert_with(|| music::volume().unwrap_or(100));
                        let _ = music::set_volume((from as f64 * r / FADE_SECS).round() as i64);
                    }
                    set(&|v| v.phase = Phase::Playing);
                    State::Playing { track, clip, clip_started, volume }
                }
            }
        };
        if matches!(state, State::Idle) {
            set(&|v| {
                if !matches!(v.phase, Phase::Error(_)) {
                    v.phase = Phase::Idle;
                    v.track_id = None;
                    v.clip = None;
                }
            });
        }
    }
}

/// Cached playlist and track lists, loaded in the background on request.
#[derive(Default)]
pub struct LibraryCache {
    pub playlists: Option<Result<Vec<Playlist>, String>>,
    pub tracks: HashMap<String, Result<Vec<Track>, String>>,
    pending: HashSet<String>,
}

enum LibCmd {
    Playlists,
    Tracks(String),
}

pub struct Library {
    tx: Sender<LibCmd>,
    pub cache: Arc<Mutex<LibraryCache>>,
}

impl Library {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<LibCmd>();
        let cache = Arc::new(Mutex::new(LibraryCache::default()));
        let c = cache.clone();
        thread::spawn(move || {
            for cmd in rx {
                match cmd {
                    LibCmd::Playlists => {
                        let r = music::playlists().map_err(|e| e.to_string());
                        c.lock().unwrap().playlists = Some(r);
                    }
                    LibCmd::Tracks(id) => {
                        let r = music::tracks(&id).map_err(|e| e.to_string());
                        let mut c = c.lock().unwrap();
                        c.pending.remove(&id);
                        c.tracks.insert(id, r);
                    }
                }
            }
        });
        Library { tx, cache }
    }

    pub fn load_playlists(&self) {
        self.cache.lock().unwrap().playlists = None;
        let _ = self.tx.send(LibCmd::Playlists);
    }

    /// Load tracks unless already cached or on the way. `force` reloads.
    pub fn load_tracks(&self, id: &str, force: bool) {
        let mut c = self.cache.lock().unwrap();
        if c.pending.contains(id) || (!force && c.tracks.contains_key(id)) {
            return;
        }
        if force {
            c.tracks.remove(id);
        }
        c.pending.insert(id.to_string());
        let _ = self.tx.send(LibCmd::Tracks(id.to_string()));
    }

    pub fn tracks(&self, id: &str) -> Option<Result<Vec<Track>, String>> {
        self.cache.lock().unwrap().tracks.get(id).cloned()
    }

    pub fn playlists(&self) -> Option<Result<Vec<Playlist>, String>> {
        self.cache.lock().unwrap().playlists.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays 5 s of a real song with a pause in the middle and a fade at the end,
    /// then checks the clip finished and the volume came back.
    /// Run with: cargo test fade -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fade_pause_resume_and_finish() {
        let pl = music::playlists().unwrap().into_iter().find(|p| p.name == "Midsommarquiz 2024").unwrap();
        let track = music::tracks(&pl.id).unwrap().remove(4);
        let volume = music::volume().unwrap();
        let p = Player::spawn();
        let before = p.view().finished;
        p.play_faded(&pl.id, &track.id, 20.0, Some(5.0), true);
        let wait_for = |what: &dyn Fn(&PlayerView) -> bool| {
            for _ in 0..150 {
                if what(&p.view()) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("timed out: {:?}", p.view());
        };
        wait_for(&|v| v.phase == Phase::Playing);
        thread::sleep(Duration::from_secs(1));
        p.pause();
        wait_for(&|v| v.phase == Phase::Paused);
        let paused_at = p.view().clip_elapsed;
        thread::sleep(Duration::from_secs(2));
        p.resume();
        wait_for(&|v| v.phase == Phase::Playing);
        wait_for(&|v| v.finished > before);
        let v = p.view();
        println!("paused at {paused_at:.1}s, finished at position {:.1}, volume {} -> {}", v.position, volume, music::volume().unwrap());
        assert_eq!(music::volume().unwrap(), volume);
        assert!(v.position >= 22.5 && v.position < 27.5, "{}", v.position);
    }
}
