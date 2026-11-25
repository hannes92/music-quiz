# mq — music quiz

Plan a music quiz from a playlist in Music.app, in the terminal. macOS only.

```
cargo run --release                          # home: recent quizzes, n for a new one
cargo run --release -- my.quiz.toml          # curate a quiz
cargo run --release -- run my.quiz.toml      # quiz night (also R on home, F5 in a quiz)
cargo run --release -- check my.quiz.toml    # before the event; --play starts every song muted
```

## Quiz night

The run view is made for a projector: the audience sees the same screen, so
it never shows a song's title or artist before the reveal. It goes through
three parts: listen (each song and its questions), a quick re-run (a short
snippet of each song, moving on by itself) and the reveal (answers shown).
Titled rounds get a card in each part. Keys: space play/pause, → ← next and
previous, r replay, g jump to a song, R jump to the reveal, b blank screen,
+ and - change the text size (0 resets it), h hide the key line, q end. The position is saved next to the quiz, so after
a crash or quit it offers to resume. Keep Music.app on another display.

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
