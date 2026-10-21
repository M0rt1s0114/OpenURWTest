//! Drive temperature/health probing.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthStatus {
    #[default]
    Unknown,
    Ok,
    Warning,
    Critical,
}

impl HealthStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Ok => "OK",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TemperatureSample {
    pub elapsed_ms: u64,
    pub temperature_c: i32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DriveHealth {
    pub temperature_c: Option<i32>,
    pub warning_c: Option<i16>,
    pub critical_c: Option<i16>,
    pub status: HealthStatus,
    pub disk_number: Option<u32>,
}

pub fn read_health(mount: &Path) -> DriveHealth {
    imp::read_health(mount)
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{DriveHealth, HealthStatus};

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const GENERIC_READ: u32 = 0x8000_0000;
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
    const STORAGE_DEVICE_TEMPERATURE_PROPERTY: u32 = 52;

    unsafe extern "system" {
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

    pub fn read_health(mount: &Path) -> DriveHealth {
        let Some(letter) = volume_letter(mount) else {
            return DriveHealth::default();
        };
        let device = format!("\\\\.\\{letter}:");
        let Some(handle) = open(&device) else {
            return DriveHealth::default();
        };

        let mut health = DriveHealth {
            disk_number: query_disk_number(handle),
            ..DriveHealth::default()
        };
        let mut temperature = query_temperature(handle, STORAGE_DEVICE_TEMPERATURE_PROPERTY);
        if temperature.is_none() {
            temperature = query_temperature(handle, 51);
        }
        unsafe {
            CloseHandle(handle);
        }
        if temperature.is_none()
            && let Some(disk_number) = health.disk_number
            && let Some(physical) = open(&format!("\\\\.\\PhysicalDrive{disk_number}"))
        {
            temperature = query_temperature(physical, STORAGE_DEVICE_TEMPERATURE_PROPERTY)
                .or_else(|| query_temperature(physical, 51));
            unsafe {
                CloseHandle(physical);
            }
        }
        if let Some((value, warning, critical)) = temperature {
            health.temperature_c = Some(value);
            health.warning_c = warning;
            health.critical_c = critical;
            health.status = classify(value, warning, critical);
        }
        health
    }

    fn volume_letter(mount: &Path) -> Option<char> {
        mount
            .to_string_lossy()
            .chars()
            .next()
            .filter(|letter| letter.is_ascii_alphabetic())
    }

    fn open(device: &str) -> Option<*mut c_void> {
        let wide: Vec<u16> = OsStr::new(device)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        for access in [0, GENERIC_READ] {
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if !handle.is_null() && handle as isize != -1 {
                return Some(handle);
            }
        }
        None
    }

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional: [u8; 4],
    }

    fn query_property(handle: *mut c_void, property_id: u32) -> Option<Vec<u8>> {
        let query = StoragePropertyQuery {
            property_id,
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
        if ok == 0 || returned == 0 {
            return None;
        }
        buffer.truncate(returned as usize);
        Some(buffer)
    }

    fn query_temperature(
        handle: *mut c_void,
        property_id: u32,
    ) -> Option<(i32, Option<i16>, Option<i16>)> {
        let buffer = query_property(handle, property_id)?;
        parse_temperature_descriptor(&buffer)
    }

    fn query_disk_number(handle: *mut c_void) -> Option<u32> {
        let mut buffer = vec![0u8; 1024];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                std::ptr::null(),
                0,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || (returned as usize) < 12 {
            return None;
        }
        let count = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        if count == 0 || (returned as usize) < 8 + 12 {
            return None;
        }
        Some(u32::from_le_bytes([
            buffer[8], buffer[9], buffer[10], buffer[11],
        ]))
    }

    fn parse_temperature_descriptor(buffer: &[u8]) -> Option<(i32, Option<i16>, Option<i16>)> {
        if buffer.len() < 24 {
            return None;
        }
        let size = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
        let critical = i16::from_le_bytes([buffer[8], buffer[9]]);
        let warning = i16::from_le_bytes([buffer[10], buffer[11]]);
        let info_count = u16::from_le_bytes([buffer[12], buffer[13]]);
        let descriptor_size = size.max(24);
        if info_count == 0 || buffer.len() < descriptor_size || buffer.len() < 28 {
            return None;
        }
        let raw = i16::from_le_bytes([buffer[26], buffer[27]]);
        let temperature = if raw > 200 {
            raw as i32 - 273
        } else {
            raw as i32
        };
        Some((
            temperature,
            (warning > 0).then_some(warning),
            (critical > 0).then_some(critical),
        ))
    }

    fn classify(temperature: i32, warning: Option<i16>, critical: Option<i16>) -> HealthStatus {
        if let Some(critical) = critical
            && temperature >= critical as i32
        {
            return HealthStatus::Critical;
        }
        if let Some(warning) = warning
            && temperature >= warning as i32
        {
            return HealthStatus::Warning;
        }
        HealthStatus::Ok
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn temperature_sample_keeps_elapsed_and_value() {
            let sample = crate::health::TemperatureSample {
                elapsed_ms: 1234,
                temperature_c: 42,
            };
            assert_eq!(sample.elapsed_ms, 1234);
            assert_eq!(sample.temperature_c, 42);
        }

        #[test]
        fn parses_temperature_descriptor() {
            let mut buffer = vec![0u8; 40];
            buffer[4..8].copy_from_slice(&24u32.to_le_bytes());
            buffer[8..10].copy_from_slice(&85i16.to_le_bytes());
            buffer[10..12].copy_from_slice(&70i16.to_le_bytes());
            buffer[12..14].copy_from_slice(&1u16.to_le_bytes());
            buffer[26..28].copy_from_slice(&41i16.to_le_bytes());
            let (temperature, warning, critical) =
                parse_temperature_descriptor(&buffer).expect("parse");
            assert_eq!(temperature, 41);
            assert_eq!(warning, Some(70));
            assert_eq!(critical, Some(85));
            assert_eq!(classify(41, warning, critical), HealthStatus::Ok);
            assert_eq!(classify(80, warning, critical), HealthStatus::Warning);
            assert_eq!(classify(90, warning, critical), HealthStatus::Critical);
        }

        #[test]
        fn parses_descriptor_with_full_size_field() {
            let mut buffer = vec![0u8; 72];
            buffer[4..8].copy_from_slice(&72u32.to_le_bytes());
            buffer[8..10].copy_from_slice(&87i16.to_le_bytes());
            buffer[10..12].copy_from_slice(&86i16.to_le_bytes());
            buffer[12..14].copy_from_slice(&3u16.to_le_bytes());
            buffer[26..28].copy_from_slice(&56i16.to_le_bytes());
            let (temperature, warning, critical) =
                parse_temperature_descriptor(&buffer).expect("parse");
            assert_eq!(temperature, 56);
            assert_eq!(warning, Some(86));
            assert_eq!(critical, Some(87));
        }

        #[test]
        fn converts_kelvin_values() {
            let mut buffer = vec![0u8; 40];
            buffer[4..8].copy_from_slice(&24u32.to_le_bytes());
            buffer[12..14].copy_from_slice(&1u16.to_le_bytes());
            buffer[26..28].copy_from_slice(&(273i16 + 35).to_le_bytes());
            let (temperature, _, _) = parse_temperature_descriptor(&buffer).expect("parse");
            assert_eq!(temperature, 35);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use super::DriveHealth;

    pub fn read_health(_mount: &Path) -> DriveHealth {
        DriveHealth::default()
    }
}
