'use strict';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = s => document.querySelector(s), $$ = s => [...document.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const N = 60;
const CPUT = [70, 82], GPUT = [75, 90], DRVT = [50, 60];

/* ---------- formatting ---------- */
const GB = 1024 ** 3, MB = 1024 ** 2;
function bytes(b) {
  if (b == null) return '–';
  if (b >= GB) return (b / GB).toFixed(1) + ' GB';
  if (b >= MB) return Math.round(b / MB) + ' MB';
  return Math.round(b / 1024) + ' KB';
}
const gb = b => (b / GB).toFixed(1);
function rate(bps) { // bytes per second, for disks
  if (bps >= MB) return (bps / MB).toFixed(1) + ' MB/s';
  if (bps >= 1024) return Math.round(bps / 1024) + ' KB/s';
  return '0 MB/s';
}
function bits(bps) { // bytes per second shown as bits, for networks
  const b = bps * 8;
  if (b >= 1e9) return (b / 1e9).toFixed(1) + ' Gbps';
  if (b >= 1e6) return (b / 1e6).toFixed(1) + ' Mbps';
  if (b >= 1e3) return Math.round(b / 1e3) + ' Kbps';
  return '0 Mbps';
}
const pct = v => (v ?? 0).toFixed(v >= 10 || v === 0 ? 0 : 1) + '%';
const pct1 = v => (v ?? 0).toFixed(1) + '%';
function uptime(s) {
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60);
  return { long: (d ? d + (d === 1 ? ' day ' : ' days ') : '') + h + (h === 1 ? ' hour' : ' hours'), short: (d ? d + ' d ' : '') + h + ' h' + (d ? '' : ' ' + m + ' m') };
}
const tclass = (v, w, h) => v >= h ? 'th' : v >= w ? 'tw' : '';

/* ---------- state ---------- */
let snap = null;
const H = { cpu: [], mem: [], gpu: [], net: [], threads: [], disks: {} };
const push = (a, v) => { a.push(v); while (a.length > N) a.shift() };
const store = {
  get: (k, d) => { try { return JSON.parse(localStorage.getItem('tm.' + k)) ?? d } catch { return d } },
  set: (k, v) => localStorage.setItem('tm.' + k, JSON.stringify(v)),
};
let section = store.get('section', 'overview');
let sortK = store.get('sort', 'mem');
let cpuView = store.get('cpuView', 'all');
let sel = null; // {key, pid}
const open = new Set();
let startup = null, services = null, replaceState = null;
let svcReq = 0;
let dead = null; // set when system information can't be read at all
const pending = {};

/* ---------- toast ---------- */
let toastT;
function toast(msg, ok) {
  const t = $('#toast'); t.textContent = msg; t.className = 'toast on' + (ok ? ' ok' : '');
  clearTimeout(toastT); toastT = setTimeout(() => t.className = 'toast' + (ok ? ' ok' : ''), 4000);
}
async function call(cmd, args) {
  try { return await invoke(cmd, args) } catch (e) { toast(String(e)); throw e }
}

/* ---------- graphs ---------- */
function graph(a, cls, h, max, grid) {
  max = max || 1;
  if (a.length < 2) a = a.length ? [a[0], a[0]] : [0, 0];
  // History grows in from the right until 60 samples exist.
  const off = N - a.length, x0 = (off / (N - 1) * 300).toFixed(1);
  const pts = a.map((v, i) => ((off + i) / (N - 1) * 300).toFixed(1) + ',' + (h - Math.min(v, max) / max * h).toFixed(1));
  const l = 'M' + pts.join('L');
  const lines = grid ? [.25, .5, .75].map(f => '<line x1="0" x2="300" y1="' + h * f + '" y2="' + h * f + '"/>').join('') : '';
  return '<svg class="' + cls + '" viewBox="0 0 300 ' + h + '" preserveAspectRatio="none" aria-hidden="true">' + lines + '<path class="a" d="' + l + 'L300,' + h + 'L' + x0 + ',' + h + 'Z"/><path class="l" d="' + l + '"/></svg>';
}
const niceMax = a => { const m = Math.max(...a, 0); return m <= 0 ? 1 : m * 1.25 };

