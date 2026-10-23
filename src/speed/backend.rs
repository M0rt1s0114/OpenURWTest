//! Platform-neutral positional I/O for speed tests.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::io::AlignedBlock;

pub trait BenchFile {
    fn read_at(&mut self, offset: u64, buffer: &mut AlignedBlock) -> io::Result<usize>;
    fn write_at(&mut self, offset: u64, buffer: &AlignedBlock) -> io::Result<usize>;
}

struct FileBackend {
    file: File,
}

impl BenchFile for FileBackend {
    fn read_at(&mut self, offset: u64, buffer: &mut AlignedBlock) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buffer.as_mut_slice())?;
        Ok(buffer.as_slice().len())
    }

    fn write_at(&mut self, offset: u64, buffer: &AlignedBlock) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buffer.as_slice())?;
        Ok(buffer.as_slice().len())
    }
}

#[cfg(windows)]
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;

/// Open (or create) the benchmark file for read/write.
pub fn open_bench_file(path: &Path, direct: bool) -> io::Result<Box<dyn BenchFile>> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        if direct {
            options.custom_flags(FILE_FLAG_NO_BUFFERING);
        }
    }

    #[cfg(unix)]
    if direct {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECT);
    }

    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(Box::new(FileBackend { file }))
}

/// Remove benchmark files left behind by dead processes. Files still held open by
/// a live worker are skipped: Windows denies deletion without FILE_SHARE_DELETE,
/// and Unix holds a shared flock.
pub fn cleanup_stale_bench_files(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(crate::speed::suite::bench_file_prefix())
            || !name.ends_with(".DAT")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        let path = entry.path();
        #[cfg(windows)]
        {
            let _ = std::fs::remove_file(&path);
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            else {
                continue;
            };
            let locked =
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
            if locked {
                let _ = std::fs::remove_file(&path);
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Create or resize the benchmark file before opening it for direct I/O.
pub fn prepare_file(path: &Path, size: u64) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.set_len(size)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn direct_io_round_trip_preserves_data() {
        let dir =
            std::env::temp_dir().join(format!("urwtest-rs-direct-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("bench.dat");
        prepare_file(&path, 8192).expect("prepare");
        let mut file = open_bench_file(&path, true).expect("open direct");
        let mut buffer = AlignedBlock::new(4096);
        buffer.as_mut_slice().fill(0x5A);
        file.write_at(0, &buffer).expect("write");
        buffer.as_mut_slice().fill(0);
        file.read_at(0, &mut buffer).expect("read");
        assert!(buffer.as_slice().iter().all(|byte| *byte == 0x5A));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn direct_io_rejects_unaligned_offset() {
        let dir = std::env::temp_dir().join(format!("urwtest-rs-direct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("bench.dat");
        prepare_file(&path, 8192).expect("prepare");
        let mut file = open_bench_file(&path, true).expect("open direct");
        let mut buffer = AlignedBlock::new(4096);
        let result = file.read_at(1, &mut buffer);
        assert!(result.is_err(), "direct IO accepted an unaligned offset");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn positional_read_write_round_trip() {
        let dir = std::env::temp_dir().join(format!("urwtest-rs-backend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("bench.dat");
        prepare_file(&path, 8192).expect("prepare");

        let mut file = open_bench_file(&path, false).expect("open");
        let mut buffer = AlignedBlock::new(4096);
        buffer.as_mut_slice().fill(0x5A);
        file.write_at(4096, &buffer).expect("write");
        buffer.as_mut_slice().fill(0);
        file.read_at(4096, &mut buffer).expect("read");
        assert!(buffer.as_slice().iter().all(|byte| *byte == 0x5A));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
