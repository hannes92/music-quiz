//! `mq`: plan a music quiz from a Music.app playlist.
//!
//!   mq               home: recent quizzes, or a new one from a playlist
//!   mq <file>        open a quiz (also `mq edit <file>`)

mod app;
mod config;
mod curation;
mod home;
mod model;
mod music;
mod picker;
mod questions;
mod settings;
mod theme;
mod worker;

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = match args.as_slice() {
        [] => None,
        [f] | [_, f] if args[0] != "-h" && args[0] != "--help" => Some(PathBuf::from(f)),
        _ => {
            eprintln!("usage: mq [edit] [<file>.quiz.toml]");
            std::process::exit(2);
        }
    };
    app::run(file)
}