/* ---------- navigation ---------- */
function go(id) {
  if (!document.getElementById(id)) id = 'overview';
  section = id; store.set('section', id);
  $$('section').forEach(s => s.classList.toggle('on', s.id === id));
  $$('.nav').forEach(b => b.setAttribute('aria-current', b.dataset.go === id));
  $('main').scrollTop = 0;
  if (id === 'startup') loadStartup();
  if (id === 'services') loadServices();
  if (id === 'settings') loadSettings();
  render();
}
$$('.nav').forEach(b => b.onclick = () => go(b.dataset.go));

/* ---------- overview ---------- */
function renderOverview() {
  const s = snap, up = uptime(s.uptime);
  $('#ov-sub').textContent = 'Up ' + up.long + ', ' + s.apps.length + ' apps running';
  const temp = (v, w) => v == null ? '' : ' · <span class="' + tclass(v, w[0], w[1]) + '">' + Math.round(v) + '°C</span>';
  const tiles = [
    ['Processor', 'cpu', Math.round(s.cpu.usage) + '%', temp(s.cpu.temp, CPUT), 100],
    ['Memory', 'mem', gb(s.mem.used), ' of ' + gb(s.mem.total) + ' GB', 100],
    ['Graphics', 'gpu', s.gpu ? Math.round(s.gpu.usage) + '%' : '–', s.gpu ? temp(s.gpu.temp, GPUT) : '', 100],
    ['Network', 'net', (s.net.rx / MB).toFixed(1), ' MB/s', niceMax(H.net)],
  ];
  $('#tiles').innerHTML = tiles.map(t => '<button class="panel stat" style="text-align:left"><div class="k">' + t[0] + '</div><div class="v">' + t[2] + '<small>' + t[3] + '</small></div>'
    + graph(H[t[1]], 'spark', 40, t[4], false).replace('<svg', '<svg style="width:100%;height:40px"') + '</button>').join('');
  $$('#tiles .stat').forEach(b => b.onclick = () => go('perf'));
  const top = [...s.apps].sort((a, b) => b.mem - a.mem).slice(0, 6), mx = top.length ? top[0].mem : 1;
  $('#top').innerHTML = top.map(a => '<div class="row"><span class="n">' + esc(a.name) + '</span><div style="width:90px"><div class="bar"><i style="width:' + (a.mem / mx * 100) + '%"></i></div></div><span class="m">' + bytes(a.mem) + '</span></div>').join('')
    + '<button class="btn sm" style="margin-top:12px" id="allapps">See all apps</button>';
  $('#allapps').onclick = () => go('apps');
  $('#ov-drives').innerHTML = s.disks.map(d => '<div class="drive"><div class="t"><span>' + esc(d.letter) + '</span><span>' + Math.round(d.free / GB) + ' GB free of ' + Math.round(d.total / GB) + ' GB</span></div><div class="bar"><i style="width:' + ((d.total - d.free) / d.total * 100) + '%"></i></div></div>').join('');
}

