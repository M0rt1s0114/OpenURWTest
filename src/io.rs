//! Block I/O helpers.
//!
//! Test data is written in 1 MiB blocks. On Windows the files are opened with
//! FILE_FLAG_NO_BUFFERING so that reads and writes reach the device instead of
//! the system cache; that mode requires a sector-aligned buffer, which is what
//! AlignedBlock provides.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Alignment required by unbuffered I/O on common devices.
pub const BUFFER_ALIGNMENT: usize = 4096;

pub struct AlignedBlock {
    ptr: std::ptr::NonNull<u8>,
    len: usize,
}

impl AlignedBlock {
    pub fn new(len: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(len, BUFFER_ALIGNMENT)
            .expect("invalid block layout");
        // SAFETY: the layout is non-zero; zeroed allocation is defined for this layout.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        let ptr = std::ptr::NonNull::new(ptr).expect("failed to allocate I/O buffer");
        Self { ptr, len }
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the pointer is valid for len bytes for the lifetime of self.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: the pointer is valid and uniquely borrowed for len bytes.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for AlignedBlock {
    fn drop(&mut self) {
        let layout = std::alloc::Layout::from_size_align(self.len, BUFFER_ALIGNMENT)
            .expect("invalid block layout");
        // SAFETY: allocated with the same layout in new.
        unsafe { std::alloc::dealloc(self.ptr.as_ptr(), layout) };
    }
}

#[cfg(windows)]
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;

#[cfg(windows)]
fn options(read: bool, write: bool) -> OpenOptions {
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    if read {
        options.read(true);
    }
    if write {
        options.write(true).create(true).truncate(true);
    }
    options.custom_flags(FILE_FLAG_NO_BUFFERING);
    options
}

#[cfg(not(windows))]
fn options(read: bool, write: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    if read {
        options.read(true);
    }
    if write {
        options.write(true).create(true).truncate(true);
    }
    options
}

/// Open a test file for writing.
pub fn open_writer(path: &Path) -> io::Result<File> {
    options(false, true).open(path)
}

/// Open a test file for reading.
pub fn open_reader(path: &Path) -> io::Result<File> {
    options(true, false).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_block_is_aligned_and_writable() {
        let mut block = AlignedBlock::new(1024 * 1024);
        assert_eq!(block.as_mut_slice().as_ptr() as usize % BUFFER_ALIGNMENT, 0);
        block.as_mut_slice()[0] = 0xAA;
        block.as_mut_slice()[1024 * 1024 - 1] = 0x55;
        assert_eq!(block.as_slice()[0], 0xAA);
        assert_eq!(block.as_slice()[1024 * 1024 - 1], 0x55);
    }
}
