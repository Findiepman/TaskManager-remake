//! Pure logic with no Windows calls, so it can be unit tested in a second.

use std::collections::HashMap;

/// One raw process as the sampler sees it.
#[derive(Debug, Clone)]
pub struct RawProc {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub exe: Option<String>,
    /// Percent of the whole machine (already divided by the logical CPU count).
    pub cpu: f32,
    pub mem: u64,
    pub gpu: f32,
    /// Bytes per second.
    pub disk: u64,
    /// Bytes per second.
    pub net: u64,
    /// Start time, seconds since the Unix epoch. Tells a process apart from a later one that
    /// got the same (reused) pid.
    pub start: u64,
    /// Windows marks it critical: ending it crashes the machine.
    pub critical: bool,
}

/// Grouping key: the exe path (case-insensitive) when known, else the process name.
/// Protected system processes have no readable path, so they group by name.
pub fn group_key(p: &RawProc) -> String {
    match &p.exe {
        Some(e) if !e.is_empty() => e.to_lowercase(),
        _ => format!("name:{}", p.name.to_lowercase()),
    }
}

pub struct Group<'a> {
    pub key: String,
    pub procs: Vec<&'a RawProc>,
}

/// Groups processes by key and keeps a stable order (biggest memory first).
pub fn group<'a>(procs: &'a [RawProc]) -> Vec<Group<'a>> {
    let mut map: HashMap<String, Vec<&'a RawProc>> = HashMap::new();
    for p in procs {
        map.entry(group_key(p)).or_default().push(p);
    }
    let mut out: Vec<Group> = map
        .into_iter()
        .map(|(key, mut procs)| {
            procs.sort_by(|a, b| b.mem.cmp(&a.mem).then(a.pid.cmp(&b.pid)));
            Group { key, procs }
        })
        .collect();
    out.sort_by(|a, b| {
        let ma: u64 = a.procs.iter().map(|p| p.mem).sum();
        let mb: u64 = b.procs.iter().map(|p| p.mem).sum();
        mb.cmp(&ma).then(a.key.cmp(&b.key))
    });
    out
}

/// Friendly display name for a group: the file description from the exe's version info
/// when it is useful, else the exe name without ".exe".
pub fn display_name(proc_name: &str, description: Option<&str>) -> String {
    let base = proc_name.strip_suffix(".exe").or_else(|| proc_name.strip_suffix(".EXE")).unwrap_or(proc_name);
    if base.eq_ignore_ascii_case("svchost") {
        return "Service Host".into();
    }
    match description.map(str::trim) {
        Some(d) if !d.is_empty() && d.len() <= 60 => d.to_string(),
        _ => base.to_string(),
    }
}

/// Every pid in the subtree rooted at `root` (root included), children before parents
/// so a kill pass ends the leaves first. Input is (pid, parent, start time). A process only
/// counts as a child when it started at or after its parent: Windows keeps the parent pid of a
/// dead parent, and that pid can be reused by an unrelated, newer process.
pub fn subtree(root: u32, procs: &[(u32, Option<u32>, u64)]) -> Vec<u32> {
    let start: HashMap<u32, u64> = procs.iter().map(|&(p, _, s)| (p, s)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, parent, s) in procs {
        if let Some(pp) = parent {
            if pp != pid && start.get(&pp).is_some_and(|&ps| s >= ps) {
                children.entry(pp).or_default().push(pid);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    let mut stack = vec![(root, false)];
    while let Some((pid, expanded)) = stack.pop() {
        if expanded {
            order.push(pid);
            continue;
        }
        if !seen.insert(pid) {
            continue;
        }
        stack.push((pid, true));
        if let Some(kids) = children.get(&pid) {
            for &k in kids {
                if !seen.contains(&k) {
                    stack.push((k, false));
                }
            }
        }
    }
    order
}

/// Parses a `\GPU Engine(*)` instance name such as
/// `pid_1234_luid_0x00000000_0x0000D1A3_phys_0_eng_0_engtype_3D` into the pid and the
/// engine id (everything after the pid, i.e. one physical engine on one adapter).
/// PDH may return the name in any case.
pub fn parse_gpu_instance(name: &str) -> Option<(u32, String)> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("pid_")?;
    let (pid, engine) = rest.split_once('_')?;
    let pid: u32 = pid.parse().ok()?;
    engine.contains("engtype_").then(|| (pid, engine.to_string()))
}

/// Aggregates GPU engine utilisation the way Windows Task Manager does ("the busiest engine"):
/// a process shows its busiest single engine; the machine total is, per physical engine, the
/// sum over all processes, and then the busiest engine. Values are clamped to 0..=100.
/// `adapter` limits the machine total to one card (its "luid_0x..._0x..." prefix), so a busy
/// integrated GPU is not shown as load on the main card.
pub fn aggregate_gpu<'a>(items: impl IntoIterator<Item = (&'a str, f64)>, adapter: Option<&str>) -> (HashMap<u32, f32>, f32) {
    let mut per_proc: HashMap<u32, f32> = HashMap::new();
    let mut per_engine: HashMap<String, f64> = HashMap::new();
    for (name, v) in items {
        if !v.is_finite() || v <= 0.0 {
            continue;
        }
        if let Some((pid, engine)) = parse_gpu_instance(name) {
            let e = per_proc.entry(pid).or_default();
            *e = e.max(v.min(100.0) as f32);
            if adapter.is_none_or(|a| engine.starts_with(a)) {
                *per_engine.entry(engine).or_default() += v;
            }
        }
    }
    let total = per_engine.values().cloned().fold(0.0, f64::max).min(100.0) as f32;
    (per_proc, total)
}

