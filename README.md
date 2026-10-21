# OpenURWTest _(urwtest-rs)_

English | [简体中文](README_CN.md)

[![standard-readme compliant](https://img.shields.io/badge/readme%20style-standard-brightgreen.svg?style=flat-square)](https://github.com/RichardLitt/standard-readme)

Mounted-volume quality and speed test tool for Windows

OpenURWTest provides two independent test lines: **quality tests** that fill a volume with deterministic data and verify it, and **speed tests** that measure CrystalDiskMark-style sequential and 4K random write-then-read suites, IOPS, and latency. It offers a terminal UI and a command-line interface, multi-drive concurrent runs, temperature and health display, and CSV/JSON/Excel/SVG reports. The Cargo package and executable remain `urwtest-rs`.

## Table of Contents

- [Security](#security)
- [Install](#install)
- [Usage](#usage)
  - [TUI](#tui)
  - [Quality test](#quality-test)
  - [Speed test](#speed-test)
  - [Multi-drive runs](#multi-drive-runs)
  - [Mouse](#mouse)
  - [CLI](#cli)
- [Test data](#test-data)
- [Reports](#reports)
- [Platforms](#platforms)
- [License](#license)

## Security

- A quality test writes until less than 1 MiB of free space remains, so the target volume will be filled.
- A speed test creates a benchmark file of the configured size (4 GiB by default) and performs direct I/O on it. The TUI removes the file after the run; the CLI keeps it unless the target directory is cleaned manually.
- Only mounted volume paths are supported. Raw devices and partition tables are never accessed.
- Writing to a volume containing Windows requires a red warning and a second confirmation dialog. Other fixed disks show a yellow warning. Removable disks show no extra warning.
- The speed test writes a benchmark file to a directory; it does not fill the volume.
- Pressing Esc or Ctrl+C during a quality run opens a confirmation dialog first. Confirming cancellation stops the run and keeps the data written so far.
- Run the tool only on media whose contents you can afford to overwrite.

## Install

### Dependencies

- Windows 10 or 11, x86_64
- Rust 1.96 or newer (only when building from source)

Build the executable:

```sh
cargo build --release
```

The binary is written to `target\release\urwtest-rs.exe`.

## Usage

Start without arguments in a terminal to open the TUI:

```sh
urwtest-rs
```

### TUI

The home screen lists mounted volumes. Each row shows a checkbox, the mount point, free space, and the device model.

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move the cursor |
| `Space` | Toggle the current volume |
| `a` / `n` | Select all / clear selection |
| `m` | Switch between quality and speed test |
| `Enter` / `w` | Open the settings dialog |
| `r` | Rescan volumes |
| `q` | Quit |
| `F2` | Toggle mouse capture (turn it off to select terminal text) |

The details panel shows the selected volume, free space, current temperature, health status, selection count, and test type. The bottom row contains clickable buttons for mode, settings, start, and quit.

### Quality test

Quality settings:

| Option | Description |
| --- | --- |
| Test mode | Write and verify, write only, or verify existing data |
| Passes | 1-99, default 1; fixed to 1 for verify-existing mode |
| Continue after errors | Default no; available for write-and-verify with more than one pass |
| Delete test files after success | Default no; only after a successful run |
| Export report automatically | Default yes; writes to a timestamped directory under `./urwtest-export` |

After the settings, a summary is shown. Removable disks start after this confirmation, fixed disks show a yellow warning, and a volume containing Windows shows a red warning followed by a second confirmation dialog.

The run screen puts the target, temperature/health, and a live speed/ETA line at the top, then the progress bar, a compact three-line log, and a large speed chart with Y-axis labels below the log. The chart's X axis is the written (or verified) byte position: for quality runs it spans the free space at start, which equals the capacity for an empty volume; for speed runs it spans the benchmark file size. The result screen shows the status (PASS, FAIL, INCOMPLETE), a speed chart by write position or time, slowdown events, and the slowdown thresholds. `x` switches the chart axis, `t` switches the threshold mode, `[` and `]` adjust the threshold, and `e` exports the report.

The default slowdown analysis uses a threshold of 80% or 100 MB/s, a 150 ms dip filter, a 90% recovery line, and a 3 s recovery window.

Pressing Esc or Ctrl+C during a quality run opens a confirmation dialog before cancelling.

### Speed test

The speed test follows CrystalDiskMark's four presets:

| Preset | Block size | Queue depth | Threads |
| --- | --- | --- | --- |
| SEQ1M Q8T1 | 1 MiB | 8 | 1 |
| SEQ1M Q1T1 | 1 MiB | 1 | 1 |
| RND4K Q32T1 | 4 KiB | 32 | 1 |
| RND4K Q1T1 | 4 KiB | 1 | 1 |

The settings dialog is a multi-select list, so one run can execute several presets in order. All four are selected by default. The test file size defaults to **4 GiB** and can be set to 1, 2, 4, or 8 GiB.

Q8 and Q32 are implemented as 8 or 32 synchronous QD1 worker lanes, not as a single-thread native asynchronous queue. Use the numbers for relative comparison; they are not directly interchangeable with CrystalDiskMark's queue model.

Throughput is phase wall-clock throughput: it includes payload generation and, for the read phase, verification work. Per-I/O latency excludes the comparison itself. On very fast media the CPU cost of generating and checking the payload can still influence the reported throughput.

Set `URWTEST_ASCII=1` to use ASCII spinners and chart markers on terminals that lack braille/box-drawing glyphs.

Every selected preset is a complete write-then-read flow:

1. the whole file is written (measured);
2. the file is read back and every block is verified (read measured; comparison happens outside the I/O timer).

Sequential presets stop the whole suite for that drive on the first read or verification error, mark the result red, and log the written offset where corruption was found. The option "continue on sequential errors" keeps going instead. RND4K read or verification errors are logged but do not stop the suite.

All presets draw a live write/read line chart while running. The active phase uses a braille spinner; running QC tabs use a rotating quadrant and running SC tabs use a mini equalizer. An error state pulses once and then stays red. After the suite finishes, the result screen overlays the sequential write/read curves and shows the four 4K bars together; random bars use a trimmed mean that drops the highest and lowest samples. The progress bar marks the error position in red.

### Multi-drive runs

- `Space` checks the volumes to test; `Enter` applies the settings to all checked volumes.
- Each drive runs in its own thread with its own cancellation token and its own tab.
- On the run screen, `Tab` cycles tabs and `1`-`9` jump directly to a tab.
- `Esc` and Ctrl+C open the cancel prompt; `Enter` or `1` stops the current drive; `2` or `S` stops all drives when more than one drive is running; `3`/`Esc` returns.
- When only one drive is running, the prompt shows only "Enter stop current" and "Esc back", without numbers or a "stop all" entry.
- When all drives finish, a multi-drive result table is shown. Press `e` to export the multi-drive report; automatic export is controlled by the quality settings.
- Speed tests also run concurrently on the selected drives.

### Mouse

- Mouse capture is enabled by default; `F2` toggles it.
- Left click: move the cursor and toggle checkboxes on the home screen; click settings fields; click tabs; click the bottom footers for start, stop, cancel, back, and export.
- Wheel: scroll the volume list, move between settings fields, switch drive tabs, or adjust the result threshold.
- Hover highlights buttons and rows.
- Only the left button, the wheel, and `F2` are part of the mouse design.

### CLI

```sh
# List mounted volumes
urwtest-rs list

# Show model, capacity, disk number, temperature, and health
urwtest-rs info --target C:\

# Write and verify
urwtest-rs run --target E:\

# Write only
urwtest-rs run --target E:\ --no-verify

# Two passes, continue after verification errors
urwtest-rs run --target E:\ --passes 2 --continue-on-error

# Verify existing test data
urwtest-rs verify --target E:\

# Default CDM suite: all four presets, 4 GiB file
urwtest-rs speed --target E:\ --size 4g

# Only the two sequential presets, 2 GiB file
urwtest-rs speed --target E:\ --preset seq1m-q8t1 --preset seq1m-q1t1 --size 2g

# 4K random presets, continue after sequential errors
urwtest-rs speed --target E:\ --preset rnd4k-q32t1 --preset rnd4k-q1t1 --size 1g --ignore-errors

# Run the suite on two drives concurrently; JSON array output
urwtest-rs speed --target E:\ --target D:\ --size 4g --json

```

| Option | Description |
| --- | --- |
| `--target <PATH>` | Volume root or directory; repeat for multiple speed targets |
| `--passes <N>` | Number of quality passes, default 1 |
| `--no-verify` | Quality test: write only, skip verification |
| `--continue-on-error` | Continue with the next pass after verification errors |
| `--progress auto\|live\|plain\|off` | Progress output mode |
| `--no-tui` | Use the plain text interface even in a terminal |
| `--export <DIR>` | Export CSV/JSON/Excel/SVG reports after a quality run |
| `--color auto\|always\|never` | Control terminal colors |
| `--preset <NAME>` | CDM preset: `seq1m-q8t1`, `seq1m-q1t1`, `rnd4k-q32t1`, `rnd4k-q1t1`, or `all`; repeat or comma-separate, default all four |
| `--size <SIZE>` | Test file size, default `4g`; 1, 2, 4, and 8 GiB are the usual choices |
| `--ignore-errors` | Continue sequential presets after the first read/verification error |
| `--seed <N>` | Override the workload seed |

When stdout is not a terminal, or with `--no-tui`, the quality test uses the text interface.

## Test data

### Quality test

- File names: `HUT00001.BIN`, `HUT00002.BIN`, ... (five-digit index)
- Block size: 1 MiB; at most 2048 blocks per file (2 GiB)
- Total blocks: `last file size / 1 MiB + (file count - 1) * 2048`
- Payload: deterministic sequence with a fixed seed; verification rebuilds the sequence and compares it byte by byte
- Verification only recognizes the contiguous test files above; other files are not removed

### Speed test

- Benchmark file: `BENCH0001.DAT` in the target directory
- Payload: deterministic non-zero pattern
- Direct I/O is used by default; block sizes and offsets are aligned for unbuffered access
- The TUI removes the benchmark file after a run; the CLI keeps it

## Reports

A single quality run exports:

| File | Content |
| --- | --- |
| `samples.csv` | Resampled speed series: position, time, speed, baseline, threshold |
| `events.csv` | Slowdown events |
| `report.json` | Run statistics, analysis parameters, and export result |
| `report.xlsx` | Excel workbook: Summary, Samples, and Events sheets |
| `speed_position.svg` | Speed by write position |
| `speed_time.svg` | Speed over time |

A multi-drive quality or speed run exports to `urwtest-export/<timestamp>/multi/`:

| File | Content |
| --- | --- |
| `multi_summary.csv` | One row per drive, and one row per preset for speed runs: mode/preset, status, bytes, errors, temperature, health, speed metrics |
| `multi_report.json` | The same rows as JSON |
| `multi_report.xlsx` | Excel workbook with a Summary sheet |

## Platforms

- Windows 10/11 x86_64 is the current target.
- The Linux code path is kept compile-clean with `cargo check --target x86_64-unknown-linux-gnu --all-targets`.
- Temperature and health are read from Windows storage properties; some USB bridges or removable devices do not expose temperature and show `unknown`.

## License

[GPL-3.0-or-later](LICENSE)