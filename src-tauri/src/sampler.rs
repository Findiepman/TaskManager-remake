//! Collects one snapshot of the whole machine per second on a background thread.

use crate::logic::{self, RawProc};
use crate::sensors::Sensors;
use crate::win::{pdh, verinfo};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use sysinfo::{DiskRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Serialize, Clone, Default)]
pub struct Snapshot {
    pub cpu: CpuInfo,
    pub mem: MemInfo,
    pub gpu: Option<GpuInfo>,
    pub net: NetInfo,
    pub disks: Vec<DiskInfo>,
    pub apps: Vec<AppGroup>,
    pub uptime: u64,
    /// Per-app network numbers are available (needs the ETW session).
    pub per_app_net: bool,
    /// Whether the CPU temperature can be read, and if not, why.
    pub cpu_temp_status: Option<crate::win::cputemp::Status>,
}

#[derive(Serialize, Clone, Default)]
pub struct CpuInfo {
    pub name: String,
    pub cores: usize,
    pub threads: usize,
    pub usage: f32,
    pub per_thread: Vec<f32>,
    pub mhz: Option<u32>,
    pub temp: Option<f32>,
    pub processes: u32,
    pub thread_count: u32,
}

#[derive(Serialize, Clone, Default)]
pub struct MemInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub committed: u64,
    pub commit_limit: u64,
    pub cached: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct GpuInfo {
    pub name: String,
    pub usage: f32,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub temp: Option<f32>,
    pub fan_rpm: Option<u32>,
    pub clock_mhz: Option<u32>,
}

#[derive(Serialize, Clone, Default)]
pub struct NetInfo {
    pub adapter: String,
    pub link_mbps: Option<u64>,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct DiskInfo {
    pub letter: String,
    pub total: u64,
    pub free: u64,
    pub active: f32,
    pub read: u64,
    pub write: u64,
    pub temp: Option<f32>,
}

#[derive(Serialize, Clone, Default)]
pub struct AppGroup {
    pub key: String,
    pub name: String,
    pub exe: Option<String>,
    pub cpu: f32,
    pub mem: u64,
    pub gpu: f32,
    pub disk: u64,
    pub net: u64,
    pub critical: bool,
    pub procs: Vec<ProcInfo>,
}

#[derive(Serialize, Clone, Default)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub mem: u64,
    pub gpu: f32,
    pub disk: u64,
    pub net: u64,
    pub start: u64,
    pub critical: bool,
}

pub struct Sampler {
    sys: System,
    nets: Option<Networks>,
    disks: Option<sysinfo::Disks>,
    pdh: Option<pdh::Query>,
    c_gpu: Option<usize>,
    c_idle: Option<usize>,
    c_read: Option<usize>,
    c_write: Option<usize>,
    c_perf: Option<usize>,
    c_freq: Option<usize>,
    ver: HashMap<String, verinfo::VerInfo>,
    /// (pid, start time) -> Windows marks it critical. A pid+start pair never changes meaning.
    critical: HashMap<(u32, u64), bool>,
    pub sensors: Sensors,
    /// (pid, parent, start time) of the last sample, for "end task" / "end process tree".
    pub procs: Vec<(u32, Option<u32>, u64)>,
    /// Adapter shown on the Network card last tick.
    net_pick: Option<String>,
    ticks: u64,
}

/// New exes whose version info is read per tick; the rest wait for the next tick so one slow
/// file (e.g. on a network share) can't stall the whole first sample.
const VERINFO_PER_TICK: usize = 64;

impl Sampler {
    pub fn new() -> Result<Self, String> {
        let mut sys = System::new().map_err(|e| format!("Could not read system information: {e:?}"))?;
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let mut pdh = pdh::Query::new();
        let (mut c_gpu, mut c_idle, mut c_read, mut c_write, mut c_perf, mut c_freq) = (None, None, None, None, None, None);
        if let Some(q) = pdh.as_mut() {
            c_gpu = q.add(r"\GPU Engine(*)\Utilization Percentage");
            c_idle = q.add(r"\LogicalDisk(*)\% Idle Time");
            c_read = q.add(r"\LogicalDisk(*)\Disk Read Bytes/sec");
            c_write = q.add(r"\LogicalDisk(*)\Disk Write Bytes/sec");
            c_perf = q.add(r"\Processor Information(_Total)\% Processor Performance");
            c_freq = q.add(r"\Processor Information(_Total)\Processor Frequency");
            q.collect();
        }
        Ok(Sampler {
            sys,
            nets: Networks::new_with_refreshed_list().ok(),
            disks: sysinfo::Disks::new_with_refreshed_list().ok(),
            pdh,
            c_gpu,
            c_idle,
            c_read,
            c_write,
            c_perf,
            c_freq,
            ver: HashMap::new(),
            critical: HashMap::new(),
            sensors: Sensors::new(),
            procs: Vec::new(),
            net_pick: None,
            ticks: 0,
        })
    }

