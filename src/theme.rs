//! TUI color theme.
//!
//! One accent hue carries the chrome, content stays neutral, and extra hues are
//! reserved for state (success, warning, danger). The palette degrades to ANSI
//! colors and then to monochrome, so the interface never depends on truecolor.

use ratatui::style::{Color, Modifier, Style};

use crate::types::ColorMode;

pub struct Palette {
    pub accent: Color,
    pub accent_soft: Color,
    pub text: Color,
    pub dim: Color,
    pub border: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub colored: bool,
}

impl Palette {
    pub fn new(mode: ColorMode) -> Self {
        let enabled = match mode {
            ColorMode::Never => false,
            ColorMode::Always => true,
            ColorMode::Auto => std::env::var_os("NO_COLOR").is_none(),
        };
        if !enabled {
            return Self {
                accent: Color::Reset,
                accent_soft: Color::Reset,
                text: Color::Reset,
                dim: Color::Reset,
                border: Color::Reset,
                success: Color::Reset,
                warning: Color::Reset,
                danger: Color::Reset,
                colored: false,
            };
        }
        if truecolor(mode) {
            Self {
                accent: Color::Rgb(255, 121, 168),
                accent_soft: Color::Rgb(200, 92, 130),
                text: Color::Reset,
                dim: Color::Rgb(154, 154, 162),
                border: Color::Rgb(58, 58, 64),
                success: Color::Rgb(91, 214, 162),
                warning: Color::Rgb(232, 180, 90),
                danger: Color::Rgb(255, 107, 107),
                colored: true,
            }
        } else {
            Self {
                accent: Color::Magenta,
                accent_soft: Color::LightMagenta,
                text: Color::Reset,
                dim: Color::Gray,
                border: Color::DarkGray,
                success: Color::Green,
                warning: Color::Yellow,
                danger: Color::Red,
                colored: true,
            }
        }
    }
}

fn truecolor(mode: ColorMode) -> bool {
    if mode == ColorMode::Always {
        return true;
    }
    if let Ok(value) = std::env::var("COLORTERM")
        && (value.contains("truecolor") || value.contains("24bit"))
    {
        return true;
    }
    if std::env::var_os("WT_SESSION").is_some() {
        return true;
    }
    std::env::var("TERM_PROGRAM")
        .map(|value| value == "vscode" || value == "WezTerm")
        .unwrap_or(false)
}

pub struct Theme {
    pub palette: Palette,
}

impl Theme {
    pub fn new(mode: ColorMode) -> Self {
        Self {
            palette: Palette::new(mode),
        }
    }

    fn apply(&self, color: Color) -> Style {
        if self.palette.colored {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    pub fn banner(&self) -> Style {
        self.apply(self.palette.accent).add_modifier(Modifier::BOLD)
    }

    #[allow(dead_code)]
    pub fn title(&self) -> Style {
        self.apply(self.palette.accent)
    }

    pub fn border(&self) -> Style {
        self.apply(self.palette.border)
    }

    pub fn text(&self) -> Style {
        self.apply(self.palette.text)
    }

    pub fn dim(&self) -> Style {
        let style = self.apply(self.palette.dim);
        if self.palette.colored {
            style
        } else {
            style.add_modifier(Modifier::DIM)
        }
    }

    pub fn selected(&self) -> Style {
        self.apply(self.palette.accent).add_modifier(Modifier::BOLD)
    }

    pub fn key(&self) -> Style {
        self.apply(self.palette.accent)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    pub fn success(&self) -> Style {
        self.apply(self.palette.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn warning(&self) -> Style {
        self.apply(self.palette.warning)
    }

    pub fn danger(&self) -> Style {
        self.apply(self.palette.danger).add_modifier(Modifier::BOLD)
    }

    pub fn gauge_write(&self) -> Style {
        self.apply(self.palette.accent)
    }

    pub fn gauge_verify(&self) -> Style {
        self.apply(self.palette.accent_soft)
    }

    pub fn speed_color(&self) -> Color {
        if self.palette.colored {
            self.palette.accent
        } else {
            Color::Reset
        }
    }

    #[allow(dead_code)]
    pub fn chart_speed(&self) -> Style {
        self.apply(self.palette.accent)
    }

    #[allow(dead_code)]
    pub fn chart_baseline(&self) -> Style {
        self.apply(self.palette.dim)
    }

    #[allow(dead_code)]
    pub fn chart_threshold(&self) -> Style {
        self.apply(self.palette.warning)
    }
}
