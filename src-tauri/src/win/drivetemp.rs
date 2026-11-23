//! Drive temperatures via IOCTL_STORAGE_QUERY_PROPERTY / StorageDeviceTemperatureProperty.
//! NVMe drives answer; most SATA drives behind the standard AHCI driver do not (shown as n/a).

use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const STORAGE_DEVICE_TEMPERATURE_PROPERTY: i32 = 52;

#[repr(C)]
struct PropertyQuery {
    property_id: i32,
    query_type: i32,
    additional: [u8; 4],
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open(path: &str) -> Option<Handle> {
    let w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    // Access 0 is enough for both queries and does not need the drive to be unlocked.
    unsafe {
        CreateFileW(PCWSTR(w.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None)
            .ok()
            .map(Handle)
    }
}

/// Physical disk numbers a volume lives on (one for a normal partition).
fn disks_of(letter: &str) -> Vec<u32> {
    let Some(h) = open(&format!(r"\\.\{}", letter)) else { return Vec::new() };
    let mut buf = [0u8; 4 + 4 + 24 * 8];
    let mut ret = 0u32;
    let ok = unsafe { DeviceIoControl(h.0, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, 0, Some(buf.as_mut_ptr() as _), buf.len() as u32, Some(&mut ret), None) };
    if ok.is_err() || ret < 8 {
        return Vec::new();
    }
    // VOLUME_DISK_EXTENTS: u32 count, 4 bytes padding, then DISK_EXTENT { u32 disk, pad, i64, i64 }
    let n = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    (0..n.min(8)).map(|i| u32::from_le_bytes(buf[8 + i * 24..12 + i * 24].try_into().unwrap())).collect()
}

/// Temperature of PhysicalDriveN in degrees C, if the drive reports it.
fn disk_temp(n: u32) -> Option<f32> {
    let h = open(&format!(r"\\.\PhysicalDrive{n}"))?;
    let q = PropertyQuery { property_id: STORAGE_DEVICE_TEMPERATURE_PROPERTY, query_type: 0, additional: [0; 4] };
    let mut buf = [0u8; 1024];
    let mut ret = 0u32;
    unsafe {
        DeviceIoControl(
            h.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&q as *const _ as _),
            12,
            Some(buf.as_mut_ptr() as _),
            buf.len() as u32,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    parse_descriptor(&buf[..ret as usize])
}

/// STORAGE_TEMPERATURE_DATA_DESCRIPTOR: header 24 bytes (InfoCount u16 at offset 12), then
/// 16-byte STORAGE_TEMPERATURE_INFO entries with Temperature i16 at offset 2. Entry 0 is the
/// drive's composite temperature.
pub fn parse_descriptor(b: &[u8]) -> Option<f32> {
    if b.len() < 24 + 16 {
        return None;
    }
    let count = u16::from_le_bytes([b[12], b[13]]);
    if count == 0 {
        return None;
    }
    let t = i16::from_le_bytes([b[24 + 2], b[24 + 3]]);
    (t > 0 && t < 150).then_some(t as f32)
}

/// "C:" -> degrees for every given drive letter whose disk reports a temperature.
pub fn read_all(letters: &[String]) -> HashMap<String, f32> {
    let mut by_disk: HashMap<u32, Option<f32>> = HashMap::new();
    let mut out = HashMap::new();
    for l in letters {
        for d in disks_of(l) {
            let t = *by_disk.entry(d).or_insert_with(|| disk_temp(d));
            if let Some(t) = t {
                out.insert(l.to_uppercase(), t);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptor() {
        let mut b = vec![0u8; 24 + 32];
        b[12] = 2; // InfoCount
        b[26..28].copy_from_slice(&38i16.to_le_bytes());
        assert_eq!(super::parse_descriptor(&b), Some(38.0));
        b[12] = 0;
        assert_eq!(super::parse_descriptor(&b), None);
        assert_eq!(super::parse_descriptor(&b[..10]), None);
    }
}
