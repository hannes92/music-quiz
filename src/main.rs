//! `mq`: plan a music quiz from a Music.app playlist.
//!
//!   mq               pick a playlist and start a new quiz
//!   mq <file>        open a quiz (also `mq edit <file>`)

mod app;
mod curation;
mod model;
mod music;
mod picker;
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