/// Which network adapter the Network card shows. Candidates are (name, bytes this tick, bytes
/// since boot). Stays on the previous adapter unless another one clearly carries more right now
/// (twice as much and at least 64 KB), so the card doesn't flip between e.g. Ethernet and a VPN
/// that mirrors the same traffic. With no previous pick: busiest now, then busiest since boot.
pub fn pick_adapter(prev: Option<&str>, cands: &[(String, u64, u64)]) -> Option<usize> {
    let best = (0..cands.len()).max_by_key(|&i| (cands[i].1, cands[i].2))?;
    if let Some(pi) = prev.and_then(|p| cands.iter().position(|c| c.0 == p)) {
        let (cur, top) = (cands[pi].1, cands[best].1);
        if top < cur.saturating_mul(2) || top - cur < 64 * 1024 {
            return Some(pi);
        }
    }
    Some(best)
}

/// Startup impact using Windows Task Manager's thresholds: High above 1 s CPU or 3 MB disk,
/// Medium above 300 ms or 300 KB, else Low.
pub fn startup_impact(cpu_us: u64, disk_bytes: u64) -> &'static str {
    if cpu_us > 1_000_000 || disk_bytes > 3 * 1024 * 1024 {
        "High"
    } else if cpu_us > 300_000 || disk_bytes > 300 * 1024 {
        "Medium"
    } else {
        "Low"
    }
}

/// StartupApproved values are 12 bytes; an even first byte means enabled, odd means disabled.
/// No value at all means enabled.
pub fn startup_approved_enabled(data: Option<&[u8]>) -> bool {
    match data.and_then(|d| d.first()) {
        Some(b) => b % 2 == 0,
        None => true,
    }
}

/// Pulls the exe path out of a Run-key command line: a quoted path, or everything up to
/// the first ".exe".
pub fn exe_from_command(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().filter(|s| !s.is_empty()).map(str::to_string);
    }
    // ASCII lowercase keeps byte offsets identical to `c` (full Unicode lowercase does not).
    let lower = c.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(".exe").map(|i| i + from) {
        let end = i + 4;
        // Only a real ".exe" ending: followed by the end, a space or a quote (not "C:\my.executor\").
        if lower[end..].chars().next().is_none_or(|ch| ch.is_whitespace() || ch == '"') {
            return Some(c[..end].to_string());
        }
        from = end;
    }
    c.split_whitespace().next().map(str::to_string)
}

/// Very small XML attribute/element reader for the Windows StartupInfo files: returns
/// (process path, cpu microseconds, disk bytes) for every `<Process Name="...">` block.
pub fn parse_startup_info(xml: &str) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for block in xml.split("<Process ").skip(1) {
        let end = block.find("</Process>").unwrap_or(block.len());
        let block = &block[..end];
        let name = match attr(block, "Name") {
            Some(n) => n,
            None => continue,
        };
        let cpu = element(block, "CpuUsage").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let disk = element(block, "DiskUsage").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        out.push((xml_unescape(&name), cpu, disk));
    }
    out
}

fn attr(s: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = s.find(&pat)? + pat.len();
    let j = s[i..].find('"')? + i;
    Some(s[i..j].to_string())
}

