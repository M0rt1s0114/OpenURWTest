//! Terminal helpers shared by the live progress line and the regular output.
//!
//! The progress line is drawn with a carriage return. Before any other output
//! is printed, the line is cleared so messages never get glued to it.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static LIVE_LINE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn mark_live_line() {
    LIVE_LINE_ACTIVE.store(true, Ordering::SeqCst);
}

/// Erase the live progress line, if one is currently on screen.
pub fn clear_live_line() {
    if LIVE_LINE_ACTIVE.swap(false, Ordering::SeqCst) {
        print!("\r\x1b[2K");
        let _ = io::stdout().flush();
    }
}

/// Finish the live progress line with a newline, if one was drawn.
pub fn finish_live_line() {
    if LIVE_LINE_ACTIVE.swap(false, Ordering::SeqCst) {
        println!();
    }
}

/// Best-effort terminal width in characters.
pub fn width() -> Option<usize> {
    platform::console_width()
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window: Coord,
    }

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleScreenBufferInfo(
            handle: *mut c_void,
            info: *mut ConsoleScreenBufferInfo,
        ) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;

    pub fn console_width() -> Option<usize> {
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if handle.is_null() || handle as isize == -1 {
            return None;
        }
        // SAFETY: the structure is zero-initialised and the handle was checked.
        let mut info: ConsoleScreenBufferInfo = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetConsoleScreenBufferInfo(handle, &mut info) };
        if ok == 0 {
            return None;
        }
        let width = info.size.x;
        (width > 0).then_some(width as usize)
    }
}

#[cfg(unix)]
mod platform {
    use std::os::fd::AsRawFd;

    pub fn console_width() -> Option<usize> {
        // SAFETY: TIOCGWINSZ writes a winsize structure into size.
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        let result =
            unsafe { libc::ioctl(std::io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
        if result != 0 || size.ws_col == 0 {
            return None;
        }
        Some(size.ws_col as usize)
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub fn console_width() -> Option<usize> {
        None
    }
}
