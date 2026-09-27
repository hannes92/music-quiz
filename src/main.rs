//! `mq`: plan and run a music quiz from a Music.app playlist.
//!
//!   mq                         home: recent quizzes, or a new one from a playlist
//!   mq [edit] <file>           open a quiz for curation
//!   mq run <file>              run it on quiz night
//!   mq check <file> [--play]   check Music.app and every song before the event

mod app;
mod check;
mod config;
mod curation;
mod home;
mod model;
mod music;
mod picker;
mod questions;
mod run;
mod settings;
mod theme;
mod worker;

use app::Start;
use std::path::PathBuf;

const USAGE: &str = "usage: mq [edit <file> | run <file> | check <file> [--play] | <file>]";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let start = match args.as_slice() {
        [] => Start::Home,
        ["-h" | "--help", ..] => {
            println!("{USAGE}");
            return Ok(());
        }
        ["check", f, rest @ ..] => {
            let ok = check::cli(&PathBuf::from(f), rest.contains(&"--play"))?;
            std::process::exit(if ok { 0 } else { 1 });
        }
        ["run", f] => Start::Run(PathBuf::from(f)),
        ["edit", f] | [f] => Start::Edit(PathBuf::from(f)),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    app::run(start)
}
