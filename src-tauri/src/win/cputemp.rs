//! AMD Ryzen CPU temperature through the PawnIO driver (the signed driver LibreHardwareMonitor
//! uses). We talk to the driver with DeviceIoControl, load the signed AMDFamily17 module and
//! read the Tctl register over SMN, exactly like LibreHardwareMonitor and Linux k10temp.

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::System::IO::DeviceIoControl;

const MODULE: &[u8] = include_bytes!("../../resources/pawnio/AMDFamily17.bin");
// From PawnIO's pawnio_um.h: CTL_CODE(0xA1B2, func, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_PIO_LOAD_BINARY: u32 = 0xA1B2_2084;
const IOCTL_PIO_EXECUTE_FN: u32 = 0xA1B2_2104;
const THM_TCON_CUR_TMP: u64 = 0x0005_9800;

pub struct CpuTemp {
    dev: HANDLE,
    pci_mutex: HANDLE,
}

unsafe impl Send for CpuTemp {}

impl Drop for CpuTemp {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.dev);
            if !self.pci_mutex.is_invalid() {
                let _ = CloseHandle(self.pci_mutex);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Status {
    /// Reading works.
    Ok,
    /// Not an AMD Zen CPU; there is no supported way to read the temperature.
    Unsupported,
    /// The PawnIO driver is not installed.
    NoDriver,
    /// The driver is there but the module or read failed.
    Failed,
}

/// AMD family 17h-1Ah (Zen 1 to Zen 5), the range the AMDFamily17 module accepts.
pub fn is_supported_cpu() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        let v = __cpuid(0);
        let vendor: Vec<u8> = [v.ebx, v.edx, v.ecx].iter().flat_map(|r| r.to_le_bytes()).collect();
        if &vendor != b"AuthenticAMD" {
            return false;
        }
        let f = __cpuid(1).eax;
        let base = (f >> 8) & 0xF;
        let family = if base == 0xF { base + ((f >> 20) & 0xFF) } else { base };
        (0x17..=0x1A).contains(&family)
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

/// Tctl in degrees C from THM_TCON_CUR_TMP (LibreHardwareMonitor Amd17Cpu.cs, Linux k10temp):
/// bits 31..21 in 1/8 degree steps; the "range select" bit or both Tj-select bits mean -49 C.
/// No name-based offset applies to Zen 3 and newer, so Tctl equals Tdie there.
pub fn tctl_from_raw(raw: u32) -> f32 {
    let mut t = ((raw >> 21) * 125) as f32 / 1000.0;
    if raw & 0x8_0000 != 0 || raw & 0x3_0000 == 0x3_0000 {
        t -= 49.0;
    }
    t
}

impl CpuTemp {
    pub fn open() -> Result<Self, Status> {
        if !is_supported_cpu() {
            return Err(Status::Unsupported);
        }
        unsafe {
            let dev = CreateFileW(
                w!(r"\\?\GLOBALROOT\Device\PawnIO"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .map_err(|_| Status::NoDriver)?;
            // Shared with HWiNFO, LibreHardwareMonitor and others so PCI config reads never interleave.
            let pci_mutex = CreateMutexW(None, false, w!(r"Global\Access_PCI")).unwrap_or_default();
            let me = CpuTemp { dev, pci_mutex };
            let mut ret = 0u32;
            DeviceIoControl(me.dev, IOCTL_PIO_LOAD_BINARY, Some(MODULE.as_ptr() as _), MODULE.len() as u32, None, 0, Some(&mut ret), None)
                .map_err(|_| Status::Failed)?;
            me.read().ok_or(Status::Failed)?;
            Ok(me)
        }
    }

    fn read_smn(&self, addr: u64) -> Option<u32> {
        let mut input = [0u8; 32 + 8];
        let name = b"ioctl_read_smn";
        input[..name.len()].copy_from_slice(name);
        input[32..40].copy_from_slice(&addr.to_le_bytes());
        let mut out = [0u64; 1];
        let mut ret = 0u32;
        unsafe {
            let locked = if self.pci_mutex.is_invalid() {
                false
            } else {
                let w = WaitForSingleObject(self.pci_mutex, 50);
                if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
                    return None;
                }
                true
            };
            let r = DeviceIoControl(
                self.dev,
                IOCTL_PIO_EXECUTE_FN,
                Some(input.as_ptr() as _),
                input.len() as u32,
                Some(out.as_mut_ptr() as _),
                8,
                Some(&mut ret),
                None,
            );
            if locked {
                let _ = ReleaseMutex(self.pci_mutex);
            }
            (r.is_ok() && ret >= 8).then_some(out[0] as u32)
        }
    }

    pub fn read(&self) -> Option<f32> {
        let t = tctl_from_raw(self.read_smn(THM_TCON_CUR_TMP)?);
        // A value outside this range means the read went wrong; show nothing rather than nonsense.
        (5.0..=125.0).contains(&t).then_some(t)
    }
}

/// True when the PawnIO driver is installed (registry entry from its official installer).
pub fn driver_installed() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO")
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::tctl_from_raw;

    #[test]
    fn tctl_math() {
        // 60.125 C: raw bits 31..21 = 481
        assert_eq!(tctl_from_raw(481 << 21), 60.125);
        // range-select bit set: subtract 49
        assert_eq!(tctl_from_raw((481 << 21) | 0x8_0000), 11.125);
        // both Tj-select bits set: also -49
        assert_eq!(tctl_from_raw((800 << 21) | 0x3_0000), 51.0);
        // only one Tj-select bit: no offset
        assert_eq!(tctl_from_raw((800 << 21) | 0x1_0000), 100.0);
    }
}
