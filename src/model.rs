//! The quiz file (`*.quiz.toml`) and the flat running order the UI edits.

use crate::music::{Playlist, Track};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// Every Mac has its own track IDs. Songs matched by name get this Mac's ID
    /// while the quiz is open; this maps it back to the one in the file, so the
    /// same file works on several Macs.
    file_ids: HashMap<String, String>,
    /// The same for the playlist: (ID on this Mac, ID in the file).
    file_playlist: Option<(String, String)>,
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
        Quiz { meta, entries, removed: Vec::new(), file_ids: HashMap::new(), file_playlist: None }
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
        Ok(Quiz { meta, entries, removed, file_ids: HashMap::new(), file_playlist: None })
    }

    pub fn to_file(&self) -> QuizFile {
        let as_in_file = |s: &Song| {
            let mut s = s.clone();
            if let Some(id) = self.file_ids.get(&s.track_id) {
                s.track_id = id.clone();
            }
            s
        };
        let mut rounds: Vec<Round> = vec![Round { title: None, songs: Vec::new() }];
        for e in &self.entries {
            match e {
                Entry::Heading(t) => {
                    if rounds.len() == 1 && rounds[0].title.is_none() && rounds[0].songs.is_empty() {
                        rounds.clear();
                    }
                    rounds.push(Round { title: Some(t.clone()), songs: Vec::new() });
                }
                Entry::Song(s) => rounds.last_mut().unwrap().songs.push(as_in_file(s)),
            }
        }
        let mut meta = self.meta.clone();
        if let Some((here, in_file)) = &self.file_playlist
            && meta.source.playlist_id == *here
        {
            meta.source.playlist_id = in_file.clone();
        }
        QuizFile { rounds, removed: self.removed.iter().map(as_in_file).collect(), ..meta }
    }

    /// Use the playlist with this ID while the quiz is open; the file keeps its own.
    pub fn use_playlist(&mut self, id: &str) {
        let in_file = match self.file_playlist.take() {
            Some((here, in_file)) if here == self.meta.source.playlist_id => in_file,
            _ => self.meta.source.playlist_id.clone(),
        };
        self.file_playlist = Some((id.to_string(), in_file));
        self.meta.source.playlist_id = id.to_string();
    }

    /// Give songs whose track ID isn't in the playlist the ID of the track with
    /// the same title, artist and length (another Mac, a library rebuild).
    /// Returns the changed IDs, old to new.
    pub fn use_library(&mut self, tracks: &[Track]) -> HashMap<String, String> {
        let known: HashSet<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let in_quiz: HashSet<String> = self.songs().chain(&self.removed).map(|s| s.track_id.clone()).collect();
        let mut free: Vec<&Track> = tracks.iter().filter(|t| !in_quiz.contains(&t.id)).collect();
        let mut changed = HashMap::new();
        let entries = self.entries.iter_mut().filter_map(|e| match e {
            Entry::Song(s) => Some(s),
            _ => None,
        });
        for s in entries.chain(&mut self.removed).filter(|s| !known.contains(s.track_id.as_str())) {
            let same = |t: &&Track| {
                t.title.to_lowercase() == s.title.to_lowercase()
                    && t.artist.to_lowercase() == s.artist.to_lowercase()
                    && (t.duration.round() as i64 - s.duration as i64).abs() <= 2
            };
            let Some(i) = free.iter().position(same) else { continue };
            let id = free.remove(i).id.clone();
            let old = std::mem::replace(&mut s.track_id, id.clone());
            let in_file = self.file_ids.remove(&old).unwrap_or_else(|| old.clone());
            if in_file != id {
                self.file_ids.insert(id.clone(), in_file);
            }
            changed.insert(old, id);
        }
        changed
    }

    /// How many songs `use_library` found under another ID than the file's.
    pub fn matched_by_name(&self) -> usize {
        self.songs().filter(|s| self.file_ids.contains_key(&s.track_id)).count()
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

    /// What `sync` would change, after `use_library`. Nothing changes until `sync`.
    pub fn plan_sync(&self, tracks: &[Track]) -> SyncPlan {
        let in_playlist: HashSet<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let in_quiz: HashSet<&str> = self.songs().map(|s| s.track_id.as_str()).collect();
        SyncPlan {
            added: tracks.iter().filter(|t| !in_quiz.contains(t.id.as_str())).cloned().collect(),
            removed: self.songs().filter(|s| !in_playlist.contains(s.track_id.as_str())).cloned().collect(),
        }
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
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
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
    fn same_song_with_another_id_is_used_here_but_not_saved() {
        let mut q = Quiz::new("Q", &playlist(), &[track("1"), track("2"), track("4")]);
        // "2" has another ID here, "4" is another version, "3" is new.
        let mut moved = track("9");
        moved.title = "2".into();
        let mut longer = track("8");
        longer.title = "4".into();
        longer.duration += 5.0;
        let here = [track("1"), moved, longer, track("3")];
        assert_eq!(q.use_library(&here), HashMap::from([("2".to_string(), "9".to_string())]));
        assert!(q.use_library(&here).is_empty());
        assert_eq!(q.matched_by_name(), 1);
        let ids = |q: &Quiz| q.songs().map(|s| s.track_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&q), vec!["1", "9", "4"]);
        let plan = q.plan_sync(&here);
        assert_eq!(plan.added.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["8", "3"]);
        assert_eq!(plan.removed.iter().map(|s| s.track_id.as_str()).collect::<Vec<_>>(), vec!["4"]);
        let in_file: Vec<String> = q.to_file().rounds[0].songs.iter().map(|s| s.track_id.clone()).collect();
        assert_eq!(in_file, vec!["1", "2", "4"]);
        // Back on the first Mac, or on a third, it still maps to the file's ID.
        q.use_library(&[track("1"), track("2"), track("4")]);
        assert_eq!(ids(&q), vec!["1", "2", "4"]);
        assert_eq!(q.to_file().rounds[0].songs[1].track_id, "2");
        // The same for a song that was removed, which comes back with its questions.
        let mut q = Quiz::new("Q", &playlist(), &[track("1"), track("2")]);
        q.sync(&[track("1")]);
        let mut moved = track("9");
        moved.title = "2".into();
        q.use_library(&[track("1"), moved.clone()]);
        assert_eq!(q.sync(&[track("1"), moved]), (vec!["9".to_string()], 0));
        assert!(q.removed.is_empty());
        assert_eq!(q.to_file().rounds[0].songs[1].track_id, "2");
    }

    #[test]
    fn playlist_id_in_the_file_is_kept_until_another_playlist_is_chosen() {
        let mut q = Quiz::new("Q", &playlist(), &[]);
        q.use_playlist("HERE");
        q.use_playlist("THERE");
        assert_eq!(q.meta.source.playlist_id, "THERE");
        assert_eq!(q.to_file().source.playlist_id, "P");
        q.meta.source.playlist_id = "CHOSEN".into();
        assert_eq!(q.to_file().source.playlist_id, "CHOSEN");
        q.use_playlist("X");
        assert_eq!(q.to_file().source.playlist_id, "CHOSEN");
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name_for("Midsommarquiz 2024"), "midsommarquiz-2024.quiz.toml");
        assert_eq!(file_name_for("Jul & Nyår!"), "jul-nyar.quiz.toml");
    }
}