/* ---------- apps ---------- */
const chevSvg = '<svg viewBox="0 0 12 12"><path d="M4 2l4 4-4 4"/></svg>';
function renderApps() {
  const q = $('#q').value.trim().toLowerCase();
  const netOk = snap.per_app_net;
  const list = snap.apps.filter(a => !q || a.name.toLowerCase().includes(q) || (a.exe || '').toLowerCase().includes(q) || a.procs.some(p => String(p.pid) === q))
    .sort((a, b) => (b[sortK] - a[sortK]) || a.name.localeCompare(b.name));
  const hot = k => sortK === k ? ' hot' : '';
  const net = v => netOk ? bits(v) : '–';
  let h = '<div class="tr h"><span>Name</span><span>CPU</span><span>Memory</span><span>GPU</span><span>Disk</span><span>Network</span></div>';
  if (!list.length) h += '<div class="empty">No app matches "' + esc(q) + '".</div>';
  for (const a of list) {
    const many = a.procs.length > 1, o = many && open.has(a.key);
    const isSel = sel && sel.key === a.key && sel.pid == null;
    h += '<button class="tr' + (isSel ? ' sel' : '') + '" data-key="' + esc(a.key) + '"><span class="name">'
      + (many ? '<span class="chev' + (o ? ' open' : '') + '" data-tog="1" aria-label="Expand">' + chevSvg + '</span>' : '<span class="chev"></span>')
      + '<span class="t">' + esc(a.name) + '</span>' + (many ? '<span class="cnt">' + a.procs.length + '</span>' : '') + '</span>'
      + '<span class="' + hot('cpu') + '">' + pct1(a.cpu) + '</span><span class="' + hot('mem') + '">' + bytes(a.mem) + '</span><span class="' + hot('gpu') + '">' + pct1(a.gpu) + '</span>'
      + '<span class="' + hot('disk') + '">' + rate(a.disk) + '</span><span class="' + hot('net') + '">' + net(a.net) + '</span></button>';
    if (o) {
      const kids = [...a.procs].sort((x, y) => (y[sortK] - x[sortK]) || x.pid - y.pid);
      for (const p of kids.slice(0, 60)) {
        const ps = sel && sel.pid === p.pid;
        h += '<div class="tr child' + (ps ? ' sel' : '') + '" tabindex="0" data-key="' + esc(a.key) + '" data-pid="' + p.pid + '"><span class="t" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">' + esc(p.name) + ' <span class="cnt">' + p.pid + '</span></span>'
          + '<span>' + pct1(p.cpu) + '</span><span>' + bytes(p.mem) + '</span><span>' + pct1(p.gpu) + '</span><span>' + rate(p.disk) + '</span><span>' + net(p.net) + '</span></div>';
      }
      if (kids.length > 60) h += '<div class="tr child"><span>and ' + (kids.length - 60) + ' more processes</span><span></span><span></span><span></span><span></span><span></span></div>';
    }
  }
  // Rebuilding replaces the row the keyboard focus is on; put focus back on the same row.
  const f = document.activeElement && document.activeElement.closest && document.activeElement.closest('#tbl [data-key]');
  const fk = f ? f.dataset.key + '|' + (f.dataset.pid || '') : null;
  $('#tbl').innerHTML = h;
  if (fk) { const r = [...$$('#tbl [data-key]')].find(x => x.dataset.key + '|' + (x.dataset.pid || '') === fk); if (r) r.focus({ preventScroll: true }) }
  $('#s-apps').textContent = snap.apps.length;
}
$('#tbl').addEventListener('click', e => {
  const row = e.target.closest('[data-key]'); if (!row) return;
  const key = row.dataset.key;
  if (e.target.closest('[data-tog]')) { open.has(key) ? open.delete(key) : open.add(key); renderApps(); return }
  sel = { key, pid: row.dataset.pid ? Number(row.dataset.pid) : null };
  renderApps(); renderDetail();
});
let detailSig = '';
// The processes the details panel stands for, as they are in the newest snapshot, each with the
// start time shown so the backend can tell them apart from a newer process with a reused pid.
function targets() {
  const a = sel && snap && snap.apps.find(x => x.key === sel.key);
  if (!a) return [];
  const ps = sel.pid != null ? a.procs.filter(x => x.pid === sel.pid) : a.procs;
  return ps.map(x => ({ pid: x.pid, start: x.start }));
}
function renderDetail() {
  const el = $('#detail');
  const a = sel && snap.apps.find(x => x.key === sel.key);
  const p = a && sel.pid != null ? a.procs.find(x => x.pid === sel.pid) : null;
  if (!a || (sel.pid != null && !p)) {
    if (detailSig !== 'none') { detailSig = 'none'; el.innerHTML = '<div class="sub">Select an app to see its details and actions.</div>' }
    return;
  }
  const it = p || a, netOk = snap.per_app_net, crit = p ? p.critical : a.critical;
  const pids = p ? [p.pid] : a.procs.map(x => x.pid);
  // Rebuild the panel (and its buttons) only when what it shows changes, so a click on
  // "End task" is never lost to the once-a-second refresh.
  const sig = [a.key, sel.pid, a.exe, crit, pids.length, netOk].join('|');
  if (sig !== detailSig) {
    detailSig = sig;
    el.innerHTML = '<h3>' + esc(p ? p.name : a.name) + '</h3><div class="sub" id="d-sub"></div>'
      + '<dl class="kv"><dt>Memory</dt><dd id="d-mem"></dd><dt>CPU</dt><dd id="d-cpu"></dd><dt>GPU</dt><dd id="d-gpu"></dd><dt>Disk</dt><dd id="d-disk"></dd><dt>Network</dt><dd id="d-net"></dd>'
      + (a.exe ? '<dt>Location</dt><dd>' + esc(a.exe).replace(/\\/g, '\\<wbr>') + '</dd>' : '') + '</dl>'
      + '<div class="btns">' + (a.exe ? '<button class="btn" id="loc">Open file location</button>' : '')
      + '<button class="btn danger" id="end"' + (crit ? ' disabled' : '') + '>' + (p ? 'End process' : 'End task') + '</button>'
      + '<button class="btn danger" id="tree"' + (crit ? ' disabled' : '') + '>End process tree' + (pids.length > 1 ? ' (' + pids.length + ')' : '') + '</button></div>'
      + (crit ? '<div class="note">Windows needs this to keep running, so it cannot be ended.</div>' : '');
    if ($('#loc')) $('#loc').onclick = () => call('open_location', { path: a.exe }).catch(() => {});
    $('#end').onclick = () => { const t = targets(); if (t.length) call('end_task', { targets: t }).catch(() => {}) };
    $('#tree').onclick = () => { const t = targets(); if (t.length) call('end_tree', { targets: t }).catch(() => {}) };
  }
  X('d-sub', p ? 'Process ' + p.pid + ' of ' + a.name : a.procs.length + ' process' + (a.procs.length > 1 ? 'es' : ''));
  X('d-mem', bytes(it.mem)); X('d-cpu', pct1(it.cpu)); X('d-gpu', pct1(it.gpu)); X('d-disk', rate(it.disk)); X('d-net', netOk ? bits(it.net) : 'not available');
}
$('#q').oninput = () => snap && renderApps();
function setSort(k) {
  sortK = k; store.set('sort', k);
  $$('#sort button').forEach(x => x.setAttribute('aria-pressed', x.dataset.k === k));
}
$$('#sort button').forEach(b => b.onclick = () => { setSort(b.dataset.k); if (snap) renderApps() });

