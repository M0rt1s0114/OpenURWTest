use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum VolumeKind {
    Removable,
    Fixed,
    Network,
    Other,
}

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub mount_point: PathBuf,
    pub fs: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub kind: VolumeKind,
    pub label: Option<String>,
    /// Vendor/product string reported by the device, when available.
    pub model: String,
    /// Firmware revision reported by the device, when available.
    pub revision: String,
    /// Whether the device reports itself as removable media.
    pub removable: bool,
}

impl VolumeInfo {
    pub fn display_name(&self) -> String {
        match &self.label {
            Some(label) if !label.is_empty() => {
                format!("{} ({label})", self.mount_point.display())
            }
            _ => self.mount_point.display().to_string(),
        }
    }

    pub fn device_name(&self) -> String {
        let model = self.model.trim();
        let revision = self.revision.trim();
        match (model.is_empty(), revision.is_empty()) {
            (true, _) => "-".to_string(),
            (false, true) => model.to_string(),
            (false, false) => format!("{model}  {revision}"),
        }
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{available_space, list_volumes};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{available_space, list_volumes};

#[cfg(not(any(windows, target_os = "linux")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "linux")))]
pub use unsupported::{available_space, list_volumes};

/// Normalize a user-supplied volume path.
///
/// Windows accepts E: and turns it into E:\\; Unix paths are left mostly
/// untouched and canonicalized by find_volume.
pub fn normalize_target_path(target: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = target.to_string_lossy();
        if text.len() == 2 && text.ends_with(':') {
            return PathBuf::from(format!("{text}\\"));
        }
    }
    target.to_path_buf()
}

/// Find the mounted volume whose root is exactly target.
pub fn find_volume(target: &Path) -> Option<VolumeInfo> {
    let target = normalize_target_path(target);
    let target = normalize_existing_path(&target);
    list_volumes()
        .into_iter()
        .find(|volume| normalize_existing_path(&volume.mount_point) == target)
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Return free space for the mounted volume containing target.
pub fn free_space(target: &Path) -> Result<u64> {
    let target = normalize_target_path(target);
    if !target.exists() {
        bail!("target does not exist: {}", target.display());
    }
    available_space(&target)
}
