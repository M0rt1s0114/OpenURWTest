use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::{VolumeInfo, VolumeKind};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const DEVICE_DESCRIPTOR_SIZE: usize = 36;

unsafe extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        free_bytes_available_to_caller: *mut u64,
        total_number_of_bytes: *mut u64,
        total_number_of_free_bytes: *mut u64,
    ) -> i32;
    fn GetVolumeInformationW(
        root_path_name: *const u16,
        volume_name_buffer: *mut u16,
        volume_name_size: u32,
        volume_serial_number: *mut u32,
        maximum_component_length: *mut u32,
        file_system_flags: *mut u32,
        file_system_name_buffer: *mut u16,
        file_system_name_size: u32,
    ) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> *mut c_void;
    fn DeviceIoControl(
        device: *mut c_void,
        control_code: u32,
        in_buffer: *const c_void,
        in_buffer_size: u32,
        out_buffer: *mut c_void,
        out_buffer_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(object: *mut c_void) -> i32;
}

#[derive(Debug, Default)]
struct DeviceDescriptor {
    model: String,
    revision: String,
    removable: bool,
}

pub fn list_volumes() -> Vec<VolumeInfo> {
    let drive_mask = unsafe { GetLogicalDrives() };
    let mut result = Vec::new();

    for index in 2..26u32 {
        if drive_mask & (1 << index) == 0 {
            continue;
        }

        let letter = (b'A' + index as u8) as char;
        let root = format!("{letter}:\\");
        let root_wide = wide(&root);

        let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        let kind = match drive_type {
            DRIVE_REMOVABLE => VolumeKind::Removable,
            DRIVE_FIXED => VolumeKind::Fixed,
            DRIVE_REMOTE => VolumeKind::Network,
            DRIVE_CDROM | DRIVE_RAMDISK => continue,
            _ => VolumeKind::Other,
        };

        let mut free_to_caller = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free_bytes = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                root_wide.as_ptr(),
                &mut free_to_caller,
                &mut total_bytes,
                &mut total_free_bytes,
            )
        };
        if ok == 0 {
            continue;
        }

        let mut label_buffer = vec![0u16; 261];
        let mut fs_buffer = vec![0u16; 261];
        let mut serial = 0u32;
        let mut max_component = 0u32;
        let mut flags = 0u32;
        let info_ok = unsafe {
            GetVolumeInformationW(
                root_wide.as_ptr(),
                label_buffer.as_mut_ptr(),
                label_buffer.len() as u32,
                &mut serial,
                &mut max_component,
                &mut flags,
                fs_buffer.as_mut_ptr(),
                fs_buffer.len() as u32,
            )
        };

        let label = if info_ok != 0 {
            let label = String::from_utf16_lossy(&label_buffer);
            let label = label.trim_end_matches('\0').trim().to_string();
            (!label.is_empty()).then_some(label)
        } else {
            None
        };

        let fs = if info_ok != 0 {
            String::from_utf16_lossy(&fs_buffer)
                .trim_end_matches('\0')
                .to_string()
        } else {
            String::new()
        };

        let descriptor = query_device_descriptor(letter).unwrap_or_default();
        let removable = if descriptor.removable {
            true
        } else {
            matches!(kind, VolumeKind::Removable)
        };

        result.push(VolumeInfo {
            mount_point: PathBuf::from(root),
            fs,
            total_bytes,
            free_bytes: free_to_caller,
            kind,
            label,
            model: descriptor.model,
            revision: descriptor.revision,
            removable,
        });
    }

    result
}

pub fn available_space(path: &Path) -> Result<u64> {
    let path_text = path.to_string_lossy();
    let path_wide = wide(&path_text);
    let mut free_to_caller = 0u64;
    let mut total_bytes = 0u64;
    let mut total_free_bytes = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            path_wide.as_ptr(),
            &mut free_to_caller,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };
    if ok == 0 {
        return Err(anyhow!("GetDiskFreeSpaceExW failed for {}", path.display()));
    }
    Ok(free_to_caller)
}

fn query_device_descriptor(letter: char) -> Option<DeviceDescriptor> {
    let device_path = wide(&format!("\\\\.\\{letter}:"));
    let handle = unsafe {
        CreateFileW(
            device_path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return None;
    }

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional: [u8; 4],
    }

    let query = StoragePropertyQuery {
        property_id: 0,
        query_type: 0,
        additional: [0; 4],
    };
    let mut buffer = vec![0u8; 1024];
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const StoragePropertyQuery as *const c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            buffer.as_mut_ptr() as *mut c_void,
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 || (returned as usize) < DEVICE_DESCRIPTOR_SIZE {
        return None;
    }

    let bytes = &buffer[..returned as usize];
    let read_u32 = |offset: usize| -> Option<u32> {
        bytes
            .get(offset..offset + 4)
            .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    };

    let vendor = read_u32(12).unwrap_or(0);
    let product = read_u32(16).unwrap_or(0);
    let revision = read_u32(20).unwrap_or(0);

    let model = {
        let vendor = read_string(bytes, vendor);
        let product = read_string(bytes, product);
        format!("{vendor} {product}").trim().to_string()
    };

    Some(DeviceDescriptor {
        model,
        revision: read_string(bytes, revision),
        removable: bytes.get(10).copied().unwrap_or(0) != 0,
    })
}

/// Read a NUL-terminated printable string at an offset inside a device descriptor.
fn read_string(bytes: &[u8], offset: u32) -> String {
    let offset = offset as usize;
    if offset < DEVICE_DESCRIPTOR_SIZE || offset >= bytes.len() {
        return String::new();
    }
    let mut result = String::new();
    for byte in &bytes[offset..] {
        if *byte == 0 {
            break;
        }
        result.push(if (0x20..0x7F).contains(byte) {
            *byte as char
        } else {
            '?'
        });
    }
    result
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