/* ---------- performance ---------- */
let perfShape = '';
const fact = (k, id, v) => '<div><span>' + k + '</span><b' + (id ? ' id="' + id + '"' : '') + '>' + (v || '') + '</b></div>';
const box = (id, c) => '<div class="' + (c || '') + '" id="' + id + '"></div>';
function buildPerf() {
  const s = snap;
  perfShape = s.cpu.threads + '|' + s.disks.map(d => d.letter).join(',') + '|' + !!s.gpu;
  let h = '<div class="panel c-cpu"><div class="title-line"><div><h2>CPU</h2><div class="sub">' + esc(s.cpu.name) + ', ' + s.cpu.cores + ' cores, ' + s.cpu.threads + ' threads</div><div class="big-n" id="cpuv"></div></div>'
    + '<div class="seg" id="cpusw" role="group" aria-label="CPU view"><button data-v="all">Overall</button><button data-v="core">Per core</button></div></div>'
    + box('cpug') + '<div class="cores" id="cores">' + s.cpu.per_thread.map((_, i) => '<div class="core">CPU ' + i + '<b id="cv' + i + '"></b>' + box('cg' + i) + '</div>').join('') + '</div>'
    + '<div class="facts">' + fact('Speed', 'cpuf') + fact('Temperature', 'cput') + fact('Processes', 'cpup') + fact('Threads', 'cpuh') + fact('Up time', 'cpuu') + '</div></div>'
    + '<div class="panel c-ram"><div class="title-line"><div><h2>Memory</h2><div class="sub">' + gb(s.mem.total) + ' GB total</div><div class="big-n" id="ramv"></div></div></div>'
    + box('ramg') + '<div class="bar pbar"><i id="rambar"></i></div>'
    + '<div class="facts">' + fact('Available', 'ramav') + fact('Committed', 'ramco') + fact('Cached', 'ramca') + '</div></div>';
  h += s.gpu
    ? '<div class="panel c-gpu"><div class="title-line"><div><h2>GPU</h2><div class="sub">' + esc(s.gpu.name) + (s.gpu.vram_total ? ', ' + Math.round(s.gpu.vram_total / GB) + ' GB' : '') + '</div><div class="big-n" id="gpuv"></div></div></div>'
      + box('gpug') + '<div class="facts">' + fact('Video memory', 'gpum') + fact('Clock', 'gpuc') + fact('Fan', 'gpuf') + fact('Temperature', 'gput') + '</div></div>'
    : '<div class="panel c-gpu"><h2>GPU</h2><div class="sub">No graphics card found.</div></div>';
  h += '<div class="panel c-net"><div class="title-line"><div><h2>Network</h2><div class="sub" id="netsub"></div><div class="big-n" id="netv"></div></div></div>'
    + box('netg') + '<div class="facts">' + fact('Receive', 'netrx') + fact('Send', 'nettx') + '</div></div>';
  h += s.disks.map((d, i) => '<div class="panel c-drv"><div class="title-line" style="margin-bottom:0"><div><h2>' + esc(d.letter) + '</h2><div class="sub">' + Math.round(d.total / GB) + ' GB</div></div></div>'
    + '<div class="bar pbar"><i id="du' + i + '"></i></div>' + box('dg' + i, 's')
    + '<div class="facts">' + fact('Free', 'df' + i) + fact('Active', 'da' + i) + fact('Temperature', 'dt' + i) + '</div></div>').join('');
  $('#pbody').innerHTML = h;
  $$('#cpusw button').forEach(b => b.onclick = () => { cpuView = b.dataset.v; store.set('cpuView', cpuView); applyCpuView() });
  applyCpuView();
}
function applyCpuView() {
  $$('#cpusw button').forEach(x => x.setAttribute('aria-pressed', x.dataset.v === cpuView));
  const core = cpuView === 'core';
  $('#cores').classList.toggle('on', core); $('#cpug').style.display = core ? 'none' : '';
}
const G = (id, a, max, grid) => { const e = document.getElementById(id); if (e) e.innerHTML = graph(a, 'big', 100, max || 100, grid) };
const X = (id, t) => { const e = document.getElementById(id); if (e) e.textContent = t };
function T(id, v, w) {
  const e = document.getElementById(id); if (!e) return;
  if (v == null) { e.textContent = 'n/a'; e.className = 'na'; return }
  e.textContent = Math.round(v) + '°C'; e.className = tclass(v, w[0], w[1]);
}
function updatePerf() {
  const s = snap;
  if (perfShape !== s.cpu.threads + '|' + s.disks.map(d => d.letter).join(',') + '|' + !!s.gpu) buildPerf();
  G('cpug', H.cpu, 100, true); X('cpuv', Math.round(s.cpu.usage) + '%');
  X('cpuf', s.cpu.mhz ? (s.cpu.mhz / 1000).toFixed(2) + ' GHz' : 'n/a');
  T('cput', s.cpu.temp, CPUT);
  const ct = document.getElementById('cput');
  if (ct && s.cpu.temp == null) ct.title = s.cpu_temp_status === 'NoDriver' ? 'Install the sensor driver in Settings' : s.cpu_temp_status === 'Unsupported' ? 'Only AMD Ryzen CPUs are supported' : '';
  X('cpup', s.cpu.processes.toLocaleString('en-US')); X('cpuh', s.cpu.thread_count.toLocaleString('en-US')); X('cpuu', uptime(s.uptime).short);
  if (cpuView === 'core') H.threads.forEach((a, i) => { G('cg' + i, a); X('cv' + i, Math.round(a[a.length - 1]) + '%') });
  G('ramg', H.mem, 100, true); X('ramv', gb(s.mem.used) + ' GB'); X('ramav', gb(s.mem.available) + ' GB');
  X('ramco', Math.round(s.mem.committed / GB) + ' / ' + Math.round(s.mem.commit_limit / GB) + ' GB'); X('ramca', gb(s.mem.cached) + ' GB');
  $('#rambar').style.width = (s.mem.used / s.mem.total * 100) + '%';
  if (s.gpu) {
    G('gpug', H.gpu, 100, true); X('gpuv', Math.round(s.gpu.usage) + '%');
    X('gpum', s.gpu.vram_used != null && s.gpu.vram_total ? gb(s.gpu.vram_used) + ' of ' + Math.round(s.gpu.vram_total / GB) + ' GB' : 'n/a');
    X('gpuc', s.gpu.clock_mhz ? s.gpu.clock_mhz.toLocaleString('en-US') + ' MHz' : 'n/a');
    X('gpuf', s.gpu.fan_rpm != null ? s.gpu.fan_rpm.toLocaleString('en-US') + ' RPM' : 'n/a');
    T('gput', s.gpu.temp, GPUT);
  }
  X('netsub', (s.net.adapter || 'No network adapter') + (s.net.link_mbps ? ', connected at ' + (s.net.link_mbps >= 1000 ? (s.net.link_mbps / 1000) + ' Gbps' : s.net.link_mbps + ' Mbps') : ''));
  G('netg', H.net, niceMax(H.net), true); X('netv', rate(s.net.rx + s.net.tx)); X('netrx', rate(s.net.rx)); X('nettx', rate(s.net.tx));
  s.disks.forEach((d, i) => {
    const u = document.getElementById('du' + i); if (u) u.style.width = ((d.total - d.free) / d.total * 100) + '%';
    G('dg' + i, H.disks[d.letter] || []); X('df' + i, Math.round(d.free / GB) + ' GB'); X('da' + i, Math.round(d.active) + '%'); T('dt' + i, d.temp, DRVT);
  });
}

