//! On-disk test data format.
//!
//! The tool writes a fixed sequence of test files to the root of the target
//! volume: HUT00001.BIN, HUT00002.BIN, ...
//!
//! Each file holds at most 2048 blocks of 1 MiB (2 GiB). Every 32-bit word of
//! the payload comes from a deterministic generator, so verification can
//! rebuild the expected contents without storing any state outside the volume.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Payload block size.
pub const BLOCK_SIZE: u64 = 1024 * 1024;

/// Maximum number of blocks per test file (2 GiB).
pub const BLOCKS_PER_FILE: u64 = 2048;

/// Stop writing once the free space drops to this value.
pub const MIN_FREE_SPACE: u64 = 1024 * 1024;

/// Maximum size of a single test file.
pub const MAX_FILE_SIZE: u64 = BLOCK_SIZE * BLOCKS_PER_FILE;

/// Name of the test file that holds a given 1-based index.
pub fn test_file_name(index: u32) -> String {
    format!("HUT{index:05}.BIN")
}

/// Parse a test file name and return its 1-based index.
pub fn parse_test_file_name(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    let digits = upper.strip_prefix("HUT")?.strip_suffix(".BIN")?;
    if digits.len() != 5 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index: u32 = digits.parse().ok()?;
    (index >= 1).then_some(index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestFile {
    pub index: u32,
    pub size: u64,
}

/// Collect the contiguous test-file sequence HUT00001.BIN, HUT00002.BIN, ...
pub fn scan_test_files(directory: &Path) -> io::Result<Vec<TestFile>> {
    let mut found = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(index) = parse_test_file_name(&name) {
            found.insert(index, entry.metadata()?.len());
        }
    }

    let mut files = Vec::new();
    let mut index = 1u32;
    while let Some(size) = found.get(&index) {
        files.push(TestFile { index, size: *size });
        index += 1;
    }
    Ok(files)
}

/// Total number of 1 MiB blocks covered by the test files.
///
/// The last file may be shorter than 2 GiB; earlier files are expected to be
/// full. A partial trailing block is ignored.
pub fn total_blocks(files: &[TestFile]) -> Option<u64> {
    let last = files.last()?;
    if last.size == 0 || last.size > MAX_FILE_SIZE {
        return None;
    }
    if files[..files.len() - 1]
        .iter()
        .any(|file| file.size != MAX_FILE_SIZE)
    {
        return None;
    }
    Some(last.size / BLOCK_SIZE + (files.len() as u64 - 1) * BLOCKS_PER_FILE)
}

/// Path of the file that contains the given block index.
pub fn block_path(directory: &Path, block_index: u64) -> PathBuf {
    let file_index = (block_index / BLOCKS_PER_FILE + 1) as u32;
    directory.join(test_file_name(file_index))
}

/// Remove the contiguous test-file sequence. Returns the number of files removed.
pub fn delete_test_files(directory: &Path) -> io::Result<u32> {
    let mut removed = 0u32;
    let mut index = 1u32;
    loop {
        let path = directory.join(test_file_name(index));
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
        index += 1;
    }
    Ok(removed)
}

/// Deterministic payload generator.
///
/// The generator is seeded with fixed constants; each pass regenerates the
/// same sequence from the beginning, which is what makes verification possible.
pub struct Prng {
    state: [u32; 4],
}

impl Default for Prng {
    fn default() -> Self {
        Self::new()
    }
}

impl Prng {
    pub fn new() -> Self {
        Self {
            state: [0x159A_55E5, 0x1F12_3BB5, 0x0549_1333, 0xDCA3_45EA],
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let t = self.state[0] ^ (self.state[0] << 11);
        let w = self.state[3] ^ (((self.state[3] >> 11) ^ t) >> 8) ^ t;
        self.state[0] = self.state[1];
        self.state[1] = self.state[2];
        self.state[2] = self.state[3];
        self.state[3] = w;
        w
    }

    /// Fill a buffer with the next block of payload (little-endian u32 words).
    pub fn fill_block(&mut self, buffer: &mut [u8]) {
        for chunk in buffer.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&self.next_u32().to_le_bytes());
        }
    }

    /// Advance the generator by one block without producing output.
    pub fn skip_block(&mut self) {
        for _ in 0..(BLOCK_SIZE / 4) {
            self.next_u32();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_matches_reference_sequence() {
        let mut prng = Prng::new();
        let expected = [
            0x1B51_16E6,
            0x9510_49AA,
            0xD88D_00B0,
            0x1EC7_825E,
            0x8DB2_4146,
        ];
        for value in expected {
            assert_eq!(prng.next_u32(), value);
        }
    }

    #[test]
    fn generator_produces_expected_bytes() {
        let mut prng = Prng::new();
        let mut buffer = vec![0u8; 16];
        prng.fill_block(&mut buffer);
        assert_eq!(
            buffer,
            vec![
                0xE6, 0x16, 0x51, 0x1B, 0xAA, 0x49, 0x10, 0x95, 0xB0, 0x00, 0x8D, 0xD8, 0x5E, 0x82,
                0xC7, 0x1E
            ]
        );
    }

    #[test]
    fn generator_is_deterministic_across_blocks() {
        let mut a = Prng::new();
        let mut b = Prng::new();
        let mut first = vec![0u8; 64];
        let mut second = vec![0u8; 64];
        a.fill_block(&mut first);
        b.fill_block(&mut second);
        assert_eq!(first, second);
        a.skip_block();
        b.skip_block();
        let mut third = vec![0u8; 64];
        let mut fourth = vec![0u8; 64];
        a.fill_block(&mut third);
        b.fill_block(&mut fourth);
        assert_eq!(third, fourth);
        assert_ne!(first, third);
    }

    #[test]
    fn file_names_round_trip() {
        assert_eq!(test_file_name(1), "HUT00001.BIN");
        assert_eq!(test_file_name(42), "HUT00042.BIN");
        assert_eq!(parse_test_file_name("HUT00042.BIN"), Some(42));
        assert_eq!(parse_test_file_name("hut00042.bin"), Some(42));
        assert_eq!(parse_test_file_name("HUT42.BIN"), None);
        assert_eq!(parse_test_file_name("HUT00000.BIN"), None);
        assert_eq!(parse_test_file_name("OTHER00001.BIN"), None);
    }

    #[test]
    fn total_blocks_uses_last_file_size() {
        let files = [
            TestFile {
                index: 1,
                size: MAX_FILE_SIZE,
            },
            TestFile {
                index: 2,
                size: 4 * BLOCK_SIZE,
            },
        ];
        assert_eq!(total_blocks(&files), Some(BLOCKS_PER_FILE + 4));
        assert_eq!(total_blocks(&[]), None);
        assert_eq!(total_blocks(&[TestFile { index: 1, size: 0 }]), None);
    }

    #[test]
    fn total_blocks_rejects_invalid_layouts() {
        let short_middle = [
            TestFile {
                index: 1,
                size: 4 * BLOCK_SIZE,
            },
            TestFile {
                index: 2,
                size: 2 * BLOCK_SIZE,
            },
        ];
        assert_eq!(total_blocks(&short_middle), None);
        let oversized_last = [TestFile {
            index: 1,
            size: MAX_FILE_SIZE + BLOCK_SIZE,
        }];
        assert_eq!(total_blocks(&oversized_last), None);
    }

    #[test]
    fn delete_removes_only_the_contiguous_sequence() {
        let directory =
            std::env::temp_dir().join(format!("urwtest-rs-format-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create temp dir");
        std::fs::write(directory.join("HUT00001.BIN"), b"1").expect("write");
        std::fs::write(directory.join("HUT00002.BIN"), b"2").expect("write");
        std::fs::write(directory.join("keep.txt"), b"x").expect("write");

        let removed = delete_test_files(&directory).expect("delete");
        assert_eq!(removed, 2);
        assert!(!directory.join("HUT00001.BIN").exists());
        assert!(!directory.join("HUT00002.BIN").exists());
        assert!(directory.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn block_paths_split_at_2048_blocks() {
        let directory = Path::new("X:");
        assert_eq!(block_path(directory, 0), directory.join("HUT00001.BIN"));
        assert_eq!(block_path(directory, 2047), directory.join("HUT00001.BIN"));
        assert_eq!(block_path(directory, 2048), directory.join("HUT00002.BIN"));
        assert_eq!(block_path(directory, 4096), directory.join("HUT00003.BIN"));
    }
}