fn element<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = s.find(&format!("<{name}"))?;
    let gt = s[open..].find('>')? + open + 1;
    let close = s[gt..].find(&format!("</{name}>"))? + gt;
    Some(&s[gt..close])
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Programs that only run something else (a script, a DLL, a command), so their own file
/// description says nothing about the startup item.
pub fn is_generic_host(exe: &str) -> bool {
    const HOSTS: &[&str] = &["wscript.exe", "cscript.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "rundll32.exe", "mshta.exe", "conhost.exe", "javaw.exe", "java.exe", "python.exe", "pythonw.exe", "node.exe"];
    let file = exe.trim().trim_matches('"').rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    HOSTS.contains(&file.as_str())
}

/// Normalises a path for comparisons between Run-key commands and process paths.
pub fn norm_path(p: &str) -> String {
    p.trim().trim_matches('"').replace('/', "\\").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: Option<u32>, name: &str, exe: Option<&str>, mem: u64) -> RawProc {
        RawProc { pid, parent, name: name.into(), exe: exe.map(Into::into), cpu: 0.0, mem, gpu: 0.0, disk: 0, net: 0, start: 0, critical: false }
    }

    #[test]
    fn groups_by_exe_case_insensitive_and_sorts_by_memory() {
        let v = vec![
            p(1, None, "brave.exe", Some(r"C:\B\brave.exe"), 100),
            p(2, Some(1), "brave.exe", Some(r"c:\b\BRAVE.exe"), 300),
            p(3, None, "Code.exe", Some(r"C:\Code.exe"), 250),
            p(4, None, "System", None, 10),
        ];
        let g = group(&v);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].procs.len(), 2);
        assert_eq!(g[0].procs[0].pid, 2, "biggest child first");
        assert_eq!(g[1].key, r"c:\code.exe");
        assert_eq!(g[2].key, "name:system");
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("svchost.exe", Some("Host Process for Windows Services")), "Service Host");
        assert_eq!(display_name("Code.exe", Some("Visual Studio Code")), "Visual Studio Code");
        assert_eq!(display_name("rust-analyzer.exe", None), "rust-analyzer");
        assert_eq!(display_name("foo.exe", Some("   ")), "foo");
        assert_eq!(display_name("Registry", None), "Registry");
    }

    #[test]
    fn subtree_children_first_and_cycle_safe() {
        let rel = vec![(1, None, 10), (2, Some(1), 11), (3, Some(2), 12), (4, Some(1), 10), (5, Some(9), 1), (6, Some(6), 1)];
        let t = subtree(1, &rel);
        assert_eq!(t.len(), 4);
        assert_eq!(*t.last().unwrap(), 1);
        let pos = |x| t.iter().position(|&y| y == x).unwrap();
        assert!(pos(3) < pos(2));
        // a pid listed as its own parent must not loop
        assert_eq!(subtree(6, &rel), vec![6]);
        // cycle 7 -> 8 -> 7 (same start time) must terminate
        let cyc = vec![(7, Some(8), 5), (8, Some(7), 5)];
        assert_eq!(subtree(7, &cyc).len(), 2);
    }

    #[test]
    fn subtree_ignores_reused_parent_pid() {
        // csrss (pid 20) was started by a smss that exited; its parent pid 9 was later reused by
        // an app that started much later. Ending that app's tree must not include csrss.
        let rel = vec![(9, None, 5000), (20, Some(9), 100), (21, Some(9), 5001)];
        let t = subtree(9, &rel);
        assert!(!t.contains(&20));
        assert!(t.contains(&21));
    }

    #[test]
    fn gpu_parsing_and_task_manager_aggregation() {
        assert_eq!(
            parse_gpu_instance("pid_1234_luid_0x00000000_0x0000D1A3_phys_0_eng_0_engtype_3D"),
            Some((1234, "luid_0x00000000_0x0000d1a3_phys_0_eng_0_engtype_3d".to_string()))
        );
        assert_eq!(parse_gpu_instance("pid_x_luid"), None);
        assert_eq!(parse_gpu_instance("pid_12_luid_0x0"), None);
        let items = vec![
            ("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 20.0),
            ("pid_10_luid_0x0_0x1_phys_0_eng_4_engtype_VideoDecode", 12.0),
            ("PID_20_LUID_0X0_0X1_PHYS_0_ENG_0_ENGTYPE_3D", 30.0),
            ("pid_20_luid_0x0_0x1_phys_0_eng_1_engtype_Copy", 35.0),
            ("pid_20_luid_0x0_0x1_phys_0_eng_2_engtype_Copy", f64::NAN),
        ];
        let (per, total) = aggregate_gpu(items.iter().map(|(n, v)| (*n, *v)), None);
        assert_eq!(per[&10], 20.0, "busiest engine of the process");
        assert_eq!(per[&20], 35.0);
        assert_eq!(total, 50.0, "engine 0 is shared: 20 + 30");
        let (_, capped) = aggregate_gpu(vec![("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 80.0), ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 70.0)], None);
        assert_eq!(capped, 100.0);
        // the integrated GPU (luid 0x2) is busy; the main card (luid 0x1) total ignores it
        let two = vec![("pid_1_luid_0x0_0x2_phys_0_eng_0_engtype_3D", 90.0), ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 10.0)];
        let (per2, main) = aggregate_gpu(two, Some("luid_0x0_0x1"));
        assert_eq!(main, 10.0);
        assert_eq!(per2[&1], 90.0, "per-app values still cover every GPU");
    }

    #[test]
    fn adapter_pick_is_sticky() {
        let c = |v: &[(&str, u64, u64)]| v.iter().map(|x| (x.0.to_string(), x.1, x.2)).collect::<Vec<_>>();
        // first pick: busiest now
        assert_eq!(pick_adapter(None, &c(&[("Ethernet", 10, 9000), ("Wi-Fi", 500, 10)])), Some(1));
        // all idle: busiest since boot
        assert_eq!(pick_adapter(None, &c(&[("Ethernet", 0, 9000), ("Wi-Fi", 0, 10)])), Some(0));
        // a VPN mirroring Ethernet's traffic doesn't steal the card
        assert_eq!(pick_adapter(Some("Ethernet"), &c(&[("Ethernet", 100_000, 1), ("VPN", 120_000, 1)])), Some(0));
        // clearly busier elsewhere: switch
        assert_eq!(pick_adapter(Some("Ethernet"), &c(&[("Ethernet", 1000, 1), ("Wi-Fi", 900_000, 1)])), Some(1));
        // previous adapter gone
        assert_eq!(pick_adapter(Some("Old"), &c(&[("Wi-Fi", 0, 5)])), Some(0));
        assert_eq!(pick_adapter(None, &[]), None);
    }

    #[test]
    fn impact_thresholds() {
        assert_eq!(startup_impact(1_500_000, 0), "High");
        assert_eq!(startup_impact(0, 4 * 1024 * 1024), "High");
        assert_eq!(startup_impact(400_000, 0), "Medium");
        assert_eq!(startup_impact(100_000, 1000), "Low");
    }

    #[test]
    fn approved_and_command_parsing() {
        assert!(startup_approved_enabled(None));
        assert!(startup_approved_enabled(Some(&[2, 0, 0])));
        assert!(startup_approved_enabled(Some(&[6])));
        assert!(!startup_approved_enabled(Some(&[3, 0])));
        assert_eq!(exe_from_command(r#""C:\Program Files\Steam\steam.exe" -silent"#).unwrap(), r"C:\Program Files\Steam\steam.exe");
        assert_eq!(exe_from_command(r"C:\Program Files\Foo\foo.EXE /min").unwrap(), r"C:\Program Files\Foo\foo.EXE");
        assert_eq!(exe_from_command("rundll32 x.dll").unwrap(), "rundll32");
        // Unicode that changes length when lowercased must not panic or cut wrongly
        assert_eq!(exe_from_command(r"C:\Users\İsmail\app.exe -x").unwrap(), r"C:\Users\İsmail\app.exe");
        assert_eq!(exe_from_command(r"C:\İİİİ\a.EXE").unwrap(), r"C:\İİİİ\a.EXE");
        // ".exe" inside a folder name is not the end of the path
        assert_eq!(exe_from_command(r"C:\my.executor\run.exe /q").unwrap(), r"C:\my.executor\run.exe");
    }

    #[test]
    fn generic_hosts() {
        assert!(is_generic_host(r"C:\Windows\System32\WScript.exe"));
        assert!(is_generic_host("powershell.exe"));
        assert!(!is_generic_host(r"C:\Program Files\Steam\steam.exe"));
    }

    #[test]
    fn startup_info_xml() {
        let xml = r#"<?xml version="1.0"?><StartupData>
<Process Name="C:\Users\u\AppData\Local\Discord\Update.exe" PID="1"><StartTime>1</StartTime><CpuUsage Unit="us">1234567</CpuUsage><DiskUsage Unit="bytes">2048</DiskUsage></Process>
<Process Name="C:\A &amp; B\x.exe"><CpuUsage>10</CpuUsage></Process>
</StartupData>"#;
        let v = parse_startup_info(xml);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], (r"C:\Users\u\AppData\Local\Discord\Update.exe".to_string(), 1234567, 2048));
        assert_eq!(v[1].0, r"C:\A & B\x.exe");
        assert_eq!(v[1].2, 0);
    }
}