/* ---------- startup ---------- */
let stReq = 0;
async function loadStartup() {
  const n = ++stReq;
  let list;
  try { list = await invoke('startup_list') } catch (e) { if (n === stReq) toast(String(e)); list = null }
  if (n !== stReq) return; // a newer request is on its way
  startup = list || startup || [];
  renderStartup();
}
function renderStartup() {
  const on = startup ? startup.filter(s => s.enabled).length : 0;
  $('#s-start').textContent = startup ? on + ' on' : '';
  if (!startup) return;
  const imp = { High: 3, Medium: 2, Low: 1 };
  $('#stbl').innerHTML = '<div class="lrow h"><span>Name</span><span>Boot impact</span><span>Run at startup</span></div>'
    + (startup.length ? startup.map((s, i) => '<div class="lrow"><div style="min-width:0"><div>' + esc(s.name) + '</div><div class="d" title="' + esc(s.command) + '">' + esc(s.publisher || s.command) + '</div></div>'
      + '<span>' + (s.impact ? '<span class="imp">' + [1, 2, 3].map(n => '<i class="' + (n <= imp[s.impact] ? 'on' : '') + '"></i>').join('') + '</span>' + s.impact : '<span class="na">Not measured</span>') + '</span>'
      + '<span><button class="sw" role="switch" aria-checked="' + s.enabled + '" aria-label="Run ' + esc(s.name) + ' at startup" data-i="' + i + '"></button></span></div>').join('')
      : '<div class="empty">No apps start with Windows.</div>');
  $$('#stbl .sw').forEach(b => {
    const s = startup[b.dataset.i];
    if (pending['st:' + s.id]) b.disabled = true;
    b.onclick = async () => {
      if (pending['st:' + s.id]) return;
      pending['st:' + s.id] = 1; s.enabled = !s.enabled; renderStartup();
      try { await invoke('startup_set', { id: s.id, enabled: s.enabled }) } catch (e) { toast(String(e)) }
      delete pending['st:' + s.id];
      await loadStartup(); // show what Windows actually has now
    };
  });
}

