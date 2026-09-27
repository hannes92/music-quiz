use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType};

/// All colours in one place, so a light or projector theme is a later swap.
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub clip: Color,
    pub warn: Color,
    pub dim: Color,
    pub faint: Color,
}

pub const DARK: Theme = Theme {
    bg: Color::Rgb(0x0d, 0x0d, 0x12),
    fg: Color::Rgb(0xe8, 0xe8, 0xf0),
    accent: Color::Rgb(0xff, 0x3d, 0x8b),
    clip: Color::Rgb(0x3d, 0xe0, 0xff),
    warn: Color::Rgb(0xff, 0xb0, 0x3d),
    dim: Color::Rgb(0x80, 0x80, 0x92),
    faint: Color::Rgb(0x40, 0x40, 0x4c),
};

impl Theme {
    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }
    pub fn dim(&self) -> Style {
        Style::new().fg(self.dim)
    }
    pub fn faint(&self) -> Style {
        Style::new().fg(self.faint)
    }
    pub fn accent(&self) -> Style {
        Style::new().fg(self.accent)
    }
    pub fn clip(&self) -> Style {
        Style::new().fg(self.clip)
    }
    pub fn warn(&self) -> Style {
        Style::new().fg(self.warn)
    }
    pub fn selected(&self) -> Style {
        Style::new().fg(self.fg).bg(Color::Rgb(0x2a, 0x12, 0x20)).add_modifier(Modifier::BOLD)
    }
    pub fn key(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    /// Like `frame`, but the border eases between dim and accent for 200 ms
    /// after focus moved at `since`.
    pub fn frame_fading<'a>(&self, title: impl Into<ratatui::text::Line<'a>>, focused: bool, since: std::time::Instant) -> Block<'a> {
        let p = (since.elapsed().as_secs_f64() / 0.2).min(1.0);
        let p = if focused { p } else { 1.0 - p };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(blend(self.faint, self.accent, p)))
            .title(title)
            .style(self.base())
    }

    /// Rounded frame; the focused pane gets the accent border.
    pub fn frame<'a>(&self, title: impl Into<ratatui::text::Line<'a>>, focused: bool) -> Block<'a> {
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(if focused { self.accent() } else { self.faint() })
            .title(title)
            .style(self.base())
    }
}

fn blend(a: Color, b: Color, p: f64) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * p).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => if p >= 0.5 { b } else { a },
    }
}

/// Decorative 8-bar equalizer, driven by the UI tick rather than real audio.
pub fn equalizer(tick: u64) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    (0..8u64)
        .map(|i| {
            let t = tick as f64 * 0.35 + i as f64 * 1.7;
            let v = ((t.sin() + (t * 0.53 + i as f64).sin()) * 0.25 + 0.5).clamp(0.0, 0.999);
            BARS[(v * 8.0) as usize]
        })
        .collect()
}

pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Symbols, so terminals with a Nerd Font get icons and others plain text.
pub struct Glyphs {
    pub cursor: &'static str,
    pub playing: &'static str,
    pub check: &'static str,
    pub removed: &'static str,
    pub sync: &'static str,
    pub on: &'static str,
    pub off: &'static str,
    pub error: &'static str,
    pub stopped: &'static str,
}

pub const UNICODE: Glyphs = Glyphs {
    cursor: "▶",
    playing: "♪",
    check: "✓",
    removed: "✗",
    sync: "⟳",
    on: "●",
    off: "○",
    error: "✗",
    stopped: "■",
};

pub const NERD: Glyphs = Glyphs {
    cursor: "\u{f04b}",
    playing: "\u{f001}",
    check: "\u{f00c}",
    removed: "\u{f00d}",
    sync: "\u{f021}",
    on: "\u{f111}",
    off: "\u{f10c}",
    error: "\u{f071}",
    stopped: "\u{f04d}",
};

pub const ASCII: Glyphs = Glyphs {
    cursor: ">",
    playing: "*",
    check: "+",
    removed: "x",
    sync: "~",
    on: "*",
    off: "o",
    error: "!",
    stopped: "-",
};
