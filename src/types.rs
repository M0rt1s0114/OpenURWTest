use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ProgressMode {
    /// Live line on a terminal, otherwise no progress output.
    #[default]
    Auto,
    /// Always use the live single-line progress display.
    Live,
    /// Print a periodic plain-text progress line (useful for logs).
    Plain,
    /// No progress output.
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum MouseMode {
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TestKind {
    #[default]
    Quality,
    Speed,
}

impl TestKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Quality => "质量测试",
            Self::Speed => "速度测试",
        }
    }
}