/* ---------- services ---------- */
async function loadServices() {
  const n = ++svcReq;
  let list;
  try { list = await invoke('services_list') } catch (e) { if (n === svcReq) toast(String(e)); list = null }
  if (n !== svcReq) return; // a newer request is on its way
  services = list || services || [];
  renderServices();
}
function renderServices() {
  if (!services) return;
  $('#s-svc').textContent = services.filter(s => s.running).length + ' on';
  const q = $('#sq').value.trim().toLowerCase();
  const l = services.filter(s => !q || s.name.toLowerCase().includes(q) || s.id.toLowerCase().includes(q) || s.description.toLowerCase().includes(q));
  $('#vtbl').innerHTML = '<div class="lrow svc h"><span>Name</span><span>Status</span><span></span></div>' + (l.length ? l.map(s => {
    const p = pending[s.id];
    let btn;
    if (p) btn = '<button class="btn sm" disabled>' + p + '</button>';
    else if (s.running) btn = '<button class="btn sm" data-id="' + esc(s.id) + '" data-run="0"' + (s.can_stop ? '' : ' disabled title="This service cannot be stopped"') + '>Stop</button>';
    else btn = s.start_type === 'Disabled' ? '<span class="na">Disabled</span>' : '<button class="btn sm" data-id="' + esc(s.id) + '" data-run="1">Start</button>';
    return '<div class="lrow svc"><div style="min-width:0"><div>' + esc(s.name) + '</div><div class="d" title="' + esc(s.description) + '">' + esc(s.description || s.id) + '</div></div><span><span class="dot' + (s.running ? ' run' : '') + '"></span>' + (s.running ? 'Running' : 'Stopped') + '</span><span>' + btn + '</span></div>';
  }).join('') : '<div class="empty">No service matches that name.</div>');
}
$('#vtbl').addEventListener('click', async e => {
  const b = e.target.closest('button[data-id]'); if (!b) return;
  const id = b.dataset.id, run = b.dataset.run === '1';
  pending[id] = run ? 'Starting…' : 'Stopping…'; renderServices();
  try { await invoke('service_set', { id, run }) } catch (err) { toast(String(err)) }
  delete pending[id];
  await loadServices();
});
$('#sq').oninput = renderServices;

