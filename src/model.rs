//! The quiz file (`*.quiz.toml`) and the flat running order the UI edits.

use crate::music::{Playlist, Track};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizFile {
    pub format: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub source: Source,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, rename = "round")]
    pub rounds: Vec<Round>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<Song>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub playlist: String,
    pub playlist_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_at: Option<toml::value::Datetime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub clip_length: u32,
    pub rerun_length: u32,
    pub questions: Vec<String>,
    /// Fade out the last seconds of each clip during the run.
    #[serde(default = "yes")]
    pub fade_out: bool,
    /// Size of questions and answers in the run view, 1 to `MAX_TEXT_SIZE`.
    #[serde(default = "default_text_size")]
    pub text_size: u32,
    /// Prompt per question kind, overriding the built-in Swedish ones.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prompts: BTreeMap<String, String>,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            clip_length: 30,
            rerun_length: 10,
            questions: vec!["artist".into(), "title".into()],
            fade_out: true,
            text_size: DEFAULT_TEXT_SIZE,
            prompts: BTreeMap::new(),
        }
    }
}

pub const DEFAULT_TEXT_SIZE: u32 = 2;
pub const MAX_TEXT_SIZE: u32 = 5;

fn default_text_size() -> u32 {
    DEFAULT_TEXT_SIZE
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, rename = "song")]
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Song {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub duration: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_length: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip: bool,
    #[serde(default, rename = "question", skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub kind: String,
    pub prompt: String,
    pub answer: String,
    pub points: f64,
}

impl Song {
    pub fn from_track(t: &Track) -> Self {
        Song {
            track_id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            year: t.year,
            duration: t.duration.round() as u32,
            clip_start: None,
            clip_length: None,
            skip: false,
            questions: Vec::new(),
        }
    }
}

/// One line of the running order: a round heading or a song.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Heading(String),
    Song(Song),
}

/// The quiz as the UI edits it: rounds flattened into headings between songs.
#[derive(Debug, Clone)]
pub struct Quiz {
    pub meta: QuizFile,
    pub entries: Vec<Entry>,
    pub removed: Vec<Song>,
}

impl Quiz {
    pub fn new(title: &str, playlist: &Playlist, tracks: &[Track]) -> Self {
        let meta = QuizFile {
            format: 1,
            title: title.to_string(),
            subtitle: None,
            instructions: None,
            source: Source {
                playlist: playlist.name.clone(),
                playlist_id: playlist.id.clone(),
                folder: playlist.folder.clone(),
                synced_at: Some(now_utc()),
            },
            defaults: Defaults::default(),
            rounds: Vec::new(),
            removed: Vec::new(),
        };
        let entries = tracks.iter().map(|t| Entry::Song(Song::from_track(t))).collect();
        Quiz { meta, entries, removed: Vec::new() }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut meta: QuizFile = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut entries = Vec::new();
        for round in std::mem::take(&mut meta.rounds) {
            if let Some(t) = round.title {
                entries.push(Entry::Heading(t));
            }
            entries.extend(round.songs.into_iter().map(Entry::Song));
        }
        let removed = std::mem::take(&mut meta.removed);
        Ok(Quiz { meta, entries, removed })
    }

    pub fn to_file(&self) -> QuizFile {
        let mut rounds: Vec<Round> = vec![Round { title: None, songs: Vec::new() }];
        for e in &self.entries {
            match e {
                Entry::Heading(t) => {
                    if rounds.len() == 1 && rounds[0].title.is_none() && rounds[0].songs.is_empty() {
                        rounds.clear();
                    }
                    rounds.push(Round { title: Some(t.clone()), songs: Vec::new() });
                }
                Entry::Song(s) => rounds.last_mut().unwrap().songs.push(s.clone()),
            }
        }
        QuizFile { rounds, removed: self.removed.clone(), ..self.meta.clone() }
    }

    /// Write to a temp file, then rename, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(&self.to_file())?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Pick up songs added to or removed from the playlist since last time.
    /// New songs go to the end; removed songs keep their questions in `removed`
    /// and come back if re-added. The order in Music.app is ignored.
    /// Returns the track IDs of the newly added songs and the number removed.
    pub fn sync(&mut self, tracks: &[Track]) -> (Vec<String>, usize) {
        let in_playlist: HashSet<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let mut removed_now = 0;
        self.entries.retain(|e| match e {
            Entry::Song(s) if !in_playlist.contains(s.track_id.as_str()) => {
                self.removed.push(s.clone());
                removed_now += 1;
                false
            }
            _ => true,
        });
        let present: HashSet<String> = self.songs().map(|s| s.track_id.clone()).collect();
        let mut added = Vec::new();
        for t in tracks.iter().filter(|t| !present.contains(&t.id)) {
            let song = match self.removed.iter().position(|s| s.track_id == t.id) {
                Some(i) => self.removed.remove(i),
                None => Song::from_track(t),
            };
            added.push(song.track_id.clone());
            self.entries.push(Entry::Song(song));
        }
        self.meta.source.synced_at = Some(now_utc());
        (added, removed_now)
    }

