//! Colors. Two true-color themes (dark, light) with subtle chrome, and a
//! 16-color theme that uses the terminal's own palette and background for
//! terminals without true color (picked automatically via `COLORTERM`).

use crate::format::Category;
use crate::settings::ThemeName;
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent_fg: Color,
    pub border: Color,
    pub header_bg: Color,
    pub cursor_bg: Color,
    pub cursor_fg: Color,
    pub cursor_inactive_bg: Color,
    pub selected: Color,
    pub stripe_bg: Color,
    pub fresh_bg: Color,
    pub popup_bg: Color,
    pub dir: Color,
    pub archive: Color,
    pub image: Color,
    pub audio: Color,
    pub video: Color,
    pub code: Color,
    pub doc: Color,
    pub exec: Color,
    pub link: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    /// True color themes can afford background tints (stripes, gauges).
    pub rich: bool,
}

fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub fn truecolor_supported() -> bool {
    std::env::var("COLORTERM").map(|v| v.contains("truecolor") || v.contains("24bit")).unwrap_or(false)
}

impl Theme {
    pub fn resolve(name: ThemeName) -> Theme {
        match name {
            ThemeName::Dark => Theme::dark(),
            ThemeName::Light => Theme::light(),
            ThemeName::Terminal => Theme::terminal(),
            ThemeName::Auto if truecolor_supported() => Theme::dark(),
            ThemeName::Auto => Theme::terminal(),
        }
    }

    pub fn dark() -> Theme {
        Theme {
            bg: rgb(0x181a1f),
            fg: rgb(0xd5d8df),
            dim: rgb(0x8a92a2),
            faint: rgb(0x535a67),
            accent: rgb(0x6cb6ff),
            accent_fg: rgb(0x0d1117),
            border: rgb(0x343a46),
            header_bg: rgb(0x21242b),
            cursor_bg: rgb(0x2c4a70),
            cursor_fg: rgb(0xffffff),
            cursor_inactive_bg: rgb(0x2a2e37),
            selected: rgb(0xffc66d),
            stripe_bg: rgb(0x1d2026),
            fresh_bg: rgb(0x23452d),
            popup_bg: rgb(0x20232a),
            dir: rgb(0x7aa2f7),
            archive: rgb(0xe0af68),
            image: rgb(0xbb9af7),
            audio: rgb(0x73daca),
            video: rgb(0xf7768e),
            code: rgb(0x9ece6a),
            doc: rgb(0xc8cfe6),
            exec: rgb(0xff9e64),
            link: rgb(0x7dcfff),
            ok: rgb(0x7ecf86),
            warn: rgb(0xe5c07b),
            err: rgb(0xf47067),
            keyword: rgb(0xc792ea),
            string: rgb(0xa5d6a7),
            comment: rgb(0x6a7384),
            number: rgb(0xf78c6c),
            rich: true,
        }
    }

    pub fn light() -> Theme {
        Theme {
            bg: rgb(0xfbfbfc),
            fg: rgb(0x24292f),
            dim: rgb(0x57606a),
            faint: rgb(0x9aa3ad),
            accent: rgb(0x0969da),
            accent_fg: rgb(0xffffff),
            border: rgb(0xd0d7de),
            header_bg: rgb(0xf0f2f5),
            cursor_bg: rgb(0xcfe3ff),
            cursor_fg: rgb(0x0b1b33),
            cursor_inactive_bg: rgb(0xe6e9ee),
            selected: rgb(0xb35900),
            stripe_bg: rgb(0xf4f5f7),
            fresh_bg: rgb(0xd5f5dc),
            popup_bg: rgb(0xffffff),
            dir: rgb(0x0550ae),
            archive: rgb(0x9a6700),
            image: rgb(0x8250df),
            audio: rgb(0x1a7f37),
            video: rgb(0xcf222e),
            code: rgb(0x116329),
            doc: rgb(0x24292f),
            exec: rgb(0xbc4c00),
            link: rgb(0x0a7ea4),
            ok: rgb(0x1a7f37),
            warn: rgb(0x9a6700),
            err: rgb(0xcf222e),
            keyword: rgb(0x8250df),
            string: rgb(0x0a3069),
            comment: rgb(0x6e7781),
            number: rgb(0x953800),
            rich: true,
        }
    }

    pub fn terminal() -> Theme {
        Theme {
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::Gray,
            faint: Color::DarkGray,
            accent: Color::Cyan,
            accent_fg: Color::Black,
            border: Color::DarkGray,
            header_bg: Color::Reset,
            cursor_bg: Color::Blue,
            cursor_fg: Color::White,
            cursor_inactive_bg: Color::DarkGray,
            selected: Color::Yellow,
            stripe_bg: Color::Reset,
            fresh_bg: Color::Green,
            popup_bg: Color::Reset,
            dir: Color::LightBlue,
            archive: Color::Yellow,
            image: Color::Magenta,
            audio: Color::Cyan,
            video: Color::LightRed,
            code: Color::Green,
            doc: Color::Reset,
            exec: Color::LightRed,
            link: Color::LightCyan,
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
            keyword: Color::Magenta,
            string: Color::Green,
            comment: Color::DarkGray,
            number: Color::LightRed,
            rich: false,
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn dim(&self) -> Style {
        Style::default().fg(self.dim)
    }

    pub fn faint(&self) -> Style {
        Style::default().fg(self.faint)
    }

    pub fn accent(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn category(&self, c: Category) -> Color {
        match c {
            Category::Dir => self.dir,
            Category::Archive => self.archive,
            Category::Image => self.image,
            Category::Audio => self.audio,
            Category::Video => self.video,
            Category::Code => self.code,
            Category::Document => self.doc,
            Category::Executable => self.exec,
            Category::Symlink => self.link,
            Category::Other => self.fg,
        }
    }

    /// Finder's tag colors.
    pub fn tag(&self, name: &str) -> Color {
        match name {
            "Red" => if self.rich { rgb(0xff5f57) } else { Color::Red },
            "Orange" => if self.rich { rgb(0xff9f0a) } else { Color::LightRed },
            "Yellow" => if self.rich { rgb(0xffd60a) } else { Color::Yellow },
            "Green" => if self.rich { rgb(0x32d74b) } else { Color::Green },
            "Blue" => if self.rich { rgb(0x0a84ff) } else { Color::Blue },
            "Purple" => if self.rich { rgb(0xbf5af2) } else { Color::Magenta },
            "Gray" => if self.rich { rgb(0x98989d) } else { Color::Gray },
            _ => self.dim,
        }
    }
}
