//! Shared formatting and display helpers for the terminal UI.

use std::path::{Path, PathBuf};

use crate::volume;

pub(super) fn export_directory() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("urwtest-export")
        .join(stamp.to_string())
}

pub(super) fn export_directory_for(target: &Path) -> PathBuf {
    let directory = export_directory();
    let target_volume = volume::find_volume_containing(target);
    let export_volume = volume::find_volume_containing(&directory);
    if target_volume.as_ref().map(|volume| &volume.mount_point)
        == export_volume.as_ref().map(|volume| &volume.mount_point)
    {
        let name = directory
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "export".into());
        std::env::temp_dir().join("urwtest-export").join(name)
    } else {
        directory
    }
}

pub(super) fn display_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    text.width()
}

pub(super) fn pad_display(text: &str, width: usize) -> String {
    let used = display_width(text);
    let mut result = text.to_string();
    if used < width {
        result.push_str(&" ".repeat(width - used));
    }
    result
}

pub(super) fn trimmed_mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.len() > 2 {
        sorted = sorted[1..sorted.len() - 1].to_vec();
    }
    sorted.iter().sum::<f64>() / sorted.len() as f64
}

pub(super) fn format_axis_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.1}T", value / TIB)
    } else if value >= GIB {
        format!("{:.1}G", value / GIB)
    } else if value >= MIB {
        format!("{:.1}M", value / MIB)
    } else if value >= KIB {
        format!("{:.1}K", value / KIB)
    } else {
        bytes.to_string()
    }
}

pub(super) fn format_seconds(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

pub(super) fn format_millis(millis: u64) -> String {
    if millis >= 1000 {
        format!("{:.1}s", millis as f64 / 1000.0)
    } else {
        format!("{millis}ms")
    }
}

pub(super) fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}