    pub fn sample(&mut self, interval_s: f64) -> Snapshot {
        self.ticks += 1;
        let threads = self.sys.cpus().len().max(1);
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory().with_cpu().with_disk_usage().with_exe(UpdateKind::OnlyIfNotSet),
        );
        if let Some(n) = self.nets.as_mut() {
            n.refresh(true);
        }
        // Rescanning the drive list enumerates every volume, so do it every 10 s; in between
        // only refresh the free space of the drives already known.
        if let Some(d) = self.disks.as_mut() {
            if self.ticks % 10 == 0 {
                d.refresh(true);
            } else {
                for disk in d.iter_mut() {
                    disk.refresh_specifics(DiskRefreshKind::nothing().with_storage());
                }
            }
        }
        let letters: Vec<String> = self
            .disks
            .iter()
            .flat_map(|d| d.iter())
            .map(|d| d.mount_point().to_string_lossy().trim_end_matches('\\').to_string())
            .filter(|l| l.len() == 2)
            .collect();
        self.sensors.set_letters(&letters);

        let gpu_luid = self.sensors.gpu_luid();
        let (gpu_per_pid, gpu_total, idle, rbytes, wbytes, perf, freq) = match self.pdh.as_ref() {
            Some(q) => {
                q.collect();
                let g = self.c_gpu.map(|i| q.array(i)).unwrap_or_default();
                let (per, total) = logic::aggregate_gpu(g.iter().map(|(n, v)| (n.as_str(), *v)), gpu_luid.as_deref());
                let to_map = |i: Option<usize>| -> HashMap<String, f64> { i.map(|i| q.array(i)).unwrap_or_default().into_iter().collect() };
                let first = |i: Option<usize>| i.and_then(|i| q.array(i).first().map(|x| x.1));
                (per, total, to_map(self.c_idle), to_map(self.c_read), to_map(self.c_write), first(self.c_perf), first(self.c_freq))
            }
            None => Default::default(),
        };

        let net_per_pid = self.sensors.net_per_pid(interval_s);
        let sensors = self.sensors.read();

        // processes (pid 0 is the "System Idle Process", not a program)
        let mut raws = Vec::with_capacity(self.sys.processes().len());
        let mut live: HashSet<(u32, u64)> = HashSet::with_capacity(self.sys.processes().len());
        for (pid, p) in self.sys.processes() {
            let pid = pid.as_u32();
            if pid == 0 {
                continue;
            }
            let name = p.name().to_string_lossy().to_string();
            let start = p.start_time();
            live.insert((pid, start));
            let critical = *self.critical.entry((pid, start)).or_insert_with(|| crate::win::actions::is_critical_pid(pid, &name));
            let du = p.disk_usage();
            raws.push(RawProc {
                pid,
                parent: p.parent().map(|x| x.as_u32()),
                exe: p.exe().map(|e| e.to_string_lossy().to_string()),
                cpu: p.cpu_usage() / threads as f32,
                // Private working set, like Windows Task Manager; working set as a fallback.
                mem: crate::win::perfinfo::private_working_set(pid).unwrap_or_else(|| p.memory()),
                gpu: gpu_per_pid.get(&pid).copied().unwrap_or(0.0),
                disk: ((du.read_bytes + du.written_bytes) as f64 / interval_s) as u64,
                net: net_per_pid.as_ref().and_then(|m| m.get(&pid).copied()).unwrap_or(0),
                start,
                critical,
                name,
            });
        }
        self.critical.retain(|k, _| live.contains(k));
        self.procs = raws.iter().map(|r| (r.pid, r.parent, r.start)).collect();

        let mut budget = VERINFO_PER_TICK;
        let mut apps = Vec::new();
        for g in logic::group(&raws) {
            let first = g.procs[0];
            let desc = match &first.exe {
                Some(e) => {
                    let k = e.to_lowercase();
                    if !self.ver.contains_key(&k) && budget > 0 {
                        budget -= 1;
                        self.ver.insert(k.clone(), verinfo::read(e));
                    }
                    self.ver.get(&k).and_then(|v| v.description.clone())
                }
                None => None,
            };
            let name = logic::display_name(&first.name, desc.as_deref());
            let procs: Vec<ProcInfo> = g
                .procs
                .iter()
                .map(|p| ProcInfo {
                    pid: p.pid,
                    name: p.name.clone(),
                    cpu: p.cpu,
                    mem: p.mem,
                    gpu: p.gpu,
                    disk: p.disk,
                    net: p.net,
                    start: p.start,
                    critical: p.critical,
                })
                .collect();
            apps.push(AppGroup {
                key: g.key.clone(),
                name,
                exe: first.exe.clone(),
                cpu: procs.iter().map(|p| p.cpu).sum::<f32>().min(100.0),
                mem: procs.iter().map(|p| p.mem).sum(),
                gpu: procs.iter().map(|p| p.gpu).sum::<f32>().min(100.0),
                disk: procs.iter().map(|p| p.disk).sum(),
                net: procs.iter().map(|p| p.net).sum(),
                // One critical process makes the whole group's "End task" unsafe.
                critical: procs.iter().any(|p| p.critical),
                procs,
            });
        }
        // Drop the version cache entries of exes that are gone so it can't grow forever.
        if self.ticks % 60 == 0 {
            let live: HashSet<String> = raws.iter().filter_map(|r| r.exe.as_ref().map(|e| e.to_lowercase())).collect();
            self.ver.retain(|k, _| live.contains(k));
        }

