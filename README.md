# mq — music quiz

Plan a music quiz from a playlist in Music.app, in the terminal. macOS only.

```
cargo run --release              # home: recent quizzes, n for a new one
cargo run --release -- my.quiz.toml
```

The quiz is a normal Music.app playlist. `mq` keeps what Music.app doesn't
know about in `<name>.quiz.toml`: the running order, rounds, clips and
questions. Press `?` on any screen for its keys.

The first time, macOS asks whether your terminal may control Music. That is
the only permission needed; Music.app is never brought to the front.

## Settings

`~/.config/mq/config.toml`:

```toml
glyphs = "nerd"   # Nerd Font icons; "ascii" for plain text; default Unicode
```

`MQ_GLYPHS=nerd` does the same for one run. Recent quizzes are listed in
`~/.config/mq/recent.toml`.

## Tests

`cargo test` runs the offline tests. The ones marked `#[ignore]` talk to
Music.app (one plays a few seconds of audio, one creates and deletes a
scratch playlist): `cargo test -- --ignored --nocapture`.
