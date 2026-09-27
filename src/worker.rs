//! Background threads so the UI never waits on Music.app: one drives playback
//! and polls player state, the other loads playlists and tracks.

use crate::music::{self, Playlist, Track};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    Error(String),
}

/// What the UI shows about playback; refreshed by the player thread.
#[derive(Debug, Clone)]
pub struct PlayerView {
    pub connection: Connection,
    pub phase: Phase,
    /// Track we asked for (while starting or playing).
    pub track_id: Option<String>,
    pub position: f64,
    /// Clip being played: (start, length). `None` when playing the full track.
    pub clip: Option<(f64, f64)>,
}

enum Cmd {
    Play { playlist: String, track: String, start: f64, clip_length: Option<f64> },
    Seek(f64),
    Stop,
}

enum State {
    Idle,
    Starting { track: String, start: f64, clip_length: Option<f64>, since: Instant },
    Playing { track: String, clip: Option<(f64, f64)>, clip_started: Instant },
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
        let _ = self.tx.send(Cmd::Play { playlist: playlist.into(), track: track.into(), start, clip_length });
    }

    pub fn seek_by(&self, delta: f64) {
        let _ = self.tx.send(Cmd::Seek(delta));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn is_active(&self) -> bool {
        matches!(self.view().phase, Phase::Starting | Phase::Playing)
    }
}

fn player_loop(rx: Receiver<Cmd>, view: Arc<Mutex<PlayerView>>) {
    let mut state = State::Idle;
    let set = |f: &dyn Fn(&mut PlayerView)| f(&mut view.lock().unwrap());
    loop {
        let wait = if matches!(state, State::Idle) { 1500 } else { 150 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(Cmd::Play { playlist, track, start, clip_length }) => {
                set(&|v| {
                    v.phase = Phase::Starting;
                    v.track_id = Some(track.clone());
                    v.position = start;
                    v.clip = clip_length.map(|l| (start, l));
                });
                match music::play(&playlist, &track) {
                    Ok(()) => state = State::Starting { track, start, clip_length, since: Instant::now() },
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
            Ok(Cmd::Stop) => {
                let _ = music::pause();
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
                if !matches!(state, State::Idle) {
                    let _ = music::pause();
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
            State::Starting { track, start, clip_length, since } => {
                if on_track(&track) {
                    // Seeking a stream that hasn't buffered yet fails with -28; retry next tick.
                    if start <= 0.0 || music::seek(start).is_ok() {
                        // The clip timer starts once Music.app is actually playing.
                        State::Playing { track, clip: clip_length.map(|l| (start, l)), clip_started: Instant::now() }
                    } else {
                        State::Starting { track, start, clip_length, since }
                    }
                } else if since.elapsed() > Duration::from_secs(10) {
                    set(&|v| v.phase = Phase::Error("Music.app did not start the song".into()));
                    State::Idle
                } else {
                    State::Starting { track, start, clip_length, since }
                }
            }
            State::Playing { track, clip, clip_started } => {
                if let Some(p) = status.position {
                    set(&|v| v.position = p);
                }
                let clip_done = clip.is_some_and(|(_, len)| clip_started.elapsed().as_secs_f64() >= len);
                if clip_done {
                    let _ = music::pause();
                    State::Idle
                } else if !on_track(&track) {
                    // Paused, stopped or changed song from outside mq.
                    State::Idle
                } else {
                    set(&|v| v.phase = Phase::Playing);
                    State::Playing { track, clip, clip_started }
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
