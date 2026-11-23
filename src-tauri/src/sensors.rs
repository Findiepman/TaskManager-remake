//! Hardware sensors: CPU/GPU/drive temperatures, GPU fan, clock and VRAM, per-app network.
//! Each source is optional; a missing one shows as "not available" in the UI.

use crate::sampler::GpuInfo;
use crate::win::{cputemp, drivetemp, gpu, netetw, pdh};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Readings {
    pub cpu_temp: Option<f32>,
    pub gpu: Option<GpuInfo>,
    /// "C:" -> degrees
    pub drive_temps: HashMap<String, f32>,
}

/// A handle that failed this many reads in a row is dropped and reopened (the driver can reset
/// after sleep or a GPU driver timeout, which leaves old handles dead).
const MAX_FAILS: u32 = 5;

pub struct Sensors {
    gpu: Option<gpu::Adapter>,
    gpu_fails: u32,
    gpu_retry: Instant,
    vram: Option<(pdh::Query, usize)>,
    cpu: Option<cputemp::CpuTemp>,
    cpu_fails: u32,
    pub cpu_status: cputemp::Status,
    cpu_retry: Instant,
    /// Drive letters to read, and the latest temperatures; a worker thread fills them so a slow
    /// or sleeping drive never stalls the 1-second sample.
    letters: Arc<Mutex<Vec<String>>>,
    drive_temps: Arc<Mutex<HashMap<String, f32>>>,
    net: Option<netetw::NetTrace>,
}

impl Sensors {
    pub fn new() -> Self {
        let vram = pdh::Query::new().and_then(|mut q| {
            let i = q.add(r"\GPU Adapter Memory(*)\Dedicated Usage")?;
            q.collect();
            Some((q, i))
        });
        let (cpu, cpu_status) = match cputemp::CpuTemp::open() {
            Ok(c) => (Some(c), cputemp::Status::Ok),
            Err(s) => (None, s),
        };
        let letters: Arc<Mutex<Vec<String>>> = Arc::default();
        let drive_temps: Arc<Mutex<HashMap<String, f32>>> = Arc::default();
        {
            let (l, t) = (letters.clone(), drive_temps.clone());
            let _ = std::thread::Builder::new().name("drive-temps".into()).spawn(move || loop {
                let ls = l.lock().clone();
                if !ls.is_empty() {
                    let r = drivetemp::read_all(&ls);
                    *t.lock() = r;
                }
                std::thread::sleep(Duration::from_secs(if ls.is_empty() { 1 } else { 10 }));
            });
        }
        Sensors {
            gpu: gpu::main_adapter(),
            gpu_fails: 0,
            gpu_retry: Instant::now(),
            vram,
            cpu,
            cpu_fails: 0,
            cpu_status,
            cpu_retry: Instant::now(),
            letters,
            drive_temps,
            net: netetw::NetTrace::start(),
        }
    }

    pub fn set_letters(&self, letters: &[String]) {
        let mut l = self.letters.lock();
        if l.as_slice() != letters {
            *l = letters.to_vec();
        }
    }

    /// "luid_0x..._0x..." of the main graphics card, for filtering the GPU total.
    pub fn gpu_luid(&self) -> Option<String> {
        self.gpu.as_ref().map(|a| a.pdh_luid.clone())
    }

    pub fn read(&mut self) -> Readings {
        let mut r = Readings::default();

        // CPU: if the driver was missing or the handle died, look again every 15 s so installing
        // the driver or waking from sleep works without restarting the app.
        if self.cpu.is_none() && self.cpu_status != cputemp::Status::Unsupported && self.cpu_retry.elapsed() > Duration::from_secs(15) {
            self.cpu_retry = Instant::now();
            match cputemp::CpuTemp::open() {
                Ok(c) => {
                    self.cpu = Some(c);
                    self.cpu_fails = 0;
                    self.cpu_status = cputemp::Status::Ok;
                }
                Err(s) => self.cpu_status = s,
            }
        }
        if let Some(c) = self.cpu.as_ref() {
            r.cpu_temp = c.read();
            if r.cpu_temp.is_none() {
                self.cpu_fails += 1;
                if self.cpu_fails >= MAX_FAILS {
                    self.cpu = None;
                    self.cpu_status = cputemp::Status::Failed;
                    self.cpu_retry = Instant::now();
                }
            } else {
                self.cpu_fails = 0;
            }
        }

        if self.gpu.is_none() && self.gpu_retry.elapsed() > Duration::from_secs(15) {
            self.gpu_retry = Instant::now();
            self.gpu = gpu::main_adapter();
            self.gpu_fails = 0;
        }
        if let Some(a) = self.gpu.as_ref() {
            match gpu::read_sensors(a) {
                Some(s) => {
                    self.gpu_fails = 0;
                    let used = self.vram.as_ref().and_then(|(q, i)| {
                        q.collect();
                        gpu::vram_used(a, &q.array(*i))
                    });
                    r.gpu = Some(GpuInfo {
                        name: a.name.clone(),
                        usage: 0.0,
                        vram_used: used,
                        vram_total: Some(a.vram_total),
                        temp: s.temp,
                        fan_rpm: s.fan_rpm,
                        clock_mhz: s.clock_mhz,
                    });
                }
                None => {
                    self.gpu_fails += 1;
                    // Keep showing the card (name, usage) while it recovers.
                    r.gpu = Some(GpuInfo { name: a.name.clone(), vram_total: Some(a.vram_total), ..Default::default() });
                    if self.gpu_fails >= MAX_FAILS {
                        self.gpu = None;
                        self.gpu_retry = Instant::now().checked_sub(Duration::from_secs(15)).unwrap_or_else(Instant::now);
                    }
                }
            }
        }

        r.drive_temps = self.drive_temps.lock().clone();
        r
    }

    /// Bytes per second per pid, or None when per-app network data is not available.
    pub fn net_per_pid(&mut self, interval_s: f64) -> Option<HashMap<u32, u64>> {
        self.net.as_ref().map(|n| n.drain(interval_s))
    }
}
