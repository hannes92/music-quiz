//! Music.app access through `osascript` running JXA. Every call returns JSON.
//! Nothing here brings Music.app to the front.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::process::Command;

const SCRIPT: &str = r#"
function run(argv) {
  const M = Application('Music');
  const cmd = argv[0];
  const playlist = id => {
    const p = M.userPlaylists.whose({persistentID: id});
    if (p.length === 0) throw new Error('playlist not found: ' + id);
    return p[0];
  };
  if (cmd === 'status') {
    if (!M.running()) return JSON.stringify({state: 'not running'});
    const r = {state: M.playerState()};
    try { r.position = M.playerPosition(); r.track_id = M.currentTrack.persistentID(); } catch (e) {}
    return JSON.stringify(r);
  }
  if (cmd === 'playlists') {
    const P = M.userPlaylists;
    const names = P.name(), ids = P.persistentID(), smart = P.smart(),
          kind = P.specialKind(), dur = P.duration(), cls = P.class();
    const out = [];
    for (let i = 0; i < names.length; i++) {
      if (kind[i] !== 'none' || cls[i] === 'folderPlaylist') continue;
      const path = [];
      try { let f = P[i].parent; for (let d = 0; d < 8; d++) { path.unshift(f.name()); f = f.parent; } } catch (e) {}
      out.push({id: ids[i], name: names[i], folder: path.join(' / '), smart: smart[i],
                duration: dur[i], count: P[i].tracks.length});
    }
    return JSON.stringify(out);
  }
  if (cmd === 'tracks') {
    const t = playlist(argv[1]).tracks;
    const ids = t.persistentID(), names = t.name(), artists = t.artist(), albums = t.album(),
          years = t.year(), durs = t.duration();
    return JSON.stringify(ids.map((id, i) => ({id, title: names[i], artist: artists[i],
      album: albums[i], year: years[i], duration: durs[i]})));
  }
  if (cmd === 'play') {
    const t = playlist(argv[1]).tracks.whose({persistentID: argv[2]});
    if (t.length === 0) throw new Error('track not in playlist: ' + argv[2]);
    t[0].play();
    return '{}';
  }
  if (cmd === 'seek') { M.playerPosition = parseFloat(argv[1]); return '{}'; }
  if (cmd === 'pause') { if (M.running()) M.pause(); return '{}'; }
  throw new Error('unknown command ' + cmd);
}
"#;

fn jxa(args: &[&str]) -> Result<String> {
    let out = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT])
        .args(args)
        .output()
        .context("failed to run osascript")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // -1743: the terminal is not allowed to control Music.app.
        if err.contains("-1743") {
            bail!("no permission to control Music (System Settings → Privacy & Security → Automation)");
        }
        return Err(anyhow!(err));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub smart: bool,
    pub duration: f64,
    pub count: usize,
}

impl Playlist {
    pub fn path(&self) -> String {
        if self.folder.is_empty() { self.name.clone() } else { format!("{} / {}", self.folder, self.name) }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: u32,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Status {
    pub state: String,
    pub position: Option<f64>,
    pub track_id: Option<String>,
}

pub fn playlists() -> Result<Vec<Playlist>> {
    Ok(serde_json::from_str(&jxa(&["playlists"])?)?)
}

pub fn tracks(playlist_id: &str) -> Result<Vec<Track>> {
    Ok(serde_json::from_str(&jxa(&["tracks", playlist_id])?)?)
}

pub fn status() -> Result<Status> {
    Ok(serde_json::from_str(&jxa(&["status"])?)?)
}

pub fn play(playlist_id: &str, track_id: &str) -> Result<()> {
    jxa(&["play", playlist_id, track_id]).map(drop)
}

pub fn seek(position: f64) -> Result<()> {
    jxa(&["seek", &position.to_string()]).map(drop)
}

pub fn pause() -> Result<()> {
    jxa(&["pause"]).map(drop)
}