        // cpu
        let cpus = self.sys.cpus();
        let pi = crate::win::perfinfo::read();
        let cpu = CpuInfo {
            name: cpus.first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
            cores: System::physical_core_count().unwrap_or(threads),
            threads,
            usage: self.sys.global_cpu_usage(),
            per_thread: cpus.iter().map(|c| c.usage()).collect(),
            // Base clock x "% Processor Performance" is how Task Manager shows the live speed.
            mhz: match (freq, perf) {
                (Some(f), Some(p)) if f > 0.0 => Some((f * p / 100.0).round() as u32),
                (Some(f), None) if f > 0.0 => Some(f as u32),
                _ => cpus.first().map(|c| c.frequency() as u32).filter(|&m| m > 0),
            },
            temp: sensors.cpu_temp,
            processes: pi.processes,
            thread_count: pi.threads,
        };

        let mem = MemInfo {
            total: self.sys.total_memory(),
            used: self.sys.used_memory(),
            available: self.sys.available_memory(),
            committed: pi.committed,
            commit_limit: pi.commit_limit,
            cached: pi.cached,
        };

        let gpu = sensors.gpu.map(|g| GpuInfo { usage: gpu_total, ..g });

        // network: the real adapter carrying the traffic (skips loopback, virtual switches and
        // filter layers); see logic::pick_adapter for how it avoids flipping between adapters.
        let mut net = NetInfo::default();
        if let Some(nets) = self.nets.as_ref() {
            let real: Vec<(String, &sysinfo::NetworkData)> =
                nets.iter().map(|(n, d)| (n.to_string_lossy().to_string(), d)).filter(|(n, _)| is_real_adapter(n)).collect();
            let cands: Vec<(String, u64, u64)> =
                real.iter().map(|(n, d)| (n.clone(), d.received() + d.transmitted(), d.total_received() + d.total_transmitted())).collect();
            if let Some(i) = logic::pick_adapter(self.net_pick.as_deref(), &cands) {
                let (name, d) = &real[i];
                self.net_pick = Some(name.clone());
                net = NetInfo {
                    adapter: name.clone(),
                    link_mbps: d.receive_link_speed().map(|b| b / 1_000_000).filter(|&m| m > 0),
                    rx: (d.received() as f64 / interval_s) as u64,
                    tx: (d.transmitted() as f64 / interval_s) as u64,
                };
            }
        }

        let mut disks: Vec<DiskInfo> = Vec::new();
        for d in self.disks.iter().flat_map(|d| d.iter()) {
            let mp = d.mount_point().to_string_lossy().to_string();
            let letter = mp.trim_end_matches('\\').to_string();
            if letter.len() != 2 || d.total_space() == 0 || disks.iter().any(|x| x.letter == letter) {
                continue;
            }
            let pick = |m: &HashMap<String, f64>| m.get(&letter).copied();
            disks.push(DiskInfo {
                active: pick(&idle).map(|i| (100.0 - i).clamp(0.0, 100.0) as f32).unwrap_or(0.0),
                read: pick(&rbytes).unwrap_or(0.0) as u64,
                write: pick(&wbytes).unwrap_or(0.0) as u64,
                temp: sensors.drive_temps.get(&letter.to_uppercase()).copied(),
                total: d.total_space(),
                free: d.available_space(),
                letter,
            });
        }
        disks.sort_by(|a, b| a.letter.cmp(&b.letter));

        Snapshot {
            cpu,
            mem,
            gpu,
            net,
            disks,
            apps,
            uptime: System::uptime().unwrap_or(0),
            per_app_net: net_per_pid.is_some(),
            cpu_temp_status: Some(self.sensors.cpu_status),
        }
    }
}

pub fn is_real_adapter(name: &str) -> bool {
    let l = name.to_lowercase();
    const SKIP: &[&str] = &["loopback", "vethernet", "filter", "wfp", "qos", "scheduler", "teredo", "isatap", "6to4", "kernel debug", "npcap", "virtualbox host", "vmware network", "bluetooth"];
    !SKIP.iter().any(|s| l.contains(s))
}

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_filter() {
        assert!(super::is_real_adapter("Ethernet"));
        assert!(super::is_real_adapter("Wi-Fi"));
        assert!(!super::is_real_adapter("vEthernet (WSL)"));
        assert!(!super::is_real_adapter("Ethernet-WFP Native MAC Layer LightWeight Filter-0000"));
        assert!(!super::is_real_adapter("Loopback Pseudo-Interface 1"));
    }
}