/* ---------- settings ---------- */
let isAdmin = true;
async function loadSettings() {
  try { replaceState = await invoke('replace_get') } catch { replaceState = null }
  try { isAdmin = await invoke('is_admin') } catch { isAdmin = true }
  renderSettings();
}
function renderSettings() {
  const st = snap ? snap.cpu_temp_status : null;
  let drv;
  if (st === 'Ok') drv = '<span>Working' + (snap.cpu.temp != null ? ', ' + Math.round(snap.cpu.temp) + '°C' : '') + '</span>';
  else if (st === 'NoDriver') drv = '<button class="btn sm" id="drv">Install driver</button>';
  else if (st === 'Unsupported') drv = '<span class="na">Not supported</span>';
  else if (st === 'Failed') drv = '<span class="na">Could not read</span>';
  else drv = '<span class="na">Checking…</span>';
  const drvText = st === 'Unsupported' ? 'Reading the CPU temperature is only supported on AMD Ryzen processors.'
    : st === 'Failed' ? 'The PawnIO driver is installed, but the temperature could not be read. Restarting Windows usually fixes this.'
    : 'Windows has no built-in way to read the CPU temperature. The free, signed PawnIO driver (also used by LibreHardwareMonitor) makes it possible.';
  const rs = replaceState && replaceState.state, on = rs === 'On';
  const replText = rs === 'Other'
    ? 'Another program is set to open instead of Windows Task Manager (' + esc(replaceState.path) + '). Turning this on replaces it with this app.'
    : 'Ctrl+Shift+Esc and "Task Manager" in the taskbar menu open this app. Turn it off to get the Windows one back.';
  $('#settbl').innerHTML =
    '<div class="set"><div><div>Open this instead of Windows Task Manager</div><div class="d">' + replText + '</div></div>'
    + '<button class="sw" role="switch" id="repl" aria-label="Open this instead of Windows Task Manager" aria-checked="' + on + '"' + (rs ? '' : ' disabled') + '></button></div>'
    + '<div class="set"><div><div>CPU temperature</div><div class="d">' + drvText + '</div></div>' + drv + '</div>'
    + (isAdmin ? '' : '<div class="set"><div><div>Running without administrator rights</div><div class="d">Administrator rights were declined, so ending some processes, services, per-app network and the CPU temperature don\'t work right now.</div></div><button class="btn sm" id="admin">Restart as administrator</button></div>');
  if ($('#admin')) $('#admin').onclick = () => call('restart_as_admin').catch(() => {});
  $('#repl').onclick = async () => {
    $('#repl').disabled = true;
    try { replaceState = await invoke('replace_set', { on: !on }); toast(replaceState.state === 'On' ? 'Ctrl+Shift+Esc now opens this app.' : 'Windows Task Manager is back.', true) } catch (e) { toast(String(e)) }
    renderSettings();
  };
  if ($('#drv')) $('#drv').onclick = async () => {
    $('#drv').disabled = true; $('#drv').textContent = 'Installing…';
    try { await invoke('install_driver'); toast('Driver installer finished. The temperature shows up within 15 seconds.', true) } catch (e) { toast(String(e)) }
    renderSettings();
  };
}

