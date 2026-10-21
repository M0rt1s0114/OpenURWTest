use std::io::{self, IsTerminal};

use crate::terminal;
use crate::types::ColorMode;
use crate::volume::{VolumeInfo, VolumeKind};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy)]
pub struct Console {
    color: bool,
    terminal: bool,
}

impl Console {
    pub fn new(mode: ColorMode) -> Self {
        let terminal = io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let color = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => terminal && !no_color,
        };
        #[cfg(windows)]
        if color {
            enable_virtual_terminal();
        }
        Self { color, terminal }
    }

    pub fn progress_enabled(&self) -> bool {
        self.terminal
    }

    fn paint(&self, code: &str, message: &str) -> String {
        if self.color {
            format!("{code}{message}{RESET}")
        } else {
            message.to_string()
        }
    }

    fn line(&self, message: &str) {
        terminal::clear_live_line();
        println!("{message}");
    }

    pub fn ok(&self, message: &str) {
        self.line(&self.paint(GREEN, message));
    }

    pub fn fail(&self, message: &str) {
        self.line(&self.paint(RED, message));
    }

    pub fn warn(&self, message: &str) {
        self.line(&self.paint(YELLOW, message));
    }

    pub fn info(&self, message: &str) {
        self.line(&self.paint(CYAN, message));
    }

    pub fn print_volumes(&self, volumes: &[VolumeInfo]) {
        terminal::clear_live_line();
        println!(
            "{:<3} {:<8} {:<10} {:<8} {:>10} {:>10}  Device",
            "#", "Volume", "Kind", "FS", "Total", "Free"
        );
        println!("{}", "-".repeat(96));
        for (index, volume) in volumes.iter().enumerate() {
            println!(
                "{:<3} {:<8} {:<10} {:<8} {:>10} {:>10}  {}",
                index + 1,
                volume.mount_point.display(),
                volume.kind.label(),
                volume.fs,
                format_bytes(volume.total_bytes),
                format_bytes(volume.free_bytes),
                volume.device_name()
            );
        }
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

impl VolumeKind {
    pub fn label(self) -> &'static str {
        match self {
            VolumeKind::Removable => "Removable",
            VolumeKind::Fixed => "Fixed",
            VolumeKind::Network => "Network",
            VolumeKind::Other => "Other",
        }
    }
}
#[cfg(windows)]
fn enable_virtual_terminal() {
    use std::ffi::c_void;

    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }

    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if handle.is_null() || handle as isize == -1 {
        return;
    }
    let mut mode = 0u32;
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return;
    }
    unsafe {
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }
}
