//! User settings and the recent-quizzes list, in ~/.config/mq/.

use crate::theme::{ASCII, Glyphs, NERD, UNICODE};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// "nerd" for Nerd Font icons, "ascii" for plain text, otherwise Unicode symbols.
    #[serde(default)]
    pub glyphs: String,
}

fn dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("mq")
}

impl Config {
    pub fn load() -> Self {
        let mut c: Config = std::fs::read_to_string(dir().join("config.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        if let Ok(g) = std::env::var("MQ_GLYPHS") {
            c.glyphs = g;
        }
        c
    }

    pub fn glyphs(&self) -> &'static Glyphs {
        match self.glyphs.as_str() {
            "nerd" => &NERD,
            "ascii" => &ASCII,
            _ => &UNICODE,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Recent {
    #[serde(default)]
    quizzes: Vec<PathBuf>,
}

/// Recently opened quiz files that still exist, newest first.
pub fn recent() -> Vec<PathBuf> {
    let r: Recent = std::fs::read_to_string(dir().join("recent.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    r.quizzes.into_iter().filter(|p| p.exists()).collect()
}

pub fn remember(path: &Path) {
    let Ok(path) = std::fs::canonicalize(path) else { return };
    let mut list = recent();
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(20);
    let _ = std::fs::create_dir_all(dir());
    if let Ok(text) = toml::to_string(&Recent { quizzes: list }) {
        let _ = std::fs::write(dir().join("recent.toml"), text);
    }
}