    /// What `sync` would change, plus songs whose track ID stopped resolving but
    /// match a playlist track on title, artist and length (another Mac, a
    /// library rebuild). Nothing changes until `apply_sync`.
    pub fn plan_sync(&self, tracks: &[Track]) -> SyncPlan {
        let in_playlist: HashSet<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let in_quiz: HashSet<&str> = self.songs().map(|s| s.track_id.as_str()).collect();
        let mut candidates: Vec<&Track> = tracks.iter().filter(|t| !in_quiz.contains(t.id.as_str())).collect();
        let mut relinks = Vec::new();
        let mut removed = Vec::new();
        for s in self.songs().filter(|s| !in_playlist.contains(s.track_id.as_str())) {
            let same = |t: &&Track| {
                t.title.eq_ignore_ascii_case(&s.title)
                    && t.artist.eq_ignore_ascii_case(&s.artist)
                    && (t.duration.round() as i64 - s.duration as i64).abs() <= 2
            };
            match candidates.iter().position(same) {
                Some(i) => relinks.push((s.track_id.clone(), candidates.remove(i).clone())),
                None => removed.push(s.clone()),
            }
        }
        SyncPlan { added: candidates.into_iter().cloned().collect(), removed, relinks }
    }

    /// Apply a reviewed plan. Returns the track IDs of added songs.
    pub fn apply_sync(&mut self, plan: &SyncPlan, tracks: &[Track]) -> Vec<String> {
        for (old, t) in &plan.relinks {
            for e in &mut self.entries {
                if let Entry::Song(s) = e
                    && s.track_id == *old
                {
                    s.track_id = t.id.clone();
                }
            }
        }
        self.sync(tracks).0
    }

    pub fn songs(&self) -> impl Iterator<Item = &Song> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Song(s) => Some(s),
            _ => None,
        })
    }

    /// Song numbers as printed on the sheet: continuous across rounds, skipped songs unnumbered.
    pub fn numbers(&self) -> Vec<Option<usize>> {
        let mut n = 0;
        self.entries
            .iter()
            .map(|e| match e {
                Entry::Song(s) if !s.skip => {
                    n += 1;
                    Some(n)
                }
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncPlan {
    pub added: Vec<Track>,
    pub removed: Vec<Song>,
    /// (old track ID, the playlist track it matches)
    pub relinks: Vec<(String, Track)>,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.relinks.is_empty()
    }
}

/// `midsommarquiz-2024.quiz.toml` from "Midsommarquiz 2024".
pub fn file_name_for(title: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        let c = match c {
            'å' | 'ä' => 'a',
            'ö' => 'o',
            c if c.is_alphanumeric() => c,
            _ => '-',
        };
        if !(c == '-' && (slug.is_empty() || slug.ends_with('-'))) {
            slug.push(c);
        }
    }
    format!("{}.quiz.toml", slug.trim_end_matches('-'))
}

fn now_utc() -> toml::value::Datetime {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
    .parse()
    .expect("valid datetime")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track { id: id.into(), title: id.into(), artist: "A".into(), album: "B".into(), year: 1984, duration: 180.4 }
    }

    fn playlist() -> Playlist {
        Playlist { id: "P".into(), name: "Quiz".into(), folder: String::new(), smart: false, duration: 0.0, count: 0 }
    }

    #[test]
    fn rounds_round_trip_through_toml() {
        let mut q = Quiz::new("Midsommarquiz 2025", &playlist(), &[track("1"), track("2"), track("3")]);
        q.entries.insert(0, Entry::Heading("Filmmusik".into()));
        q.entries.insert(3, Entry::Heading("80-tal".into()));
        let text = toml::to_string_pretty(&q.to_file()).unwrap();
        assert!(text.contains("[[round.song]]"), "{text}");
        let dir = std::env::temp_dir().join(format!("mq-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.quiz.toml");
        q.save(&path).unwrap();
        let back = Quiz::load(&path).unwrap();
        assert_eq!(back.entries, q.entries);
    }

    #[test]
    fn untitled_first_round_is_kept_before_a_heading() {
        let mut q = Quiz::new("Q", &playlist(), &[track("1"), track("2")]);
        q.entries.insert(1, Entry::Heading("R2".into()));
        let f = q.to_file();
        assert_eq!(f.rounds.len(), 2);
        assert_eq!(f.rounds[0].title, None);
        assert_eq!(f.rounds[1].title.as_deref(), Some("R2"));
    }

    #[test]
    fn sync_appends_new_and_keeps_removed_questions() {
        let mut q = Quiz::new("Q", &playlist(), &[track("1"), track("2")]);
        if let Entry::Song(s) = &mut q.entries[1] {
            s.questions.push(Question { kind: "artist".into(), prompt: "?".into(), answer: "A".into(), points: 1.0 });
        }
        let (added, removed) = q.sync(&[track("1"), track("3")]);
        assert_eq!((added, removed), (vec!["3".to_string()], 1));
        assert_eq!(q.removed[0].questions.len(), 1);
        let (added, _) = q.sync(&[track("1"), track("3"), track("2")]);
        assert_eq!(added, vec!["2".to_string()]);
        assert!(q.removed.is_empty());
        assert_eq!(q.songs().last().unwrap().questions.len(), 1);
    }

    #[test]
    fn plan_sync_relinks_same_song_with_new_id() {
        let mut q = Quiz::new("Q", &playlist(), &[track("1"), track("2")]);
        let mut moved = track("9");
        moved.title = "2".into();
        let plan = q.plan_sync(&[track("1"), moved.clone(), track("3")]);
        assert_eq!(plan.relinks.len(), 1);
        assert_eq!(plan.relinks[0].0, "2");
        assert_eq!(plan.added.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["3"]);
        assert!(plan.removed.is_empty());
        let added = q.apply_sync(&plan, &[track("1"), moved, track("3")]);
        assert_eq!(added, vec!["3".to_string()]);
        let ids: Vec<&str> = q.songs().map(|s| s.track_id.as_str()).collect();
        assert_eq!(ids, vec!["1", "9", "3"]);
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name_for("Midsommarquiz 2024"), "midsommarquiz-2024.quiz.toml");
        assert_eq!(file_name_for("Jul & Nyår!"), "jul-nyar.quiz.toml");
    }
}
