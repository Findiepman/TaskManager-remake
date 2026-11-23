//! Per-app network traffic from the Microsoft-Windows-Kernel-Network ETW provider: every TCP
//! and UDP send/receive event carries the owning PID and the byte count. Needs admin.

use ferrisetw::parser::Parser;
use ferrisetw::provider::{EventFilter, Provider};
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, TraceTrait, UserTrace};
use ferrisetw::EventRecord;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

const SESSION: &str = "TaskManagerRemake-Net";
const KERNEL_NETWORK: &str = "7dd42a49-5329-4832-8dfd-43d979153a88";
/// TCPv4/v6 and UDPv4/v6 send+receive. Retransmit (14) and copy (18) are left out: they would
/// count the same bytes twice.
const EVENTS: [u16; 8] = [10, 11, 26, 27, 42, 43, 58, 59];

static TRACE: Mutex<Option<UserTrace>> = Mutex::new(None);

pub struct NetTrace {
    acc: Arc<Mutex<HashMap<u32, u64>>>,
}

impl NetTrace {
    pub fn start() -> Option<Self> {
        let acc: Arc<Mutex<HashMap<u32, u64>>> = Arc::default();
        let a = acc.clone();
        let cb = move |rec: &EventRecord, loc: &SchemaLocator| {
            let Ok(schema) = loc.event_schema(rec) else { return };
            let p = Parser::create(rec, &schema);
            // The payload PID is the owner; the event header PID is often an unrelated context.
            let (Ok(pid), Ok(size)) = (p.try_parse::<u32>("PID"), p.try_parse::<u32>("size")) else { return };
            // Traffic to the same machine (source = destination, e.g. 127.0.0.1) never leaves the
            // PC and is not in the adapter totals, so it does not count as network use.
            let local = match rec.event_id() {
                10 | 11 | 42 | 43 => matches!((p.try_parse::<u32>("saddr"), p.try_parse::<u32>("daddr")), (Ok(a), Ok(b)) if a == b),
                _ => matches!((p.try_parse::<Vec<u8>>("saddr"), p.try_parse::<Vec<u8>>("daddr")), (Ok(a), Ok(b)) if a == b),
            };
            if local {
                return;
            }
            *a.lock().entry(pid).or_insert(0) += size as u64;
        };
        let build = || {
            let prov = Provider::by_guid(KERNEL_NETWORK)
                .any(0x30) // IPv4 | IPv6 keywords
                .add_filter(EventFilter::ByEventIds(EVENTS.to_vec()))
                .add_callback(cb.clone())
                .build();
            UserTrace::new().named(SESSION.to_string()).enable(prov).start()
        };
        // A session left over from a crash keeps the name busy: stop it and try again.
        let started = build().or_else(|_| {
            let _ = stop_trace_by_name(SESSION);
            build()
        });
        let (trace, handle) = started.ok()?;
        std::thread::Builder::new()
            .name("etw-net".into())
            .spawn(move || {
                let _ = UserTrace::process_from_handle(handle);
            })
            .ok()?;
        *TRACE.lock() = Some(trace);
        Some(NetTrace { acc })
    }

    /// Bytes per second per pid since the last call.
    pub fn drain(&self, interval_s: f64) -> HashMap<u32, u64> {
        let m = std::mem::take(&mut *self.acc.lock());
        m.into_iter().map(|(k, v)| (k, (v as f64 / interval_s) as u64)).collect()
    }
}

/// Stops the ETW session; called when the app exits so it is not left running in Windows.
pub fn shutdown() {
    if let Some(t) = TRACE.lock().take() {
        let _ = t.stop();
    }
    let _ = stop_trace_by_name(SESSION);
}
