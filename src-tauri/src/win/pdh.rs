//! Thin wrapper over Windows performance counters (PDH). One query holds several wildcard
//! counters; `collect()` once per tick, then read each counter's instance array.

use windows::core::PCWSTR;
use windows::Win32::System::Performance::*;

/// Double values, not capped at 100 (PDH_FMT_NOCAP100 is missing from the windows crate).
const FMT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x0000_8000);

pub struct Query {
    q: PDH_HQUERY,
    counters: Vec<PDH_HCOUNTER>,
}

// The PDH handles are only touched from the sampler thread that owns the Query.
unsafe impl Send for Query {}

impl Query {
    pub fn new() -> Option<Self> {
        let mut q = PDH_HQUERY::default();
        let r = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut q) };
        (r == 0).then_some(Query { q, counters: Vec::new() })
    }

    /// Adds an English counter path (works on non-English Windows too). Returns its index.
    pub fn add(&mut self, path: &str) -> Option<usize> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut c = PDH_HCOUNTER::default();
        let r = unsafe { PdhAddEnglishCounterW(self.q, PCWSTR(wide.as_ptr()), 0, &mut c) };
        if r != 0 {
            return None;
        }
        self.counters.push(c);
        Some(self.counters.len() - 1)
    }

    pub fn collect(&self) {
        unsafe {
            PdhCollectQueryData(self.q);
        }
    }

    /// Every instance of the counter as (instance name, value). Empty until two collects
    /// have happened for rate counters, or when the counter has no instances right now.
    pub fn array(&self, idx: usize) -> Vec<(String, f64)> {
        let Some(&c) = self.counters.get(idx) else { return Vec::new() };
        let mut out = Vec::new();
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            // PDH_MORE_DATA on the sizing call is expected.
            PdhGetFormattedCounterArrayW(c, FMT, &mut size, &mut count, None);
            if size == 0 {
                return out;
            }
            // Instances can appear between the two calls; retry once with a fresh size.
            for _ in 0..2 {
                // u64 storage keeps the item array 8-byte aligned, as the struct requires.
                let mut buf = vec![0u64; (size as usize).div_ceil(8)];
                let r = PdhGetFormattedCounterArrayW(
                    c,
                    FMT,
                    &mut size,
                    &mut count,
                    Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
                );
                if r == PDH_MORE_DATA {
                    continue;
                }
                if r != 0 {
                    return out;
                }
                let items = std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
                for it in items {
                    // 0 = valid data, 1 = new data (also valid); anything else has no value yet.
                    if it.FmtValue.CStatus > 1 {
                        continue;
                    }
                    let name = it.szName.to_string().unwrap_or_default();
                    out.push((name, it.FmtValue.Anonymous.doubleValue));
                }
                break;
            }
        }
        out
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.q);
        }
    }
}