/* ---------- main loop ---------- */
// A click is mouse-down + mouse-up on the same element; replacing the rows in between would
// swallow it. While a button is held, defer the rebuild until it is released.
let holding = false, missed = false;
addEventListener('pointerdown', () => { holding = true }, true);
addEventListener('pointerup', () => { holding = false; if (missed) { missed = false; setTimeout(render, 0) } }, true);
const release = () => { holding = false; if (missed) { missed = false; setTimeout(render, 0) } };
addEventListener('pointercancel', release, true);
addEventListener('blur', release);
addEventListener('contextmenu', release, true);
addEventListener('pointermove', e => { if (holding && e.buttons === 0) release() }, true);
function render() {
  // Startup, Services and Settings don't need live data and keep working.
  if (dead) { if (section === 'overview' || section === 'apps' || section === 'perf') fatal(dead); return }
  if (!snap) return;
  if (holding) { missed = true; return }
  $('#s-cpu').textContent = Math.round(snap.cpu.usage) + '%';
  $('#s-apps').textContent = snap.apps.length;
  if (section === 'overview') renderOverview();
  else if (section === 'apps') { renderApps(); renderDetail() }
  else if (section === 'perf') updatePerf();
}
function onSnap(s) {
  const first = !snap;
  snap = s;
  push(H.cpu, s.cpu.usage); push(H.mem, s.mem.used / s.mem.total * 100); push(H.gpu, s.gpu ? s.gpu.usage : 0); push(H.net, s.net.rx + s.net.tx);
  s.cpu.per_thread.forEach((v, i) => { H.threads[i] = H.threads[i] || []; push(H.threads[i], v) });
  s.disks.forEach(d => { H.disks[d.letter] = H.disks[d.letter] || []; push(H.disks[d.letter], d.active) });
  if (first) $('#cpu-name').textContent = s.cpu.name;
  // the settings page shows the live driver status; only re-render it when that changes
  if (section === 'settings' && (first || s.cpu_temp_status !== renderSettings.last)) { renderSettings.last = s.cpu_temp_status; renderSettings() }
  render();
}

setSort(sortK);
go(section);
loadStartup();
loadServices();
function fatal(msg) {
  dead = msg; section = 'overview';
  $$('section').forEach(s => s.classList.toggle('on', s.id === 'overview'));
  $$('.nav').forEach(b => b.setAttribute('aria-current', b.dataset.go === 'overview'));
  $('#overview').classList.add('on');
  $('#ov-sub').textContent = msg;
  $('#tiles').innerHTML = ''; $('#top').innerHTML = ''; $('#ov-drives').innerHTML = ''; // stale numbers would look live
}
listen('snapshot', e => onSnap(e.payload));
listen('fatal', e => fatal(String(e.payload)));
invoke('snapshot').then(s => { if (s && !snap) onSnap(s) }).catch(e => fatal(String(e)));
