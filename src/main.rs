//! Proof of concept: play songs from a Music.app playlist for a quiz, from a
//! given offset for N seconds, without bringing Music.app to the front.
//!
//! Usage:
//!   music-quiz-poc list <playlist>
//!   music-quiz-poc play <playlist> <n> [--start SECS] [--duration SECS]

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn osascript(script: &str) -> Result<String, String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("failed to run osascript: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

struct Track {
    persistent_id: String,
    name: String,
    artist: String,
    year: String,
    duration: String,
}

fn list(playlist: &str) -> Result<Vec<Track>, String> {
    // Tab-separated fields, one track per line.
    let out = osascript(&format!(
        r#"tell application "Music"
  set out to ""
  repeat with t in tracks of user playlist {}
    set out to out & (persistent ID of t) & tab & (name of t) & tab & (artist of t) & tab & (year of t) & tab & ((duration of t) as integer) & linefeed
  end repeat
  return out
end tell"#,
        quote(playlist)
    ))?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() == 5).then(|| Track {
                persistent_id: f[0].into(),
                name: f[1].into(),
                artist: f[2].into(),
                year: f[3].into(),
                duration: f[4].into(),
            })
        })
        .collect())
}

fn frontmost_app() -> String {
    osascript(r#"tell application "System Events" to get name of first process whose frontmost is true"#)
        .unwrap_or_else(|e| format!("? ({e})"))
}

fn play(playlist: &str, n: usize, start: f64, duration: f64) -> Result<(), String> {
    let tracks = list(playlist)?;
    let track = tracks
        .get(n.wrapping_sub(1))
        .ok_or(format!("playlist has {} tracks, asked for #{n}", tracks.len()))?;
    println!("Playing #{n}: {} - {} ({})", track.artist, track.name, track.persistent_id);
    let front_before = frontmost_app();

    // Play by persistent ID so the quiz can store stable references to songs.
    osascript(&format!(
        r#"tell application "Music" to play (first track of user playlist {} whose persistent ID is "{}")"#,
        quote(playlist),
        track.persistent_id
    ))?;

    // Seeking a stream that hasn't buffered yet fails with error -28; retry.
    let started = Instant::now();
    let seek = format!(r#"tell application "Music" to set player position to {start}"#);
    while let Err(e) = osascript(&seek) {
        if started.elapsed() > Duration::from_secs(5) {
            return Err(format!("seek failed: {e}"));
        }
        sleep(Duration::from_millis(250));
    }
    println!("  started and seeked to {start}s after {:.1}s", started.elapsed().as_secs_f64());

    sleep(Duration::from_secs_f64(duration));
    let pos = osascript(r#"tell application "Music" to get player position"#)?;
    osascript(r#"tell application "Music" to pause"#)?;
    println!("  paused at {pos}s");
    println!("  frontmost app before: {front_before}, after: {}", frontmost_app());
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    const USAGE: &str = "usage: music-quiz-poc list <playlist>\n       music-quiz-poc play <playlist> <n> [--start SECS] [--duration SECS]";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str, default: f64| -> Result<f64, String> {
        match args.iter().position(|a| a == name) {
            Some(i) => args.get(i + 1).ok_or(format!("{name} needs a value"))?.parse().map_err(|e| format!("{name}: {e}")),
            None => Ok(default),
        }
    };
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 2 => {
            for (i, t) in list(&args[1])?.iter().enumerate() {
                println!("{:>3}. {} - {} ({}, {}s) [{}]", i + 1, t.artist, t.name, t.year, t.duration, t.persistent_id);
            }
            Ok(())
        }
        Some("play") if args.len() >= 3 => {
            let n = args[2].parse().map_err(|e| format!("track number: {e}"))?;
            play(&args[1], n, flag("--start", 30.0)?, flag("--duration", 10.0)?)
        }
        _ => Err(USAGE.into()),
    }
}
