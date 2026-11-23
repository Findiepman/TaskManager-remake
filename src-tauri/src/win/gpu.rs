//! GPU name, video memory, temperature, fan and core clock without a vendor SDK.
//! DXGI lists the adapters, D3DKMT (the same kernel-thunk interface Task Manager reads)
//! gives the sensors, and the "GPU Adapter Memory" counter gives the VRAM in use.

use windows::Wdk::Graphics::Direct3D::*;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

/// Own copy of D3DKMT_ADAPTER_PERFDATA: the crate declares PowerStateOverride as a Rust bool,
/// and a driver writing any other byte value there would be undefined behaviour.
#[repr(C)]
#[derive(Default)]
struct AdapterPerfData {
    physical_adapter_index: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    temperature: u32, // tenths of a degree C
    power_state_override: u8,
}

pub struct Adapter {
    pub name: String,
    /// "luid_0x00000000_0x0000d1a3" as it appears in PDH instance names (lowercase).
    pub pdh_luid: String,
    pub vram_total: u64,
    handle: u32,
    /// The driver reports a fan (MaxFanRPM > 0). A real 0 RPM then means the fan is stopped
    /// (zero-RPM mode); without a fan sensor 0 means "not reported".
    has_fan: bool,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let c = D3DKMT_CLOSEADAPTER { hAdapter: self.handle };
        unsafe {
            let _ = D3DKMTCloseAdapter(&c);
        }
    }
}

#[derive(Default)]
pub struct Sensors {
    pub temp: Option<f32>,
    pub fan_rpm: Option<u32>,
    pub clock_mhz: Option<u32>,
}

/// The main graphics card: the hardware adapter with the most dedicated video memory.
/// Skips software and virtual display adapters (they report no dedicated memory).
pub fn main_adapter() -> Option<Adapter> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(String, LUID, u64)> = None;
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let mem = d.DedicatedVideoMemory as u64;
            if mem == 0 || best.as_ref().is_some_and(|b| b.2 >= mem) {
                continue;
            }
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            best = Some((String::from_utf16_lossy(&d.Description[..n]).trim().to_string(), d.AdapterLuid, mem));
        }
        let (name, luid, dxgi_mem) = best?;
        let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: luid, hAdapter: 0 };
        if D3DKMTOpenAdapterFromLuid(&mut open).is_err() {
            return None;
        }
        let mut a = Adapter {
            name,
            pdh_luid: format!("luid_0x{:08x}_0x{:08x}", luid.HighPart as u32, luid.LowPart),
            vram_total: dxgi_mem,
            handle: open.hAdapter,
            has_fan: false,
        };
        let mut caps = D3DKMT_ADAPTER_PERFDATACAPS::default();
        a.has_fan = query(a.handle, KMTQAITYPE_ADAPTERPERFDATA_CAPS, &mut caps) && caps.MaxFanRPM > 0;
        let mut seg = D3DKMT_SEGMENTSIZEINFO::default();
        if query(a.handle, KMTQAITYPE_GETSEGMENTSIZE, &mut seg) && seg.DedicatedVideoMemorySize > 0 {
            a.vram_total = seg.DedicatedVideoMemorySize;
        }
        Some(a)
    }
}

fn query<T>(handle: u32, ty: KMTQUERYADAPTERINFOTYPE, data: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: ty,
        pPrivateDriverData: data as *mut T as *mut _,
        PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q).is_ok() }
}

/// None when the adapter handle stopped working (e.g. after a driver reset); reopen it then.
pub fn read_sensors(a: &Adapter) -> Option<Sensors> {
    let mut s = Sensors::default();
    let mut perf = AdapterPerfData::default();
    if !query(a.handle, KMTQAITYPE_ADAPTERPERFDATA, &mut perf) {
        return None;
    }
    // 0 means the driver does not report it.
    if perf.temperature > 0 && perf.temperature < 1500 {
        s.temp = Some(perf.temperature as f32 / 10.0);
    }
    if a.has_fan {
        s.fan_rpm = Some(perf.fan_rpm);
    }
    // Node 0 is the 3D engine; its frequency is the core clock.
    let mut node = D3DKMT_NODE_PERFDATA { NodeOrdinal: 0, PhysicalAdapterIndex: 0, ..Default::default() };
    if query(a.handle, KMTQAITYPE_NODEPERFDATA, &mut node) && node.Frequency > 0 {
        s.clock_mhz = Some((node.Frequency / 1_000_000) as u32);
    }
    Some(s)
}

/// Sums "\GPU Adapter Memory(luid_..._phys_N)\Dedicated Usage" for the adapter.
pub fn vram_used(a: &Adapter, counter: &[(String, f64)]) -> Option<u64> {
    let mut sum = 0.0;
    let mut found = false;
    for (name, v) in counter {
        if name.to_ascii_lowercase().starts_with(&a.pdh_luid) {
            sum += v;
            found = true;
        }
    }
    found.then_some(sum as u64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn perfdata_layout_matches_windows() {
        // Measured on Windows 11 x64: D3DKMT_ADAPTER_PERFDATA is 64 bytes.
        assert_eq!(std::mem::size_of::<super::AdapterPerfData>(), 64);
    }
}
