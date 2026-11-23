// Runs src/app.js top to bottom against a tiny fake DOM and Tauri API, then feeds it one
// snapshot. Catches start-up errors (e.g. a `let` used before its line runs) that a syntax
// check misses and that would leave the real window empty. Run: node tools/ui-smoke.js
'use strict';
const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, '..', 'src', 'app.js'), 'utf8');

const el = () => ({
  textContent: '', innerHTML: '', style: {}, className: '', dataset: {}, title: '', disabled: false,
  classList: { toggle() {}, add() {}, remove() {} }, setAttribute() {}, addEventListener() {},
  closest() { return null }, focus() {},
});
const els = {};
const q = s => els[s] || (els[s] = el());
global.document = { querySelector: q, querySelectorAll: () => [], getElementById: q, activeElement: null };
global.localStorage = { getItem: () => null, setItem() {} };
global.addEventListener = () => {};

const calls = [];
const listeners = {};
global.window = {
  __TAURI__: {
    core: { invoke: c => { calls.push(c); return new Promise(() => {}) } },
    event: { listen: (n, f) => { listeners[n] = f } },
  },
};

const snapshot = {
  cpu: { name: 'Test CPU', cores: 2, threads: 4, usage: 12.5, per_thread: [1, 2, 3, 4], mhz: 3400, temp: null, processes: 10, thread_count: 100 },
  mem: { total: 16 * 2 ** 30, used: 8 * 2 ** 30, available: 8 * 2 ** 30, committed: 9 * 2 ** 30, commit_limit: 20 * 2 ** 30, cached: 2 ** 30 },
  gpu: { name: 'Test GPU', usage: 5, vram_used: 2 ** 30, vram_total: 8 * 2 ** 30, temp: 50, fan_rpm: null, clock_mhz: 500 },
  net: { adapter: 'Ethernet', link_mbps: 1000, rx: 1000, tx: 500 },
  disks: [{ letter: 'C:', total: 500 * 2 ** 30, free: 100 * 2 ** 30, active: 3, read: 0, write: 0, temp: 40 }],
  apps: [{ key: 'c:\\a.exe', name: 'A <b>"x"</b>', exe: 'C:\\a.exe', cpu: 1, mem: 2 ** 20, gpu: 0, disk: 0, net: 0, critical: false,
    procs: [{ pid: 5, name: 'a.exe', cpu: 1, mem: 2 ** 20, gpu: 0, disk: 0, net: 0, start: 1, critical: false }] }],
  uptime: 3600, per_app_net: false, cpu_temp_status: 'NoDriver',
};

try {
  eval(src);
  for (const want of ['startup_list', 'services_list', 'snapshot']) if (!calls.includes(want)) throw new Error('never called ' + want);
  for (const want of ['snapshot', 'fatal']) if (!listeners[want]) throw new Error('never listened for ' + want);
  listeners.snapshot({ payload: snapshot });
  if (!/Up 1 hour/.test(q('#ov-sub').textContent)) throw new Error('overview did not render: ' + q('#ov-sub').textContent);
  if (q('#top').innerHTML.includes('<b>')) throw new Error('app name was not escaped');
  console.log('ui smoke test ok');
} catch (e) {
  console.error('ui smoke test FAILED:', e && e.stack || e);
  process.exit(1);
}
