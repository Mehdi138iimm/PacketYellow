// PacketYellow v0.5.1 (beta) frontend.
// IMPORTANT: every measurement happens in Rust. This file never times anything; it only renders.
const T = window.__TAURI__;
const IN_APP = !!(T && T.core);
const rawInvoke = (cmd, args) => IN_APP ? T.core.invoke(cmd, args) : Promise.reject('این بخش فقط داخل اپ PacketYellow کار می‌کنه');
// every command goes through here: failures and slow calls land in the «لاگ» tab
const QUIET = new Set(['get_logs', 'clear_logs', 'log_from_ui', 'stop_monitor', 'speed_cancel', 'game_live_stop']);
async function invoke(cmd, args) {
  const t0 = performance.now();
  try {
    const r = await rawInvoke(cmd, args);
    const ms = performance.now() - t0;
    if (!QUIET.has(cmd) && ms > 15000) uiLog('warn', 'ui', `فرمان ${cmd} خیلی طول کشید (${(ms / 1000).toFixed(1)} ثانیه)`);
    return r;
  } catch (e) {
    if (!QUIET.has(cmd)) uiLog('error', 'ui', `فرمان ${cmd} شکست خورد: ${errText(e)}`);
    throw e;
  }
}
const errText = e => e == null ? 'نامشخص' : typeof e === 'string' ? e : e.message || (() => { try { return JSON.stringify(e); } catch { return String(e); } })();
const listen = (ev, cb) => IN_APP ? T.event.listen(ev, cb) : Promise.resolve(() => {});

const api = {
  pingMany: (targets, count = 6) => invoke('ping_many', { targets, count }),
  checkSites: urls => invoke('check_sites', { urls }),
  dns: resolvers => invoke('dns_benchmark', { resolvers }),
  netInfo: () => invoke('network_info'),
  steam: () => invoke('steam_servers'),
  speedCancel: () => invoke('speed_cancel'),
  async speed(budgetMb, onProgress) {
    const un = await listen('speed://progress', e => onProgress(e.payload));
    try { return await invoke('speed_test', { budgetMb }); } finally { un(); }
  },
  // returns { un, stop }: `un` only detaches the listener, `stop` also stops the Rust monitor
  async startMonitor(t, mode, onTick) {
    const key = mode === 'http' ? t.url : t.host;
    const un = await listen('monitor://tick', e => { if (e.payload.key === key) onTick(e.payload.rttMs); });
    try { await invoke('start_monitor', { host: t.host, port: t.port, url: t.url, mode, intervalMs: 1000 }); }
    catch (e) { un(); throw e; }
    return { un, stop: async () => { un(); await invoke('stop_monitor').catch(() => {}); } };
  },
};

/* ---------- in-app log ---------- */
const LOG_CAP = 1500;
let logs = [], logLvl = 'all', logUnseenErr = 0;
function addLog(e) {
  logs.push(e); if (logs.length > LOG_CAP) logs.splice(0, logs.length - LOG_CAP);
  if (e.level === 'error' && document.querySelector('.nb[aria-selected=true]')?.dataset.tab !== 'logs') logUnseenErr++;
  scheduleLogRender();
}
function uiLog(level, src, msg) {
  msg = String(msg);
  if (IN_APP) rawInvoke('log_from_ui', { level, src, msg }).catch(() => addLog({ ts: Date.now(), level, src, msg }));
  else addLog({ ts: Date.now(), level, src, msg });
}
window.addEventListener('error', e => uiLog('error', 'js', `${e.message} @ ${(e.filename || '').split('/').pop()}:${e.lineno}:${e.colno}`));
window.addEventListener('unhandledrejection', e => uiLog('error', 'js', 'Promise رد شد: ' + errText(e.reason)));
{
  const ce = console.error.bind(console), cw = console.warn.bind(console);
  console.error = (...a) => { ce(...a); uiLog('error', 'console', a.map(errText).join(' ')); };
  console.warn = (...a) => { cw(...a); uiLog('warn', 'console', a.map(errText).join(' ')); };
}

/* ---------- helpers ---------- */
const $ = id => document.getElementById(id);
const q = ms => ms == null ? 'bad' : ms < 60 ? 'good' : ms < 120 ? 'warn' : 'bad';
const f = (v, d = 0) => v == null ? '--' : (+v).toFixed(d);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
let iconRaf = 0;
const icons = () => { if (iconRaf || !window.lucide) return; iconRaf = requestAnimationFrame(() => { iconRaf = 0; lucide.createIcons(); }); };
function toast(msg) { const t = $('toast'); t.querySelector('span').textContent = msg; t.classList.add('show'); clearTimeout(t._h); t._h = setTimeout(() => t.classList.remove('show'), 2600); }
async function busy(btn, fn) {
  if (btn.disabled) return;
  const html = btn.innerHTML; btn.disabled = true; btn.innerHTML = '<span class="spin"></span>در حال تست';
  try { await fn(); } catch (e) { if (e instanceof Error) uiLog('error', 'js', (e.stack || e.message).split('\n').slice(0, 3).join(' | ')); toast('خطا: ' + errText(e)); } finally { btn.disabled = false; btn.innerHTML = html; icons(); }
}
const emptyRow = (cols, icon, text) => `<tr><td colspan="${cols}"><div class="empty"><i data-lucide="${icon}" width="22" height="22"></i>${text}</div></td></tr>`;
const store = {
  get: (k, d) => { try { return JSON.parse(localStorage.getItem('py:' + k)) ?? d; } catch { return d; } },
  set: (k, v) => { try { localStorage.setItem('py:' + k, JSON.stringify(v)); } catch {} },
};
const copy = (text, msg) => { navigator.clipboard?.writeText(text).then(() => toast(msg || 'کپی شد'), () => toast('کپی نشد')); };

/* ---------- tabs + shortcuts ---------- */
const tabs = [...document.querySelectorAll('.nb')];
let curTab = 'dash';
function show(id) {
  curTab = id;
  tabs.forEach(b => b.setAttribute('aria-selected', b.dataset.tab === id));
  document.querySelectorAll('.tab').forEach(t => t.classList.toggle('on', t.id === 'tab-' + id));
  if (id === 'dash') drawMonitor();
  if (id === 'logs') { logUnseenErr = 0; renderLogs(true); }
  if (id === 'tunnel') renderVpn();
  if (id === 'games') scheduleLive();
  if (id === 'settings') renderSettings();
  updLogBadge();
}
tabs.forEach(b => b.addEventListener('click', () => show(b.dataset.tab)));
document.addEventListener('click', e => { const g = e.target.closest('[data-goto]'); if (g) show(g.dataset.goto); });
// no zoom: Ctrl + wheel / Ctrl +,-,0 (the window itself is not maximizable either)
window.addEventListener('wheel', e => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });
document.addEventListener('keydown', e => { if ((e.ctrlKey || e.metaKey) && ['+', '=', '-', '_', '0'].includes(e.key)) e.preventDefault(); }, true);
document.addEventListener('gesturestart', e => e.preventDefault());
document.addEventListener('keydown', e => {
  if (!$('onb').hidden) return onbKey(e);
  // number keys must not switch tabs while typing or behind an open dialog
  if (!$('gModal').hidden || e.target.matches('input,textarea,select,[contenteditable]') || e.ctrlKey || e.altKey || e.metaKey) return;
  if (!/^[0-9]$/.test(e.key)) return;
  const i = e.key === '0' ? 10 : +e.key; if (i >= 1 && i <= tabs.length) show(tabs[i - 1].dataset.tab);
});

/* ---------- window controls (decorations: false) ---------- */
document.querySelectorAll('[data-win]').forEach(b => b.addEventListener('click', () => {
  if (!IN_APP) return;
  Promise.resolve(T.window.getCurrentWindow()[b.dataset.win]()).catch(e => toast('خطا: ' + errText(e)));
}));

/* ---------- live monitor ---------- */
const TARGETS = [
  { name: 'Cloudflare', host: '1.1.1.1', port: 443, url: 'https://www.cloudflare.com/cdn-cgi/trace' },
  { name: 'Google', host: '8.8.8.8', port: 443, url: 'https://www.google.com/generate_204' },
  { name: 'Microsoft', host: 'www.msftconnecttest.com', port: 80, url: 'http://www.msftconnecttest.com/connecttest.txt' },
  { name: 'Steam', host: 'api.steampowered.com', port: 443, url: 'https://api.steampowered.com/' },
  { name: 'Aparat', host: 'www.aparat.com', port: 443, url: 'https://www.aparat.com/' },
  { name: 'Digikala', host: 'www.digikala.com', port: 443, url: 'https://www.digikala.com/' },
];
let customTarget = store.get('customTarget', null);
const allTargets = () => customTarget ? [...TARGETS, customTarget] : TARGETS;
let curName = store.get('targetName', 'Cloudflare');
let cur = allTargets().find(t => t.name === curName) || TARGETS[0];
// modePref: what the user picked (auto | tcp | http). mode: what is actually running.
let modePref = store.get('modePref', 'auto');
if (!['auto', 'tcp', 'http'].includes(modePref)) modePref = 'auto';
let autoVpn = false;       // result of VPN detection (full network_info + quick local check)
let quickHint = false;     // quick local check: system proxy / TUN adapter on the default route / Fake-IP
const effMode = () => modePref === 'auto' ? (autoVpn ? 'http' : 'tcp') : modePref;
let mode = effMode();
let samples = [], logItems = [], mon = null, running = true, gen = 0;
const N = 90;

function parseTarget(v) {
  v = v.trim(); if (!v) return null;
  if (/^https?:\/\//i.test(v)) {
    try { const u = new URL(v); return { name: 'دلخواه', host: u.hostname.replace(/^\[|\]$/g, ''), port: +u.port || (u.protocol === 'http:' ? 80 : 443), url: v, custom: true }; } catch { return null; }
  }
  const m = v.match(/^\[?([^\]\s]+?)\]?(?::(\d{1,5}))?$/); if (!m) return null;
  const port = +(m[2] || 443);
  return { name: 'دلخواه', host: m[1], port, url: `${port === 80 ? 'http' : 'https'}://${m[1].includes(':') ? '[' + m[1] + ']' : m[1]}${port === 80 || port === 443 ? '' : ':' + port}/`, custom: true };
}
function renderTargets() {
  $('targets').innerHTML = allTargets().map((t, i) => `<button class="tgt" aria-pressed="${t.name === cur.name}" data-i="${i}"><span><b>${esc(t.name)}</b><small>${esc(mode === 'http' ? t.url : t.host + ':' + t.port)}</small></span></button>`).join('');
  $('targets').querySelectorAll('.tgt').forEach(b => b.onclick = () => { cur = allTargets()[+b.dataset.i]; store.set('targetName', cur.name); renderTargets(); restart(); });
}
function renderMode() {
  const auto = modePref === 'auto';
  $('modeSeg').querySelectorAll('button').forEach(b => {
    b.setAttribute('aria-checked', b.dataset.m === modePref);
    b.classList.toggle('eff', auto && b.dataset.m === mode);
  });
  $('mMode').textContent = (auto ? 'A·' : '') + mode.toUpperCase();
  $('mMode').title = auto ? 'حالت خودکار' : '';
  const how = mode === 'http' ? 'درخواست HTTP روی اتصال باز (از داخل VPN/پروکسی)' : 'پینگ TCP از هسته‌ی Rust';
  $('modeNote').textContent = auto ? `خودکار: ${autoVpn ? 'VPN روشنه ← HTTP' : 'VPN خاموشه ← TCP'} · ${how}` : how;
  $('vpnHint').hidden = !(netInfo?.vpn && modePref === 'tcp');
}
// switch the running monitor only if the effective mode really changed
function applyMode(announce) {
  const m = effMode();
  renderMode();
  if (m === mode) return;
  mode = m; renderMode(); renderTargets(); restart();
  if (announce) toast(autoVpn ? 'VPN تشخیص داده شد · رفت روی HTTP' : 'VPN خاموشه · برگشت روی TCP');
}
function setMode(m) { if (m === modePref) return; modePref = m; store.set('modePref', m); applyMode(false); }
function setAutoVpn(v, announce) {
  v = !!v; const changed = v !== autoVpn; autoVpn = v;
  if (changed) uiLog('info', 'ui', `تشخیص خودکار VPN: ${v ? 'روشن' : 'خاموش'}${modePref === 'auto' ? ` → حالت ${v ? 'HTTP' : 'TCP'}` : ''}`);
  applyMode(announce && modePref === 'auto');
}
$('modeSeg').querySelectorAll('button').forEach(b => b.onclick = () => setMode(b.dataset.m));
$('vpnHintBtn').onclick = () => setMode('auto');
$('tgtAdd').onclick = () => {
  const t = parseTarget($('tgtIn').value); if (!t) return toast('آدرس نامعتبره');
  customTarget = t; store.set('customTarget', t); cur = t; store.set('targetName', t.name); $('tgtIn').value = ''; renderTargets(); restart();
};
$('tgtIn').onkeydown = e => { if (e.key === 'Enter') $('tgtAdd').click(); };

function stats() {
  if (!samples.length) return null;
  const ok = samples.filter(v => v != null);
  let j = 0; for (let i = 1; i < ok.length; i++) j += Math.abs(ok[i] - ok[i - 1]);
  const sorted = [...ok].sort((a, b) => a - b), n = sorted.length;
  return {
    now: samples[samples.length - 1],
    avg: n ? ok.reduce((a, b) => a + b, 0) / n : null,
    med: n ? (n % 2 ? sorted[(n - 1) / 2] : (sorted[n / 2 - 1] + sorted[n / 2]) / 2) : null,
    jit: n > 1 ? j / (n - 1) : null,
    loss: (samples.length - n) / samples.length * 100,
    min: n ? sorted[0] : null, max: n ? sorted[n - 1] : null,
  };
}
function verdict(s) {
  if (s.avg == null) return ['bad', 'قطع', 'هیچ پاسخی از سرور نیومد'];
  if (s.loss > 5 || s.avg > 150) return ['bad', 'اتصال ضعیف', 'برای بازی آنلاین مناسب نیست'];
  if ((s.jit ?? 0) > 20 || s.avg > 90 || s.loss > 1) return ['warn', 'قابل قبول', 'برای وب خوبه، بازی رقابتی ممکنه لگ بزنه'];
  return ['good', 'عالی', 'برای بازی و تماس تصویری آماده‌ست'];
}
function drawMonitor() {
  const s = stats();
  if (!s) {
    ['dNow', 'dAvg', 'dMed', 'dJit', 'dLoss', 'dMM', 'mPing', 'mJit', 'mLoss', 'sbPing'].forEach(k => $(k).textContent = '--');
    $('chart').innerHTML = ''; $('chartAxis').innerHTML = ''; $('log').innerHTML = '';
    $('spark').querySelector('polyline').setAttribute('points', '');
    $('vTitle').textContent = running ? 'در حال سنجش' : 'متوقف'; $('vTitle').className = ''; $('vSub').textContent = running ? 'چند ثانیه صبر کن' : 'برای ادامه «ادامه» رو بزن';
    return;
  }
  const nowTxt = s.now == null ? 'LOSS' : Math.round(s.now);
  // sidebar + status bar always update
  $('mPing').textContent = nowTxt; $('mPing').className = 'num ' + q(s.now);
  $('mJit').textContent = f(s.jit, 1) + ' ms'; $('mLoss').textContent = f(s.loss, 1) + '%';
  $('sbPing').textContent = s.now == null ? 'loss' : Math.round(s.now) + ' ms';
  const sp = samples.slice(-40), sm = Math.max(50, ...sp.filter(v => v != null));
  $('spark').querySelector('polyline').setAttribute('points', sp.map((v, i) => `${i * 200 / Math.max(1, sp.length - 1)},${v == null ? 27 : 27 - v / sm * 25}`).join(' '));
  if (curTab !== 'dash') return; // skip heavy chart work when hidden

  $('dNow').textContent = nowTxt; $('dNow').className = 'num ' + q(s.now);
  $('dAvg').textContent = f(s.avg, 1); $('dMed').textContent = f(s.med, 1); $('dJit').textContent = f(s.jit, 1);
  $('dLoss').textContent = f(s.loss, 1) + '%'; $('dLoss').className = 'num ' + (s.loss < 1 ? '' : s.loss < 3 ? 'warn' : 'bad');
  $('dMM').textContent = s.min == null ? '--' : Math.round(s.min) + ' / ' + Math.round(s.max);
  const [cls, t, sub] = verdict(s); $('vTitle').textContent = t; $('vTitle').className = cls; $('vSub').textContent = sub;

  const W = 1000, H = 220, max = Math.max(80, (s.max ?? 0) * 1.15), step = W / (N - 1), off = N - samples.length, y = v => H - v / max * H;
  $('chartScale').textContent = 'max ' + Math.round(max) + ' ms';
  let g = '<g class="g">', axis = '';
  [.25, .5, .75].forEach(k => { const v = Math.round(max * k); g += `<line x1="0" x2="${W}" y1="${y(v)}" y2="${y(v)}"/>`; axis += `<span style="top:${y(v) / H * 100}%">${v}</span>`; });
  g += '</g>';
  const segs = []; let c = [], lost = '';
  samples.forEach((v, i) => { const x = (i + off) * step; if (v == null) { lost += `<line class="lost" x1="${x}" x2="${x}" y1="0" y2="${H}"/>`; if (c.length) segs.push(c); c = []; } else c.push([x, y(v)]); });
  if (c.length) segs.push(c);
  let p = ''; segs.forEach(sg => { const pts = sg.map(a => a.join(',')).join(' '); p += `<polygon class="area" points="${sg[0][0]},${H} ${pts} ${sg[sg.length - 1][0]},${H}"/><polyline class="ln" points="${pts}"/>`; });
  $('chart').innerHTML = g + p + lost; $('chartAxis').innerHTML = axis;
  $('log').innerHTML = logItems.map(it => `<li><span class="t">${it.t}</span><span class="bar"><span style="width:${it.v == null ? 100 : Math.min(100, it.v / 1.6)}%;background:var(--${q(it.v)})"></span></span><span class="v ${q(it.v)}">${it.v == null ? 'loss' : Math.round(it.v) + 'ms'}</span></li>`).join('');
}
function onTick(v) {
  samples.push(v); if (samples.length > N) samples.shift();
  logItems.unshift({ v, t: new Date().toLocaleTimeString('en-GB') }); if (logItems.length > 10) logItems.length = 10;
  drawMonitor();
  $('hdot').className = 'sdot ' + (v == null ? 'off' : 'on'); $('hstat').textContent = v == null ? 'پکت گم شد' : 'متصل';
}
// generation counter prevents races when the user clicks targets quickly
async function restart() {
  const my = ++gen;
  if (mon) { const m = mon; mon = null; await m.stop(); }
  samples = []; logItems = []; drawMonitor();
  $('chartTarget').textContent = `${cur.name} · ${mode === 'http' ? cur.url : cur.host + ':' + cur.port}`;
  if (!running) return;
  try {
    const m = await api.startMonitor(cur, mode, onTick);
    if (my !== gen) { m.un(); return; } // a newer restart already owns the Rust monitor
    mon = m;
  } catch (e) { if (my === gen) { toast('خطا: ' + errText(e)); $('hdot').className = 'sdot off'; $('hstat').textContent = 'خطا'; } }
}
$('monToggle').onclick = () => {
  running = !running;
  $('monToggle').innerHTML = running ? '<i data-lucide="pause" width="15" height="15"></i><span>توقف</span>' : '<i data-lucide="play" width="15" height="15"></i><span>ادامه</span>';
  icons(); restart();
};

/* ---------- sites ---------- */
const QUICK = ['google.com', 'github.com', 'youtube.com', 'instagram.com', 'chatgpt.com', 'telegram.org', 'x.com', 'whatsapp.com', 'digikala.com', 'aparat.com', 'wikipedia.org', 'spotify.com', 'discord.com', 'reddit.com', 'twitch.tv', 'store.steampowered.com', 'play.google.com', 'developer.android.com', 'docker.com', 'npmjs.com'];
const S_LABEL = {
  ok: ['ok', 'باز'], slow: ['slow', 'کُند'], restricted: ['slow', 'تحریم / محدود'], server_error: ['slow', 'خطای سرور'],
  filtered: ['no', 'فیلتر (DNS)'], tls_fail: ['no', 'فیلتر (SNI/TLS)'], tcp_fail: ['no', 'اتصال برقرار نشد'],
  dns_fail: ['no', 'DNS پیدا نکرد'], timeout: ['no', 'تایم‌اوت'], invalid: ['no', 'آدرس نامعتبر'],
};
let sites = store.get('sites', ['google.com', 'github.com', 'youtube.com', 'chatgpt.com', 'digikala.com', 'aparat.com']).map(u => ({ url: u }));
const saveSites = () => store.set('sites', sites.map(s => s.url));
function renderSites() {
  $('siteQuick').innerHTML = QUICK.filter(u => !sites.some(s => s.url === u)).map(u => `<button class="chip-btn num" data-u="${u}">+ ${u}</button>`).join('');
  $('siteQuick').querySelectorAll('button').forEach(b => b.onclick = () => { sites.push({ url: b.dataset.u }); saveSites(); renderSites(); });
  const done = sites.filter(s => s.res), up = done.filter(s => s.res.ok).length;
  $('siteSum').textContent = done.length ? `${up} از ${done.length} سایت باز میشه` : `${sites.length} سایت`;
  $('siteRows').innerHTML = sites.length ? sites.map((s, i) => {
    const r = s.res; let tag = '<span class="tag wait">تست نشده</span>', ping = '--', dns = '--', http = '--', code = '--', sub = '';
    if (s.loading) tag = '<span class="tag wait"><span class="spin" style="width:11px;height:11px;border-width:2px"></span>در حال تست</span>';
    else if (r) {
      const [c, t] = S_LABEL[r.state] || ['no', r.state];
      tag = `<span class="tag ${c}" title="${esc(r.error || '')}">${t}</span>`;
      ping = r.pingMs == null ? '--' : `<span class="${q(r.pingMs)}">${Math.round(r.pingMs)} ms</span>${r.note === 'http' ? '<span class="via">HTTP</span>' : ''}`;
      if (r.note === 'fake-ip') ping = '<span class="via" title="آدرس Fake-IP حالت TUN؛ پینگ TCP بی‌معنیه">VPN</span>';
      dns = r.dnsMs == null ? '--' : Math.round(r.dnsMs) + ' ms';
      http = r.httpMs == null ? '--' : Math.round(r.httpMs) + ' ms';
      code = r.status ?? '--';
      sub = [r.ip, r.finalUrl ? '→ ' + r.finalUrl : '', !r.ok && r.error ? r.error : ''].filter(Boolean).join(' · ');
    }
    return `<tr><td class="num">${esc(s.url)}<span class="sub" title="${esc(sub)}">${esc(sub)}</span></td><td>${tag}</td><td class="num">${ping}</td><td class="num">${dns}</td><td class="num">${http}</td><td class="num">${code}</td><td style="text-align:left"><button class="icon-btn" data-rm="${i}" aria-label="حذف"><i data-lucide="x" width="15" height="15"></i></button></td></tr>`;
  }).join('') : emptyRow(7, 'globe', 'یه سایت اضافه کن یا از پیشنهادها انتخاب کن');
  $('siteRows').querySelectorAll('[data-rm]').forEach(b => b.onclick = () => { sites.splice(+b.dataset.rm, 1); saveSites(); renderSites(); });
  icons();
}
function addSite() {
  const v = $('siteIn').value.trim().replace(/^https?:\/\//i, '').replace(/\/.*$/, '').toLowerCase();
  if (!v) return; if (!/^[a-z0-9.-]+(:\d+)?$/i.test(v) && !/^\[[0-9a-f:]+\](:\d+)?$/i.test(v)) return toast('آدرس نامعتبره');
  if (!sites.some(s => s.url === v)) sites.push({ url: v });
  $('siteIn').value = ''; saveSites(); renderSites();
}
$('siteAdd').onclick = addSite; $('siteIn').onkeydown = e => { if (e.key === 'Enter') addSite(); };
$('siteClear').onclick = () => { sites = []; saveSites(); renderSites(); };
$('siteRun').onclick = e => busy(e.currentTarget, async () => {
  if (!sites.length) return;
  const batch = [...sites]; batch.forEach(s => { s.loading = true; s.res = null; }); renderSites();
  try {
    const res = await api.checkSites(batch.map(s => s.url));
    const byUrl = new Map(res.map(r => [r.input, r])); // map by url: safe even if the list changed meanwhile
    batch.forEach(s => s.res = byUrl.get(s.url) || null);
  } finally { batch.forEach(s => s.loading = false); renderSites(); }
});

/* ---------- games ---------- */
const { GAMES, CATS } = window.PY_GAMES || { GAMES: [], CATS: {} };
let customGames = store.get('customGames', []);
const allGames = () => [...customGames.map(g => ({ ...g, cat: 'custom', custom: true })), ...GAMES];
let gSel = new Set(store.get('games', ['Valorant', 'Counter-Strike 2', 'PUBG Mobile', 'Call of Duty: Warzone', 'EA Sports FC 25', 'Clash Royale', 'Fortnite', 'League of Legends']));
let gCat = 'all', gRes = [], steamCache = null;
const saveSel = () => store.set('games', [...gSel]);
function visibleGames() {
  const term = $('gSearch').value.trim().toLowerCase();
  return allGames().filter(g => (gCat === 'all' || (gCat === 'sel' ? gSel.has(g.name) : g.cat === gCat)) && (!term || g.name.toLowerCase().includes(term)));
}
function renderCats() {
  const cats = [['all', 'همه'], ['sel', 'انتخاب‌شده'], ...(customGames.length ? [['custom', 'دلخواه']] : []), ...Object.entries(CATS)];
  $('gCats').innerHTML = cats.map(([k, v]) => `<button class="chip-btn" aria-pressed="${k === gCat}" data-c="${k}">${v}</button>`).join('');
  $('gCats').querySelectorAll('button').forEach(b => b.onclick = () => { gCat = b.dataset.c; renderCats(); renderGList(); });
}
function renderGList() {
  const list = visibleGames();
  $('gCount').textContent = `${list.length} بازی`;
  $('glist').innerHTML = list.length ? list.map(g => `<label class="gitem"><input type="checkbox" data-n="${esc(g.name)}" ${gSel.has(g.name) ? 'checked' : ''}>${esc(g.name)}<small>${g.steam ? 'Steam' : g.servers.length + (g.custom ? ' سرور' : ' منطقه')}</small>${g.custom ? `<button class="ed" data-edit="${esc(g.name)}" aria-label="ویرایش" title="ویرایش">✎</button><button class="x" data-del="${esc(g.name)}" aria-label="حذف" title="حذف">×</button>` : ''}</label>`).join('')
    : '<div class="empty">چیزی پیدا نشد</div>';
  $('glist').querySelectorAll('input').forEach(c => c.onchange = () => { c.checked ? gSel.add(c.dataset.n) : gSel.delete(c.dataset.n); saveSel(); sumGames(); });
  $('glist').querySelectorAll('[data-del]').forEach(b => b.onclick = e => { e.preventDefault(); customGames = customGames.filter(g => g.name !== b.dataset.del); gSel.delete(b.dataset.del); store.set('customGames', customGames); saveSel(); renderCats(); renderGList(); if (live?.game === b.dataset.del) stopLive(true); });
  $('glist').querySelectorAll('[data-edit]').forEach(b => b.onclick = e => { e.preventDefault(); openGameEditor(b.dataset.edit); });
  sumGames();
}
function sumGames() { const all = allGames(); $('gSum').textContent = `${all.filter(g => gSel.has(g.name)).length} بازی انتخاب شده از ${all.length}`; }
function renderGRows() {
  const best = {}; gRes.forEach((r, i) => { if (r.avgMs != null && best[r.game] == null) best[r.game] = i; });
  gRes.forEach((r, i) => { if (best[r.game] == null) best[r.game] = i; }); // games with no answer still get a live button
  $('gRows').innerHTML = gRes.length ? gRes.map((r, i) => `<tr class="${i === 0 && r.avgMs != null ? 'best' : ''}"><td>${esc(r.game)}${best[r.game] === i && r.avgMs != null ? '<span class="star" title="بهترین منطقه برای این بازی">★</span>' : ''}<small>${esc(r.region)} · ${esc(r.host)}${r.port ? ':' + r.port : ''}</small></td><td class="num ${q(r.avgMs)}" style="font-weight:500">${r.avgMs == null ? 'timeout' : Math.round(r.avgMs) + ' ms'}${r.note === 'fake-ip' ? '<span class="via" title="Fake-IP (TUN): عدد واقعی نیست">VPN?</span>' : r.note && M_FA[r.note] ? `<span class="via">${M_FA[r.note]}</span>` : ''}</td><td><div class="meter"><span style="width:${r.avgMs != null ? Math.min(100, r.avgMs / 1.6) : 100}%;background:var(--${q(r.avgMs)})"></span></div></td><td class="num">±${f(r.jitterMs, 1)}</td><td class="num">${f(r.lossPct)}%</td><td style="text-align:left">${best[r.game] === i ? `<button class="icon-btn" data-live="${esc(r.game)}" aria-label="پایش زنده" title="پایش زنده‌ی همه‌ی سرورهای این بازی"><i data-lucide="activity" width="15" height="15"></i></button>` : ''}</td></tr>`).join('')
    : emptyRow(6, 'gamepad-2', 'بازی‌ها رو انتخاب کن و «تست» رو بزن');
  $('gRows').querySelectorAll('[data-live]').forEach(b => b.onclick = () => startLive(b.dataset.live));
  icons();
}
const DC = { dxb: 'دبی', fra: 'فرانکفورت', ams: 'آمستردام', vie: 'وین', waw: 'ورشو', sto: 'استکهلم', ist: 'استانبول', par: 'پاریس', lhr: 'لندن', mad: 'مادرید', bom: 'بمبئی', sgp: 'سنگاپور', hel: 'هلسینکی', lux: 'لوکزامبورگ' };
const dcName = dc => 'Steam · ' + (DC[String(dc).slice(0, 3).toLowerCase()] || dc);
async function steamServers() {
  if (!steamCache) {
    const list = await api.steam();
    const pref = Object.keys(DC);
    steamCache = list.filter(s => pref.some(p => String(s.dc).toLowerCase().startsWith(p))).slice(0, 10);
    if (!steamCache.length) steamCache = list.slice(0, 8);
  }
  return steamCache;
}
$('gSearch').oninput = renderGList;
$('gAll').onclick = () => { visibleGames().forEach(g => gSel.add(g.name)); saveSel(); renderGList(); };
$('gNone').onclick = () => { gSel.clear(); saveSel(); renderGList(); };
$('gRun').onclick = e => busy(e.currentTarget, async () => {
  const sel = allGames().filter(g => gSel.has(g.name)); if (!sel.length) return toast('حداقل یه بازی انتخاب کن');
  // many games share the same server region: ping each host:port once, then fan the result out
  const uniq = new Map(), rows = [];
  for (const g of sel) {
    let servers = g.servers;
    if (g.steam) { try { servers = (await steamServers()).map(s => [dcName(s.dc), s.host, s.port]); } catch { servers = []; toast('لیست سرورهای Steam دریافت نشد'); } }
    for (const srv of servers) {
      const [region, host, port] = srv, method = srvMethod(g, srv);
      const k = host + ':' + port + ':' + method;
      if (!uniq.has(k)) uniq.set(k, { name: 'u' + uniq.size, host, port: port || 0, method });
      rows.push({ game: g.name, region, key: uniq.get(k).name });
    }
  }
  if (!uniq.size) return;
  $('gRows').innerHTML = emptyRow(6, 'loader', `در حال پینگ ${uniq.size} سرور برای ${sel.length} بازی...`); icons();
  const res = await api.pingMany([...uniq.values()], 6);
  const byKey = new Map(res.map(r => [r.name, r]));
  gRes = rows.map(r => ({ ...byKey.get(r.key), game: r.game, region: r.region }))
    .sort((a, b) => (a.avgMs ?? 1e9) - (b.avgMs ?? 1e9));
  renderGRows();
  if (gRes[0]?.avgMs != null) toast(`بهترین: ${gRes[0].game} · ${gRes[0].region} با ${Math.round(gRes[0].avgMs)}ms`);
});

/* ---------- custom game editor ---------- */
const METHODS = [['auto', 'خودکار (ICMP → TCP)'], ['icmp', 'ICMP (پینگ واقعی)'], ['tcp', 'TCP'], ['a2s', 'Steam / Source (UDP)'], ['samp', 'SA-MP (UDP)']];
const M_FA = { auto: 'خودکار', icmp: 'ICMP', tcp: 'TCP', a2s: 'A2S', samp: 'SA-MP' };
const srvMethod = (g, s) => s[3] || (g.custom ? 'auto' : 'tcp');
let gmEditing = null; // name of the custom game being edited, null = new
function parseAddr(v) {
  v = v.trim().replace(/^[a-z]+:\/\//i, '').replace(/\/.*$/, ''); if (!v) return null;
  const m = v.match(/^\[([0-9a-f:.]+)\](?::(\d{1,5}))?$/i) || v.match(/^([^:\s]+)(?::(\d{1,5}))?$/) || v.match(/^([0-9a-f:]+)$/i);
  if (!m) return null;
  const port = m[2] ? +m[2] : 0; if (port > 65535) return null;
  return { host: m[1], port };
}
const GM_PRESETS = [
  { name: 'Counter-Strike 1.6 / CS:S / CS:GO', short: 'کانتر (Source)', port: 27015, method: 'a2s' },
  { name: 'SA-MP / open.mp', short: 'SA-MP', port: 7777, method: 'samp' },
  { name: 'Minecraft', short: 'Minecraft', port: 25565, method: 'tcp' },
  { name: 'FiveM / GTA RP', short: 'FiveM', port: 30120, method: 'tcp' },
  { name: 'Team Fortress 2 / Garry\'s Mod', short: 'TF2 / GMod', port: 27015, method: 'a2s' },
  { name: 'نمی‌دونم', short: 'نمی‌دونم (خودکار)', port: 0, method: 'auto' },
];
const PORT_METHOD = { 27015: 'a2s', 27016: 'a2s', 27017: 'a2s', 7777: 'samp', 25565: 'tcp', 30120: 'tcp' };
function gmRow(s = ['', '', 0, 'auto']) {
  const addr = s[1] ? (s[1].includes(':') ? `[${s[1]}]` : s[1]) + (s[2] ? ':' + s[2] : '') : '';
  return `<div class="gm-row"><input class="in fa-in" data-f="region" placeholder="مثلاً تهران" value="${esc(s[0])}"><div class="gm-addr"><input class="in" data-f="addr" spellcheck="false" placeholder="1.2.3.4:27015 یا play.example.com" value="${esc(addr)}"><small class="gm-res" data-f="res"></small></div><select class="in" data-f="method">${METHODS.map(([k, v]) => `<option value="${k}" ${k === (s[3] || 'auto') ? 'selected' : ''}>${v}</option>`).join('')}</select><div class="gm-rbtn"><button class="icon-btn" data-testsrv aria-label="تست" title="تست همین سرور"><i data-lucide="zap" width="15" height="15"></i></button><button class="icon-btn" data-rmsrv aria-label="حذف سرور" title="حذف"><i data-lucide="x" width="15" height="15"></i></button></div></div>`;
}
function gmSetRes(row, cls, text) { const r = row.querySelector('[data-f=res]'); r.className = 'gm-res ' + cls; r.textContent = text; }
// live validation + smart method: port 27015 -> Steam, 7777 -> SA-MP (unless the user picked one himself)
function gmCheckRow(row) {
  const inp = row.querySelector('[data-f=addr]'), raw = inp.value.trim(), sel = row.querySelector('[data-f=method]');
  if (!raw) { inp.classList.remove('bad'); gmSetRes(row, '', ''); return; }
  const a = parseAddr(raw);
  inp.classList.toggle('bad', !a);
  if (!a) return gmSetRes(row, 'bad', 'فرمت درست نیست · مثال: 1.2.3.4:27015');
  if (!sel.dataset.touched && a.port && PORT_METHOD[a.port]) sel.value = PORT_METHOD[a.port];
  if (sel.value === 'tcp' && !a.port) return gmSetRes(row, 'warn', 'برای TCP پورت هم بنویس (مثلاً :25565)');
  gmSetRes(row, 'dim', a.port ? `${a.host} · پورت ${a.port}` : `${a.host} · بدون پورت (پیش‌فرض روش)`);
}
async function gmTest(row) {
  const a = parseAddr(row.querySelector('[data-f=addr]').value);
  if (!a) { gmCheckRow(row); return; }
  if (!IN_APP) return gmSetRes(row, 'warn', 'تست فقط داخل اپ کار می‌کنه');
  const method = row.querySelector('[data-f=method]').value;
  gmSetRes(row, 'dim', 'در حال تست...');
  try {
    const r = await invoke('game_probe', { host: a.host, port: a.port || null, method });
    if (r.ms != null) gmSetRes(row, q(r.ms), `✓ ${Math.round(r.ms)} ms · ${M_FA[r.method] || r.method} · ${r.ip}`);
    else gmSetRes(row, 'bad', `✕ جواب نداد (${M_FA[r.method] || r.method})${method !== 'auto' ? ' · روش دیگه یا «خودکار» رو امتحان کن' : ' · پورت و روش رو چک کن'}`);
  } catch (e) { gmSetRes(row, 'bad', '✕ ' + errText(e)); }
}
function openGameEditor(name) {
  const g = name ? customGames.find(x => x.name === name) : null;
  gmEditing = g ? g.name : null;
  $('gmTitle').textContent = g ? 'ویرایش بازی' : 'بازی دلخواه';
  $('gmName').value = g ? g.name : '';
  $('gmServers').innerHTML = (g ? g.servers : [['', '', 0, 'auto']]).map(s => gmRow(s)).join('');
  $('gmServers').querySelectorAll('.gm-row').forEach(r => { if (g) r.querySelector('[data-f=method]').dataset.touched = 1; gmCheckRow(r); });
  $('gmPresets').innerHTML = GM_PRESETS.map((p, i) => `<button class="chip-btn" data-preset="${i}" title="${esc(p.name)}${p.port ? ' · پورت ' + p.port : ''}">${esc(p.short)}</button>`).join('');
  $('gmDel').hidden = !g;
  $('gmTut').hidden = !!g || store.get('gmTutSeen', false); // first time: tutorial open
  store.set('gmTutSeen', true);
  $('gModal').hidden = false; icons();
  setTimeout(() => $('gmName').focus(), 30);
}
function closeGameEditor() { $('gModal').hidden = true; }
function readGameEditor() {
  const name = $('gmName').value.trim();
  if (!name) { toast('اسم بازی رو بنویس'); return null; }
  if (GAMES.some(g => g.name === name)) { toast('این اسم تو کاتالوگ هست؛ یه اسم دیگه بذار'); return null; }
  if (name !== gmEditing && customGames.some(g => g.name === name)) { toast('بازی‌ای با این اسم داری'); return null; }
  const servers = [];
  for (const [i, row] of [...$('gmServers').querySelectorAll('.gm-row')].entries()) {
    const raw = row.querySelector('[data-f=addr]').value; if (!raw.trim()) continue;
    const a = parseAddr(raw); if (!a) { toast(`آدرس سرور ${i + 1} نامعتبره`); return null; }
    const method = row.querySelector('[data-f=method]').value;
    if (method === 'tcp' && !a.port) { toast(`برای TCP سرور ${i + 1} پورت لازمه`); return null; }
    servers.push([row.querySelector('[data-f=region]').value.trim() || `سرور ${servers.length + 1}`, a.host, a.port, method]);
  }
  if (!servers.length) { toast('حداقل یه سرور اضافه کن'); return null; }
  return { name, servers };
}
function saveGameEditor() {
  const g = readGameEditor(); if (!g) return null;
  if (gmEditing && gmEditing !== g.name) { gSel.delete(gmEditing); }
  customGames = customGames.filter(x => x.name !== gmEditing && x.name !== g.name);
  customGames.unshift(g); gSel.add(g.name);
  store.set('customGames', customGames); saveSel(); renderCats(); renderGList();
  closeGameEditor(); toast(`«${g.name}» ذخیره شد`);
  return g;
}
$('gcNew').onclick = () => openGameEditor(null);
$('gmClose').onclick = closeGameEditor;
$('gModal').onclick = e => { if (e.target === $('gModal')) closeGameEditor(); };
$('gModal').onkeydown = e => { if (e.key === 'Escape') closeGameEditor(); if (e.key === 'Enter' && e.target.matches('input')) $('gmSave').click(); };
$('gmAddSrv').onclick = () => { $('gmServers').insertAdjacentHTML('beforeend', gmRow()); icons(); $('gmServers').lastElementChild.querySelector('[data-f=addr]').focus(); };
$('gmServers').onclick = e => {
  const b = e.target.closest('[data-rmsrv]'); if (b) { b.closest('.gm-row').remove(); if (!$('gmServers').children.length) $('gmAddSrv').click(); return; }
  const t = e.target.closest('[data-testsrv]'); if (t) { t.disabled = true; gmTest(t.closest('.gm-row')).finally(() => t.disabled = false); }
};
$('gmServers').oninput = e => { const row = e.target.closest('.gm-row'); if (row && e.target.matches('[data-f=addr]')) gmCheckRow(row); };
$('gmServers').onchange = e => { const row = e.target.closest('.gm-row'); if (row && e.target.matches('[data-f=method]')) { e.target.dataset.touched = 1; gmCheckRow(row); } };
$('gmTestAll').onclick = e => busy(e.currentTarget, () => Promise.all([...$('gmServers').querySelectorAll('.gm-row')].filter(r => r.querySelector('[data-f=addr]').value.trim()).map(gmTest)));
$('gmTutBtn').onclick = () => { $('gmTut').hidden = !$('gmTut').hidden; store.set('gmTutSeen', true); icons(); };
$('gmPresets').onclick = e => {
  const b = e.target.closest('[data-preset]'); if (!b) return;
  const p = GM_PRESETS[+b.dataset.preset];
  if (!$('gmName').value.trim() && p.method !== 'auto') $('gmName').value = p.name;
  // fill the first empty row (or add one), keep whatever address was already typed
  let row = [...$('gmServers').querySelectorAll('.gm-row')].find(r => !r.querySelector('[data-f=addr]').value.trim()) || $('gmServers').lastElementChild;
  const inp = row.querySelector('[data-f=addr]'), sel = row.querySelector('[data-f=method]');
  sel.value = p.method; sel.dataset.touched = 1;
  const a = parseAddr(inp.value);
  if (a && p.port && !a.port) inp.value = (a.host.includes(':') ? `[${a.host}]` : a.host) + ':' + p.port;
  inp.placeholder = p.port ? `1.2.3.4:${p.port}` : '1.2.3.4 یا play.example.com';
  gmCheckRow(row); inp.focus();
  toast(p.port ? `روش ${M_FA[p.method]} · پورت پیش‌فرض ${p.port}. حالا آدرس سرور رو بنویس` : 'روش خودکار شد. آدرس سرور رو بنویس');
};
$('gmSave').onclick = saveGameEditor;
$('gmLive').onclick = () => { const g = saveGameEditor(); if (g) startLive(g.name); };
$('gmDel').onclick = () => {
  if (!gmEditing) return;
  customGames = customGames.filter(g => g.name !== gmEditing); gSel.delete(gmEditing);
  store.set('customGames', customGames); saveSel(); renderCats(); renderGList(); closeGameEditor();
  if (live?.game === gmEditing) stopLive(true);
};

/* ---------- live game monitor ---------- */
const LIVE_N = 60, LIVE_COLORS = ['var(--y)', 'var(--good)', '#7aa2ff', 'var(--warn)', '#e879f9', '#22d3ee', '#f87171', '#a3e635'];
let live = null, liveSession = 0, liveInt = store.get('liveInt', 1000);
async function liveServers(g) {
  if (g.steam) return (await steamServers()).map(s => [dcName(s.dc), s.host, s.port, 'tcp']);
  return g.servers.map(s => [s[0], s[1], s[2] || 0, srvMethod(g, s)]);
}
async function startLive(name) {
  const g = allGames().find(x => x.name === name); if (!g) return;
  if (!IN_APP) return toast('پایش زنده فقط داخل اپ کار می‌کنه');
  let servers;
  try { servers = (await liveServers(g)).slice(0, 12); } catch (e) { return toast('لیست سرورها دریافت نشد: ' + errText(e)); }
  if (!servers.length) return toast('این بازی سروری نداره');
  const my = ++liveSession;
  live = { session: my, game: name, running: true, servers: servers.map((s, i) => ({ id: 's' + i, region: s[0], host: s[1], port: s[2], method: s[3], ip: null, err: null, samples: [] })) };
  $('gLive').hidden = false; renderLive();
  $('gLive').scrollIntoView({ block: 'nearest' });
  await liveInvoke(my);
}
async function liveInvoke(my) {
  if (!live) return;
  try {
    const res = await invoke('game_live_start', { session: my, targets: live.servers.map(s => ({ id: s.id, host: s.host, port: s.port || null, method: s.method })), intervalMs: liveInt });
    if (!live || live.session !== my) return;
    for (const r of res) { const s = live.servers.find(x => x.id === r.id); if (s) { s.ip = r.ip; s.err = r.error; s.method = r.method; } }
  } catch (e) { if (live?.session === my) { live.running = false; toast('پایش زنده شروع نشد: ' + errText(e)); } }
  renderLive();
}
function stopLive(close) {
  liveSession++;
  if (IN_APP) invoke('game_live_stop').catch(() => {});
  if (close) { live = null; $('gLive').hidden = true; return; }
  if (live) { live.running = false; renderLive(); }
}
function liveStats(a) {
  const ok = a.filter(v => v != null), n = ok.length;
  let j = 0; for (let i = 1; i < n; i++) j += Math.abs(ok[i] - ok[i - 1]);
  return { now: a.length ? a[a.length - 1] : undefined, avg: n ? ok.reduce((x, y) => x + y, 0) / n : null, jit: n > 1 ? j / (n - 1) : null, loss: a.length ? (a.length - n) / a.length * 100 : null, min: n ? Math.min(...ok) : null };
}
let liveRaf = 0;
function scheduleLive() { if (liveRaf) return; liveRaf = requestAnimationFrame(() => { liveRaf = 0; renderLive(); }); }
function renderLive() {
  if (!live || curTab !== 'games' || document.hidden) return;
  $('glName').textContent = live.game;
  $('glDot').className = 'ldot' + (live.running ? ' on' : '');
  $('glMeta').textContent = `${live.servers.length} سرور · هر ${liveInt / 1000} ثانیه · ${LIVE_N} نمونه‌ی آخر`;
  $('glInt').querySelectorAll('button').forEach(b => b.setAttribute('aria-checked', +b.dataset.i === liveInt));
  $('glPause').innerHTML = live.running ? '<i data-lucide="pause" width="14" height="14"></i><span>توقف</span>' : '<i data-lucide="play" width="14" height="14"></i><span>ادامه</span>';
  const stats = live.servers.map(s => liveStats(s.samples));
  const all = live.servers.flatMap(s => s.samples.filter(v => v != null));
  const W = 1000, H = 150, max = Math.max(60, ...all) * 1.15, step = W / (LIVE_N - 1), y = v => H - v / max * H;
  let g = '<g class="g">', axis = '';
  [.33, .66].forEach(k => { const v = Math.round(max * k); g += `<line x1="0" x2="${W}" y1="${y(v)}" y2="${y(v)}"/>`; axis += `<span style="top:${y(v) / H * 100}%">${v}</span>`; });
  g += '</g>';
  live.servers.forEach((s, i) => {
    const off = LIVE_N - s.samples.length; let seg = [];
    const flush = () => { if (seg.length > 1) g += `<polyline points="${seg.join(' ')}" style="stroke:${LIVE_COLORS[i % LIVE_COLORS.length]}"/>`; seg = []; };
    s.samples.forEach((v, k) => { if (v == null) flush(); else seg.push(`${((k + off) * step).toFixed(1)},${y(v).toFixed(1)}`); });
    flush();
  });
  $('glChart').innerHTML = g; $('glAxis').innerHTML = axis;
  let best = -1; stats.forEach((st, i) => { if (st.avg != null && (best < 0 || st.avg < stats[best].avg)) best = i; });
  $('glCards').innerHTML = live.servers.map((s, i) => {
    const st = stats[i], now = st.now;
    const big = now === undefined ? '<span style="color:var(--t3)">...</span>' : now == null ? '<span class="bad">loss</span>' : `<span class="${q(now)}">${now < 10 ? now.toFixed(1) : Math.round(now)}</span><small>ms</small>`;
    return `<div class="glc ${i === best && live.servers.length > 1 ? 'best' : ''}" style="--c:${LIVE_COLORS[i % LIVE_COLORS.length]}"><div class="rg">${esc(s.region)}${i === best && live.servers.length > 1 ? '<span class="star">★</span>' : ''}<span class="ptag">${M_FA[s.method] || esc(s.method)}</span></div><small title="${esc(s.host)}">${esc(s.host)}${s.port ? ':' + s.port : ''}${s.ip && s.ip !== s.host ? ' · ' + esc(s.ip) : ''}</small>${s.err ? `<div class="gerr">${esc(s.err)}</div>` : `<div class="big">${big}</div><div class="kv3"><span>میانگین <b class="num">${f(st.avg)}</b></span><span>جیتر <b class="num">${f(st.jit, 1)}</b></span><span>لاس <b class="num">${st.loss == null ? '--' : f(st.loss) + '%'}</b></span></div>`}</div>`;
  }).join('');
  icons();
}
$('glPause').onclick = () => { if (!live) return; if (live.running) stopLive(false); else { live.running = true; const my = ++liveSession; live.session = my; liveInvoke(my); } };
$('glClose').onclick = () => stopLive(true);
$('glInt').querySelectorAll('button').forEach(b => b.onclick = () => { liveInt = +b.dataset.i; store.set('liveInt', liveInt); if (live?.running) { const my = ++liveSession; live.session = my; liveInvoke(my); } else renderLive(); });
function initLive() {
  listen('game://tick', e => {
    const p = e.payload; if (!live || p.session !== live.session || !live.running) return;
    for (const smp of p.samples) { const s = live.servers.find(x => x.id === smp.id); if (s) { s.samples.push(smp.rtt); if (s.samples.length > LIVE_N) s.samples.shift(); } }
    scheduleLive();
  });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) scheduleLive(); });
}

/* ---------- speed ---------- */
let spHist = store.get('speedHist', []), spRunning = false, spBudget = store.get('speedBudget', 10);
if (![3, 10, 25].includes(spBudget)) spBudget = 10;
function renderSpBudget() { $('spBudget').querySelectorAll('button').forEach(b => { b.setAttribute('aria-checked', +b.dataset.b === spBudget); b.disabled = spRunning; }); }
$('spBudget').querySelectorAll('button').forEach(b => b.onclick = () => { if (spRunning) return; spBudget = +b.dataset.b; store.set('speedBudget', spBudget); renderSpBudget(); });
renderSpBudget();
function renderSpHist() {
  $('spHist').innerHTML = spHist.length ? spHist.map(h => `<tr><td>${esc(h.t)}</td><td class="num">${f(h.d, 1)} Mbps</td><td class="num">${f(h.u, 1)} Mbps</td><td class="num">${f(h.l)} ms</td><td class="num">${f(h.j, 1)}</td><td class="num">${esc(h.c || '--')}</td></tr>`).join('')
    : emptyRow(6, 'history', 'هنوز تستی انجام ندادی');
  icons();
}
function spBtn() {
  $('spRun').innerHTML = spRunning ? '<i data-lucide="square" width="14" height="14"></i>توقف' : '<i data-lucide="play" width="15" height="15"></i>شروع تست';
  $('spRun').classList.toggle('y', !spRunning); icons();
}
function spError(msg) { $('spErr').hidden = false; $('spErr').querySelector('span').textContent = msg; icons(); }
function bloatGrade(idle, loaded) {
  if (idle == null || loaded == null) return ['--', ''];
  const d = loaded - idle;
  return d < 30 ? ['A · عالی', 'good'] : d < 80 ? ['B · خوب', 'good'] : d < 200 ? ['C · متوسط', 'warn'] : ['D · بد', 'bad'];
}
$('spRun').onclick = async () => {
  if (spRunning) { $('spRun').disabled = true; $('spPhase').textContent = 'در حال توقف...'; api.speedCancel().catch(() => {}); return; }
  spRunning = true; spBtn(); renderSpBudget();
  ['spDv', 'spUv'].forEach(k => $(k).textContent = '0.0'); ['spDt', 'spUt'].forEach(k => $(k).style.width = '0');
  ['spLat', 'spLoad', 'spJit', 'spColo', 'spData', 'spGrade'].forEach(k => { $(k).textContent = '--'; $(k).className = k === 'spGrade' ? '' : 'num'; });
  $('spD').classList.add('dim'); $('spU').classList.add('dim'); $('spPhase').textContent = 'سنجش تاخیر...'; $('spPhase').className = 'phase'; $('spErr').hidden = true;
  try {
    const r = await api.speed(spBudget, p => {
      if (p.phase === 'latency') { if (p.latencyMs != null) $('spLat').textContent = f(p.latencyMs); $('spPhase').textContent = `سنجش تاخیر... ${Math.round(p.pct)}%`; }
      else if (p.phase === 'download') { $('spPhase').textContent = 'دانلود...'; $('spD').classList.remove('dim'); $('spDv').textContent = p.mbps.toFixed(1); $('spDt').style.width = p.pct + '%'; if (p.latencyMs != null) $('spLoad').textContent = f(p.latencyMs); }
      else if (p.phase === 'upload') { $('spPhase').textContent = 'آپلود...'; $('spD').classList.toggle('dim', !+$('spDv').textContent); $('spU').classList.remove('dim'); $('spUv').textContent = p.mbps.toFixed(1); $('spUt').style.width = p.pct + '%'; }
    });
    $('spDv').textContent = r.downloadMbps.toFixed(1); $('spUv').textContent = r.uploadMbps.toFixed(1);
    $('spLat').textContent = f(r.latencyMs); $('spLoad').textContent = f(r.loadedLatencyMs); $('spJit').textContent = f(r.jitterMs, 1);
    $('spColo').textContent = r.colo || '--'; $('spData').textContent = f(r.dataMb);
    const [g, c] = bloatGrade(r.latencyMs, r.loadedLatencyMs); $('spGrade').textContent = g; $('spGrade').className = c;
    if (!r.uploadMbps) $('spU').classList.add('dim');
    if (r.warnings?.length) spError(r.warnings.join(' · '));
    if (r.cancelled) { $('spPhase').textContent = 'متوقف شد'; toast('تست متوقف شد'); }
    else {
      $('spPhase').textContent = 'تمام شد'; $('spDt').style.width = $('spUt').style.width = '100%';
      spHist.unshift({ t: new Date().toLocaleString('fa-IR', { dateStyle: 'short', timeStyle: 'short' }), d: r.downloadMbps, u: r.uploadMbps, l: r.latencyMs, j: r.jitterMs, c: r.colo });
      spHist = spHist.slice(0, 15); store.set('speedHist', spHist); renderSpHist();
    }
  } catch (e) {
    // reset the half-drawn state so a failed run doesn't look like "0 Mbps with a full bar"
    ['spDt', 'spUt'].forEach(k => $(k).style.width = '0'); ['spDv', 'spUv'].forEach(k => $(k).textContent = '0.0');
    $('spD').classList.add('dim'); $('spU').classList.add('dim');
    $('spPhase').textContent = 'ناموفق'; $('spPhase').className = 'phase bad'; spError(errText(e));
  }
  finally { spRunning = false; $('spRun').disabled = false; spBtn(); renderSpBudget(); }
};

/* ---------- VPN client (Xray + sing-box cores, all logic in Rust) ---------- */
// port 0 / empty strings = automatic defaults, resolved in Rust (2080 or any free port · 1.1.1.1 DoH · system DNS)
const V_OPTS_DEF = { mode: 'proxy', port: 0, bypassIran: true, remoteDns: '', directDns: '', blockQuic: true, allowLan: false, fragment: false };
const vpn = {
  profiles: store.get('vpnProfiles', []), // {id, name, proto, host, port, net, sec, raw, sub}
  subs: store.get('vpnSubs', []),         // {id, name, url, info, updated}
  sel: store.get('vpnSel', null),
  opts: { ...V_OPTS_DEF, ...store.get('vpnOpts', {}) },
  group: 'all', status: null, core: null, busy: false, check: null, traffic: null,
  res: new Map(),     // id -> { tcp, tcpErr, real, realErr, testing }
  batch: null,        // ids of the running real-delay test, by index
};
const NO_CORE_MSG = 'هسته‌ی لازم نصب نیست. «دانلود هسته‌ها» رو بزن (Xray برای اکثر کانفیگ‌ها، sing-box برای Hysteria2/TUIC و حالت TUN) یا فایل‌هاشون رو تو «پوشه‌ی هسته» بذار.';
const vid = () => 'p' + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
const saveVProfiles = () => store.set('vpnProfiles', vpn.profiles);
const saveVSubs = () => store.set('vpnSubs', vpn.subs);
const vRes = id => { let r = vpn.res.get(id); if (!r) vpn.res.set(id, r = {}); return r; };
const fmtBytes = b => b == null ? '--' : b < 1024 ? b + ' B' : b < 1048576 ? (b / 1024).toFixed(1) + ' KB' : b < 1073741824 ? (b / 1048576).toFixed(1) + ' MB' : (b / 1073741824).toFixed(2) + ' GB';
const fmtDur = ms => { const s = Math.max(0, Math.floor(ms / 1000)), h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60); return (h ? h + ':' + String(m).padStart(2, '0') : m) + ':' + String(s % 60).padStart(2, '0'); };
const vVisible = () => vpn.profiles.filter(p => vpn.group === 'all' || (vpn.group === 'manual' ? !p.sub : p.sub === vpn.group));
const vSelected = () => vpn.profiles.find(p => p.id === vpn.sel) || null;

function vErr(msg, needAdmin = false) {
  $('vErr').hidden = !msg;
  if (msg) $('vErr').querySelector('span').textContent = msg;
  $('vErrAdmin').hidden = !(msg && needAdmin && !appInfo?.admin);
  icons();
}
function addProfiles(items, sub) {
  const have = new Set(vpn.profiles.map(p => p.raw));
  let added = 0, bad = 0;
  for (const it of items) {
    if (!it.ok) { bad++; uiLog('warn', 'vpn', `کانفیگ رد شد (${it.error}): ${it.raw.slice(0, 80)}`); continue; }
    if (have.has(it.raw)) continue;
    have.add(it.raw);
    const p = it.profile;
    vpn.profiles.push({ id: vid(), name: p.name, proto: p.proto, host: p.host, port: p.port, net: p.net, sec: p.sec, engine: p.engine, detail: p.detail || null, raw: p.raw, sub: sub || null });
    added++;
  }
  if (!vpn.sel && vpn.profiles.length) vpn.sel = vpn.profiles[0].id;
  saveVProfiles(); store.set('vpnSel', vpn.sel);
  return { added, bad };
}
async function addSub(url, existing) {
  const r = await invoke('vpn_fetch_sub', { url });
  let host = url; try { host = new URL(url).hostname; } catch {}
  const sub = existing || { id: vid(), url };
  sub.name = r.title || sub.name || host; sub.info = r.info || null; sub.updated = Date.now();
  if (!existing) vpn.subs.push(sub);
  // update = replace this subscription's configs, keep the selection if the same config is still there
  const selRaw = vSelected()?.raw;
  vpn.profiles = vpn.profiles.filter(p => p.sub !== sub.id);
  const res = addProfiles(r.items, sub.id);
  if (selRaw) { const again = vpn.profiles.find(p => p.raw === selRaw); if (again) vpn.sel = again.id; }
  saveVSubs(); saveVProfiles(); store.set('vpnSel', vpn.sel);
  return res;
}
async function vImport(text) {
  text = (text || '').trim(); if (!text) return toast('چیزی برای افزودن نیست');
  if (!IN_APP) return toast('این بخش فقط داخل اپ کار می‌کنه');
  const lines = text.split(/\r?\n/).map(l => l.trim()).filter(Boolean);
  const subs = lines.filter(l => /^https?:\/\//i.test(l)), rest = lines.filter(l => !/^https?:\/\//i.test(l)).join('\n');
  let added = 0, bad = 0;
  for (const u of subs) {
    if (vpn.subs.some(s => s.url === u)) { toast('این ساب قبلاً اضافه شده؛ از ⟳ آپدیتش کن'); continue; }
    try { const r = await addSub(u); added += r.added; bad += r.bad; } catch (e) { vErr('ساب: ' + errText(e)); }
  }
  if (rest) { const r = addProfiles(await invoke('vpn_parse', { text: rest })); added += r.added; bad += r.bad; }
  $('vIn').value = '';
  renderVpn();
  toast(`${added} کانفیگ اضافه شد${bad ? ` · ${bad} نامعتبر (جزئیات تو لاگ)` : ''}`);
}

function renderVGroups() {
  const n = id => vpn.profiles.filter(p => id === 'all' || (id === 'manual' ? !p.sub : p.sub === id)).length;
  const subInfo = s => {
    const i = s.info; if (!i) return '';
    const used = (i.upload || 0) + (i.download || 0);
    const parts = [];
    if (i.total) parts.push(`${fmtBytes(used)} / ${fmtBytes(i.total)}`);
    if (i.expire) { const d = Math.ceil((i.expire * 1000 - Date.now()) / 86400000); parts.push(d > 0 ? `${d} روز` : 'منقضی'); }
    return parts.join(' · ');
  };
  let h = `<button class="chip-btn" data-g="all" aria-pressed="${vpn.group === 'all'}">همه (${n('all')})</button>`;
  if (n('manual') || !vpn.subs.length) h += `<button class="chip-btn" data-g="manual" aria-pressed="${vpn.group === 'manual'}">دستی (${n('manual')})</button>`;
  h += vpn.subs.map(s => `<span class="grp" aria-pressed="${vpn.group === s.id}"><button class="chip-btn" data-g="${s.id}" title="${esc(s.url)}">${esc(s.name)} (${n(s.id)})</button>${subInfo(s) ? `<small>${esc(subInfo(s))}</small>` : ''}<button class="icon-btn" data-up="${s.id}" aria-label="آپدیت ساب" title="آپدیت ساب"><i data-lucide="refresh-cw" width="13" height="13"></i></button><button class="icon-btn" data-rs="${s.id}" aria-label="حذف ساب" title="حذف ساب"><i data-lucide="x" width="13" height="13"></i></button></span>`).join('');
  $('vGroups').innerHTML = h;
}
function realCell(r) {
  if (r.testing) return '<span class="spin" style="display:inline-block;width:12px;height:12px;border:2px solid var(--t3);border-top-color:transparent;border-radius:50%;animation:sp .7s linear infinite"></span>';
  if (r.real != null) return `<span class="${q(r.real / 3)}" style="font-weight:600">${r.real} ms</span>`;
  if (r.realErr) return `<span class="bad">✕</span><span class="err" title="${esc(r.realErr)}">${esc(r.realErr)}</span>`;
  return '--';
}
const PROTO_FA = { vless: 'VLESS', vmess: 'VMess', trojan: 'Trojan', shadowsocks: 'SS', ss: 'SS', hysteria2: 'Hysteria2', hysteria: 'Hysteria', tuic: 'TUIC', socks: 'SOCKS' };
const subName = id => id ? (vpn.subs.find(s => s.id === id)?.name || 'ساب') : 'دستی';
function renderVRows() {
  const list = vVisible(), live = vpn.status?.raw;
  $('vRows').innerHTML = list.length ? list.map((p, i) => {
    const r = vpn.res.get(p.id) || {};
    const tcp = r.tcp != null ? `<span class="${q(r.tcp)}">${Math.round(r.tcp)} ms</span>` : r.tcpErr ? '<span class="bad">timeout</span>' : '--';
    const sec = p.sec && p.sec !== 'none' ? `<span class="ptag ${p.sec === 'reality' ? 'reality' : ''}">${esc(p.sec)}</span>` : '<span class="dim">--</span>';
    return `<tr data-id="${p.id}" class="${p.id === vpn.sel ? 'sel' : ''} ${live && p.raw === live ? 'live' : ''}"><td class="num dim">${i + 1}</td><td><span class="ptag">${esc(PROTO_FA[p.proto] || p.proto)}</span></td><td class="vname"><b title="${esc(p.name)}">${esc(p.name)}</b></td><td class="ltr vaddr" title="${esc(p.host)}">${esc(p.host)}</td><td class="num ltr">${p.port}</td><td class="ltr">${esc(p.net || 'tcp')}</td><td>${sec}</td><td class="vgrp" title="${esc(subName(p.sub))}">${esc(subName(p.sub))}</td><td class="num">${tcp}</td><td class="num">${realCell(r)}</td><td class="acts"><button class="icon-btn" data-act="connect" aria-label="اتصال" title="اتصال"><i data-lucide="plug-zap" width="15" height="15"></i></button><button class="icon-btn" data-act="copy" aria-label="کپی لینک" title="کپی لینک"><i data-lucide="copy" width="15" height="15"></i></button><button class="icon-btn" data-act="del" aria-label="حذف" title="حذف"><i data-lucide="trash-2" width="15" height="15"></i></button></td></tr>`;
  }).join('') : emptyRow(11, 'shield', vpn.profiles.length ? 'تو این گروه کانفیگی نیست' : 'لینک کانفیگ یا ساب رو بالا بچسبون و «افزودن» رو بزن');
  const tested = list.filter(p => vpn.res.get(p.id)?.real != null).length;
  $('vSum').textContent = `${list.length} کانفیگ${tested ? ` · ${tested} کار می‌کنه` : ''}`;
  renderVDetail();
}
// v2rayN-style "everything about this config" panel for the selected row
function renderVDetail() {
  const p = vSelected(), el = $('vDetail');
  if (!p) { el.hidden = true; return; }
  const d = p.detail || {};
  const f = (k, v, cls = 'ltr') => v === '' || v == null || v === false ? '' : `<div class="kv"><span>${k}</span><b class="${cls}" title="${esc(v)}">${esc(v === true ? 'بله' : v)}</b></div>`;
  el.hidden = false;
  el.innerHTML = `<div class="vd-head"><b>${esc(p.name)}</b><span class="ptag">${esc(PROTO_FA[p.proto] || p.proto)}</span><button class="btn sm ghost" data-vd="json"><i data-lucide="braces" width="14" height="14"></i>کپی کانفیگ هسته (JSON)</button><button class="btn sm ghost" data-vd="link"><i data-lucide="link" width="14" height="14"></i>کپی لینک</button></div>
  <div class="vd-grid">${f('آدرس', p.host)}${f('پورت', p.port)}${f('ترنسپورت', p.net || 'tcp')}${f('امنیت', p.sec || 'none')}${f('SNI', d.sni)}${f('Host هدر', d.hostHeader)}${f(p.net === 'grpc' ? 'serviceName' : 'Path', d.path)}${f('Fingerprint', d.fp)}${f('ALPN', d.alpn)}${f('Flow', d.flow)}${f('رمزنگاری SS', d.method)}${f('Early data', d.earlyData || '')}${f('گواهی ناامن (insecure)', d.insecure, '')}${f('گروه', subName(p.sub), '')}</div>
  ${p.detail ? '' : '<small class="dim">جزئیات این کانفیگ هنوز خونده نشده؛ برنامه رو یه بار داخل اپ باز کن.</small>'}`;
}
$('vDetail').onclick = async e => {
  const b = e.target.closest('[data-vd]'), p = vSelected(); if (!b || !p) return;
  if (b.dataset.vd === 'link') return copy(p.raw, 'لینک کپی شد');
  try { copy(JSON.stringify(await invoke('vpn_inspect', { raw: p.raw }), null, 2), 'کانفیگ JSON کپی شد'); } catch (err) { toast('خطا: ' + errText(err)); }
};
function renderVConn() {
  const s = vpn.status, sel = vSelected();
  $('vConn').dataset.state = vpn.busy ? 'busy' : s ? 'on' : 'off';
  $('vBtn').disabled = vpn.busy && !s;
  $('vState').textContent = vpn.busy ? (s ? 'در حال قطع...' : 'در حال اتصال...') : s ? 'متصل' : 'قطع';
  const p = s || sel;
  $('vName').textContent = p ? p.name : 'یه کانفیگ از لیست انتخاب کن';
  const MODE_FA = { proxy: 'پروکسی سیستم', tun: 'TUN', local: 'فقط پورت' };
  $('vMeta').textContent = s ? `${s.proto} · ${s.host}:${s.port} · ${MODE_FA[s.mode] || s.mode} · ${s.engine || ''} · 127.0.0.1:${s.localPort}${s.mode === 'proxy' && !s.systemProxy ? ' (پروکسی سیستم تنظیم نشد)' : ''}` : p ? `${p.proto} · ${p.host}:${p.port}${p.engine ? ' · هسته ' + (p.engine === 'xray' ? 'Xray' : 'sing-box') : ''}` : '';
  const t = vpn.traffic, c = vpn.check;
  $('vUp').textContent = s && t ? fmtBytes(t.up) + '/s' : '--';
  $('vDown').textContent = s && t ? fmtBytes(t.down) + '/s' : '--';
  $('vTotal').textContent = s && t ? fmtBytes(t.upTotal + t.downTotal) : '--';
  $('vTime').textContent = s ? fmtDur(Date.now() - s.since) : '--';
  $('vDelay').textContent = s && c ? (c.delayMs != null ? c.delayMs + ' ms' : 'خطا') : '--';
  $('vDelay').className = 'num ' + (s && c ? (c.delayMs != null ? q(c.delayMs / 3) : 'bad') : '');
  $('vIp').textContent = s && c?.ip ? c.ip + (c.loc ? ' · ' + c.loc : '') : '--';
  $('vIp').title = s && c?.ip ? c.ip : '';
  $('vSideDot').hidden = !s;
  const addr = s ? `127.0.0.1:${s.localPort}` : '';
  $('vAddr').hidden = !s;
  if (s) {
    $('vAddrTxt').textContent = addr;
    $('vAddrHow').textContent = s.mode === 'local'
      ? `حالت «فقط پورت»: پروکسی ویندوز دست نخورده. تو تنظیمات پروکسی برنامه، SOCKS5 یا HTTP با هاست 127.0.0.1 و پورت ${s.localPort} بذار${vpn.opts.allowLan ? ' · از گوشی: IP همین کامپیوتر + همین پورت' : ''}.`
      : s.mode === 'tun' ? 'حالت TUN: همه‌ی برنامه‌ها خودکار از تونل رد میشن؛ این پورت هم برای برنامه‌هایی که پروکسی می‌خوان بازه.'
      : s.systemProxy ? 'پروکسی ویندوز روی همین آدرس تنظیم شد (مرورگرها و بیشتر برنامه‌ها خودکار ازش استفاده می‌کنن).' : 'پروکسی ویندوز تنظیم نشد؛ این آدرس رو دستی تو برنامه‌ها بذار.';
  }
}
$('vAddrCopy').onclick = () => vpn.status && copy(`127.0.0.1:${vpn.status.localPort}`, 'آدرس پروکسی کپی شد');
$('vErrAdmin').onclick = e => busy(e.currentTarget, relaunchAdmin);
function renderVSettings() {
  const o = vpn.opts;
  $('vMode').querySelectorAll('button').forEach(b => b.setAttribute('aria-checked', b.dataset.m === o.mode));
  $('vPort').value = o.port || ''; $('vDns').value = o.remoteDns || ''; $('vDDns').value = o.directDns || '';
  $('vBypass').checked = !!o.bypassIran; $('vLan').checked = !!o.allowLan; $('vQuic').checked = !!o.blockQuic; $('vFrag').checked = !!o.fragment;
  $('vNow').textContent = `الان: پورت ${o.port || '2080 (خودکار)'} · DNS تونل ${o.remoteDns || '1.1.1.1'} · DNS مستقیم ${o.directDns || 'سیستم'}`;
}
function renderCore() {
  const c = vpn.core, el = $('coreInfo');
  if (!IN_APP) { el.textContent = 'هسته: فقط داخل اپ'; return; }
  if (!c) { el.textContent = 'هسته: ...'; return; }
  const both = c.xrayPath && c.path;
  el.className = 'core-pill ' + (both ? 'ok' : c.xrayPath || c.path ? 'part' : 'no');
  el.textContent = [c.xrayPath ? `Xray ${c.xrayVersion || ''}`.trim() : 'Xray ✕', c.path ? `sing-box ${c.version || ''}`.trim() : 'sing-box ✕'].join(' · ');
  el.title = [c.xrayPath, c.path].filter(Boolean).join('\n') || c.dir;
  $('coreDl').hidden = !c.canDownload;
  $('coreDl').querySelector('span').textContent = both ? 'آپدیت هسته‌ها' : 'دانلود هسته‌ها';
}
let vRaf = 0;
function renderVpn() { if (vRaf) return; vRaf = requestAnimationFrame(() => { vRaf = 0; renderVGroups(); renderVRows(); renderVConn(); icons(); }); }

async function loadCore() { if (!IN_APP) return renderCore(); try { vpn.core = await invoke('vpn_core_info'); } catch {} renderCore(); }
async function vConnect(p) {
  if (!p) return toast('اول یه کانفیگ انتخاب کن');
  if (!IN_APP) return toast('این بخش فقط داخل اپ کار می‌کنه');
  if (vpn.busy) return;
  vpn.sel = p.id; store.set('vpnSel', p.id);
  vpn.busy = true; vpn.check = null; vpn.traffic = null; vErr(''); renderVpn();
  try {
    vpn.status = await invoke('vpn_connect', { raw: p.raw, opts: vpn.opts });
    toast(`وصل شد و تست شد · ${p.name}${vpn.status?.delayMs ? ' · ' + vpn.status.delayMs + ' ms' : ''}`);
    setTimeout(() => pollNet(), 600); // the dashboard switches to HTTP mode through the tunnel
  } catch (e) {
    vpn.status = null;
    const m = errText(e);
    if (m === 'NO_CORE') { vErr(NO_CORE_MSG); loadCore(); }
    else if (m === 'NEED_ADMIN') vErr('حالت TUN دسترسی ادمین می‌خواد. دکمه‌ی زرد رو بزن تا برنامه با دسترسی ادمین دوباره باز بشه، یا حالت «پروکسی سیستم» رو انتخاب کن.', true);
    else vErr(m, /ادمین|administrator/i.test(m));
  } finally { vpn.busy = false; renderVpn(); }
}
async function vDisconnect() {
  if (!vpn.status || vpn.busy) return;
  vpn.busy = true; renderVpn();
  try { await invoke('vpn_disconnect'); } catch {}
  vpn.status = null; vpn.check = null; vpn.traffic = null; vpn.busy = false; renderVpn();
  toast('اتصال قطع شد'); setTimeout(() => pollNet(), 600);
}
$('vBtn').onclick = () => vpn.status ? vDisconnect() : vConnect(vSelected());
$('vAdd').onclick = e => busy(e.currentTarget, () => vImport($('vIn').value));
$('vPaste').onclick = e => busy(e.currentTarget, async () => {
  let t = '';
  try { t = await navigator.clipboard.readText(); } catch { return toast('دسترسی به کلیپ‌بورد نشد؛ با Ctrl+V تو کادر بچسبون'); }
  await vImport(t);
});
$('vGroups').onclick = e => {
  const g = e.target.closest('[data-g]'), up = e.target.closest('[data-up]'), rs = e.target.closest('[data-rs]');
  if (up) { const s = vpn.subs.find(x => x.id === up.dataset.up); if (!s) return; up.disabled = true; addSub(s.url, s).then(r => toast(`ساب آپدیت شد · ${r.added} کانفیگ`)).catch(err => vErr('ساب: ' + errText(err))).finally(() => { up.disabled = false; renderVpn(); }); return; }
  if (rs) { const id = rs.dataset.rs; vpn.subs = vpn.subs.filter(s => s.id !== id); vpn.profiles = vpn.profiles.filter(p => p.sub !== id); if (vpn.group === id) vpn.group = 'all'; saveVSubs(); saveVProfiles(); renderVpn(); return; }
  if (g) { vpn.group = g.dataset.g; renderVpn(); }
};
$('vRows').onclick = e => {
  const tr = e.target.closest('tr[data-id]'); if (!tr) return;
  const p = vpn.profiles.find(x => x.id === tr.dataset.id); if (!p) return;
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (act === 'copy') return copy(p.raw, 'لینک کپی شد');
  if (act === 'del') { vpn.profiles = vpn.profiles.filter(x => x.id !== p.id); vpn.res.delete(p.id); if (vpn.sel === p.id) vpn.sel = null; saveVProfiles(); return renderVpn(); }
  vpn.sel = p.id; store.set('vpnSel', p.id);
  if (act === 'connect') return vConnect(p);
  renderVpn();
};
$('vRows').ondblclick = e => { const tr = e.target.closest('tr[data-id]'); const p = tr && vpn.profiles.find(x => x.id === tr.dataset.id); if (p) vConnect(p); };
$('vTcp').onclick = e => busy(e.currentTarget, async () => {
  const list = vVisible(); if (!list.length) return toast('کانفیگی نیست');
  const res = await api.pingMany(list.map(p => ({ name: p.id, host: p.host, port: p.port })), 4);
  for (const r of res) { const x = vRes(r.name); x.tcp = r.avgMs; x.tcpErr = r.avgMs == null; }
  renderVpn();
});
async function realTest(list) {
  if (!IN_APP) return toast('این بخش فقط داخل اپ کار می‌کنه');
  if (!list.length) return toast('کانفیگی نیست');
  vpn.batch = list.map(p => p.id);
  list.forEach(p => { const r = vRes(p.id); r.testing = true; r.real = null; r.realErr = null; });
  renderVpn();
  try {
    const res = await invoke('vpn_delay_test', { links: list.map(p => p.raw) });
    res.forEach(r => { const id = vpn.batch?.[r.index]; if (!id) return; const x = vRes(id); x.testing = false; x.real = r.delayMs ?? null; x.realErr = r.delayMs == null ? r.error || 'timeout' : null; });
  } catch (e) {
    const m = errText(e);
    if (m === 'NO_CORE') { vErr(NO_CORE_MSG); loadCore(); } else toast('خطا: ' + m);
  } finally { list.forEach(p => { vRes(p.id).testing = false; }); vpn.batch = null; renderVpn(); }
  const ok = list.filter(p => vpn.res.get(p.id)?.real != null).length;
  toast(`${ok} از ${list.length} کانفیگ کار می‌کنه`);
}
$('vReal').onclick = e => busy(e.currentTarget, () => realTest(vVisible()));
const vScore = p => { const r = vpn.res.get(p.id) || {}; return r.real != null ? r.real : r.tcp != null ? 1e6 + r.tcp : 2e6; };
$('vSort').onclick = () => { const ids = new Set(vVisible().map(p => p.id)); const sorted = vVisible().sort((a, b) => vScore(a) - vScore(b)); let i = 0; vpn.profiles = vpn.profiles.map(p => ids.has(p.id) ? sorted[i++] : p); saveVProfiles(); renderVpn(); };
$('vBest').onclick = e => busy(e.currentTarget, async () => {
  let list = vVisible(); if (!list.length) return toast('کانفیگی نیست');
  if (!list.some(p => vpn.res.get(p.id)?.real != null)) await realTest(list);
  const best = list.filter(p => vpn.res.get(p.id)?.real != null).sort((a, b) => vScore(a) - vScore(b))[0];
  if (!best) return toast('هیچ کانفیگی جواب نداد');
  if (vpn.status) await vDisconnect();
  await vConnect(best);
});
$('vPrune').onclick = () => {
  const bad = vVisible().filter(p => { const r = vpn.res.get(p.id); return r && !r.testing && r.real == null && (r.realErr || r.tcpErr); });
  if (!bad.length) return toast('اول «تست واقعی» یا «پینگ TCP» بگیر');
  const ids = new Set(bad.map(p => p.id)); vpn.profiles = vpn.profiles.filter(p => !ids.has(p.id)); saveVProfiles(); renderVpn(); toast(`${bad.length} کانفیگ حذف شد`);
};
$('vClear').onclick = () => {
  const vis = vVisible(); if (!vis.length) return;
  if (!$('vClear').dataset.sure) { $('vClear').dataset.sure = 1; $('vClear').textContent = `مطمئنی؟ (${vis.length})`; setTimeout(() => { delete $('vClear').dataset.sure; $('vClear').textContent = 'حذف همه'; }, 3000); return; }
  const ids = new Set(vis.map(p => p.id)); vpn.profiles = vpn.profiles.filter(p => !ids.has(p.id)); delete $('vClear').dataset.sure; $('vClear').textContent = 'حذف همه'; saveVProfiles(); renderVpn();
};
const saveVOpts = () => { store.set('vpnOpts', vpn.opts); renderVSettings(); if (vpn.status) toast('با اتصال دوباره اعمال میشه'); };
// accepts: 1.1.1.1 · 1.1.1.1:53 · [v6]:53 · dns.google · https://... · tls:// · udp:// · tcp:// · quic:// · h3://
const validDnsOpt = v => !v || /^(https?|h3|tls|quic|tcp|udp):\/\/[^\s]+$/i.test(v) || /^\[?[0-9a-f:.]+\]?(:\d{1,5})?$/i.test(v) || /^[a-z0-9.-]+\.[a-z]{2,}(:\d{1,5})?$/i.test(v);
$('vMode').querySelectorAll('button').forEach(b => b.onclick = () => { vpn.opts.mode = b.dataset.m; saveVOpts(); if (b.dataset.m === 'tun') toast('TUN یعنی همه‌ی برنامه‌ها و بازی‌ها · برنامه باید Run as administrator باز شده باشه'); });
$('vPort').onchange = () => {
  const raw = $('vPort').value.trim();
  if (!raw) { vpn.opts.port = 0; return saveVOpts(); }
  const p = +raw; if (!(Number.isInteger(p) && p >= 1024 && p <= 65535)) { toast('پورت باید بین 1024 و 65535 باشه (یا خالی بذار برای خودکار)'); return renderVSettings(); }
  vpn.opts.port = p; saveVOpts();
};
const dnsInput = (id, key) => $(id).onchange = () => {
  const v = $(id).value.trim();
  if (!validDnsOpt(v)) { toast('فرمت DNS درست نیست · مثال: 1.1.1.1 یا https://dns.google/dns-query'); return renderVSettings(); }
  vpn.opts[key] = v; saveVOpts();
};
dnsInput('vDns', 'remoteDns'); dnsInput('vDDns', 'directDns');
$('vBypass').onchange = () => { vpn.opts.bypassIran = $('vBypass').checked; saveVOpts(); };
$('vQuic').onchange = () => { vpn.opts.blockQuic = $('vQuic').checked; saveVOpts(); };
$('vLan').onchange = () => { vpn.opts.allowLan = $('vLan').checked; saveVOpts(); };
$('vFrag').onchange = () => { vpn.opts.fragment = $('vFrag').checked; saveVOpts(); };
$('vReset').onclick = () => { vpn.opts = { ...V_OPTS_DEF, mode: vpn.opts.mode }; saveVOpts(); toast('تنظیمات برگشت به پیش‌فرض'); };
$('coreDir').onclick = () => IN_APP ? invoke('vpn_open_core_dir').catch(() => {}) : toast('فقط داخل اپ');
$('coreDl').onclick = e => busy(e.currentTarget, async () => {
  const un = await listen('vpn://core', ev => { $('coreInfo').textContent = `دانلود ${ev.payload.core || ''} ${Math.round(ev.payload.pct)}% · ${ev.payload.mb.toFixed(1)} MB`; });
  try { vpn.core = await invoke('vpn_core_download'); toast(`هسته‌ها نصب شدن: Xray ${vpn.core.xrayVersion || '✕'} · sing-box ${vpn.core.version || '✕'}`); vErr(''); }
  catch (err) { vErr('دانلود هسته نشد: ' + errText(err) + ' · می‌تونی xray.exe (github.com/XTLS/Xray-core/releases) و sing-box.exe (github.com/SagerNet/sing-box/releases) رو دستی تو «پوشه‌ی هسته» بذاری.'); }
  finally { un(); renderCore(); }
});
async function initVpn() {
  renderVSettings(); renderVpn(); loadCore();
  if (!IN_APP) return;
  listen('vpn://state', e => {
    const { status, error } = e.payload;
    vpn.status = status; if (!status) { vpn.check = null; vpn.traffic = null; }
    if (error) { vErr(error); toast('اتصال VPN قطع شد'); setTimeout(() => pollNet(), 600); }
    renderVpn();
  });
  listen('vpn://stats', e => { vpn.traffic = e.payload; if (curTab === 'tunnel') renderVConn(); });
  listen('vpn://warn', e => { vErr(String(e.payload)); toast('پروکسی سیستم تنظیم نشد'); });
  listen('vpn://check', e => { vpn.check = e.payload; renderVConn(); if (e.payload.delayMs == null) uiLog('warn', 'vpn', 'تست تاخیر بعد از اتصال: ' + (e.payload.error || 'بدون پاسخ')); });
  listen('vpn://delay', e => { const r = e.payload, id = vpn.batch?.[r.index]; if (!id) return; const x = vRes(id); x.testing = false; x.real = r.delayMs ?? null; x.realErr = r.delayMs == null ? r.error || 'timeout' : null; if (curTab === 'tunnel') renderVpn(); });
  setInterval(() => { if (vpn.status && curTab === 'tunnel') $('vTime').textContent = fmtDur(Date.now() - vpn.status.since); }, 1000);
  try { vpn.status = await invoke('vpn_status'); } catch {}
  // configs saved by older versions have no details (sni/path/...): read them once
  // v0.5: configs also need their core (Xray / sing-box); XHTTP links rejected before are accepted now
  const bare = vpn.profiles.filter(p => !p.detail || !p.engine);
  if (bare.length) {
    try {
      const items = await invoke('vpn_parse', { text: bare.map(p => p.raw).join('\n') });
      const byRaw = new Map(items.filter(i => i.ok).map(i => [i.raw, i.profile]));
      for (const p of bare) { const x = byRaw.get(p.raw); if (x) { p.detail = x.detail; p.net = x.net; p.sec = x.sec; p.port = x.port; p.host = x.host; p.engine = x.engine; } }
      saveVProfiles();
    } catch {}
  }
  // one-time migration of the old «سرورهای تونل» textarea
  const old = store.get('tunnels', '');
  if (old && old.trim()) {
    try { const r = addProfiles(await invoke('vpn_parse', { text: old })); if (r.added) toast(`${r.added} لینک از بخش قدیمی تونل منتقل شد`); } catch {}
    store.set('tunnels', '');
  }
  renderVpn();
}

/* ---------- DNS ---------- */
const DNS_PRESETS = [
  ['Cloudflare', '1.1.1.1'], ['Cloudflare 2', '1.0.0.1'], ['Google', '8.8.8.8'], ['Google 2', '8.8.4.4'], ['Quad9', '9.9.9.9'], ['OpenDNS', '208.67.222.222'],
  ['AdGuard', '94.140.14.14'], ['Level3', '4.2.2.4'], ['Yandex', '77.88.8.8'], ['Shecan', '178.22.122.100'], ['Shecan 2', '185.51.200.2'],
  ['Electro', '78.157.42.100'], ['Electro 2', '78.157.42.101'], ['403.online', '10.202.10.202'], ['403.online 2', '10.202.10.102'],
  ['Begzar', '185.55.226.26'], ['Begzar 2', '185.55.225.25'], ['Radar Game', '10.202.10.10'], ['Radar Game 2', '10.202.10.11'], ['Pishgaman', '5.202.100.100'],
].map(([name, ip]) => ({ name, ip }));
let customDns = store.get('customDns', []), dnsRes = new Map(), dnsRan = false;
function dnsList() {
  const sys = (netInfo?.dnsServers || []).map(ip => ({ name: 'DNS فعلی سیستم', ip, sys: true }));
  const seen = new Set(), out = [];
  for (const d of [...sys, ...customDns.map(d => ({ ...d, custom: true })), ...DNS_PRESETS]) { if (!seen.has(d.ip)) { seen.add(d.ip); out.push(d); } }
  return out;
}
function validDns(s) {
  s = s.trim();
  const m4 = s.match(/^(\d{1,3}(?:\.\d{1,3}){3})(?::(\d{1,5}))?$/);
  if (m4) return m4[1].split('.').every(n => +n <= 255) && (!m4[2] || +m4[2] <= 65535);
  return /^(\[[0-9a-f:]+\](:\d{1,5})?|[0-9a-f:]+)$/i.test(s) && s.includes(':');
}
function renderDns() {
  const list = dnsList();
  const rows = list.map(d => ({ ...d, r: dnsRes.get(d.ip) }));
  if (dnsRan) rows.sort((a, b) => (a.r?.ok ? 0 : 1) - (b.r?.ok ? 0 : 1) || (a.r?.medianMs ?? 1e9) - (b.r?.medianMs ?? 1e9));
  const mx = Math.max(1, ...rows.filter(x => x.r?.ok).map(x => x.r.medianMs));
  const bestIp = dnsRan && rows[0]?.r?.ok ? rows[0].ip : null;
  $('dnsRows').innerHTML = rows.map(({ name, ip, sys, custom, r }) => {
    const tagx = sys ? '<span class="badge y">سیستم</span>' : custom ? '<span class="badge">دلخواه</span>' : '';
    let med = '--', min = '--', succ = '--', ans = '--', meter = 0, cls = '';
    if (r) {
      med = r.ok ? f(r.medianMs) + ' ms' : 'بدون پاسخ'; cls = r.ok ? q(r.medianMs) : 'bad';
      min = r.ok ? f(r.minMs) + ' ms' : '--'; succ = f(r.successPct) + '%'; meter = r.ok ? r.medianMs / mx * 100 : 0;
      ans = r.filtered == null ? '--' : r.filtered ? '<span class="tag no">فیلترشده</span>' : '<span class="tag ok">واقعی</span>';
    }
    return `<tr class="${ip === bestIp ? 'best' : ''}"><td>${esc(name)}${tagx}<small>${esc(ip)}</small></td><td class="num ${cls}" style="font-weight:500">${med}</td><td><div class="meter"><span style="width:${meter}%;background:var(--y)"></span></div></td><td class="num">${min}</td><td class="num">${succ}</td><td>${ans}</td><td style="text-align:left;white-space:nowrap"><button class="icon-btn" data-c="${esc(ip)}" aria-label="کپی"><i data-lucide="copy" width="15" height="15"></i></button>${custom ? `<button class="icon-btn" data-rm="${esc(ip)}" aria-label="حذف"><i data-lucide="x" width="15" height="15"></i></button>` : ''}</td></tr>`;
  }).join('');
  $('dnsRows').querySelectorAll('[data-c]').forEach(b => b.onclick = () => copy(b.dataset.c, b.dataset.c + ' کپی شد'));
  $('dnsRows').querySelectorAll('[data-rm]').forEach(b => b.onclick = () => { customDns = customDns.filter(d => d.ip !== b.dataset.rm); store.set('customDns', customDns); renderDns(); });
  icons();
}
$('dnsAdd').onclick = () => {
  const ip = $('dnsIp').value.trim(), name = $('dnsName').value.trim() || ip;
  if (!validDns(ip)) return toast('IP نامعتبره. مثل 1.1.1.1 یا 1.1.1.1:5353');
  if (dnsList().some(d => d.ip === ip)) return toast('این DNS تو لیست هست');
  customDns.unshift({ name, ip }); store.set('customDns', customDns); $('dnsIp').value = $('dnsName').value = ''; renderDns(); toast(name + ' اضافه شد');
};
$('dnsIp').onkeydown = e => { if (e.key === 'Enter') $('dnsAdd').click(); };
$('dnsRun').onclick = e => busy(e.currentTarget, async () => {
  const res = await api.dns(dnsList().map(({ name, ip }) => ({ name, ip })));
  dnsRes = new Map(res.map(r => [r.ip, r])); dnsRan = true; renderDns();
  if (res[0]?.ok) toast(`سریع‌ترین: ${res[0].name} (${f(res[0].medianMs)}ms)`);
});

/* ---------- network info ---------- */
let netInfo = null;
const row = (k, v, mono, cp) => `<div class="nrow"><span>${k}</span><b class="${mono ? 'num' : ''}">${v == null || v === '' ? '--' : v}</b>${cp && v ? `<button class="icon-btn" data-cp="${esc(cp)}" aria-label="کپی"><i data-lucide="copy" width="14" height="14"></i></button>` : ''}</div>`;
const card = (title, icon, body) => `<div class="panel ncard"><div class="label"><i data-lucide="${icon}" width="15" height="15"></i>${title}</div>${body}</div>`;
const yn = (v, yes = 'بله', no = 'خیر') => v == null ? null : v ? `<span class="badge r">${yes}</span>` : `<span class="badge g">${no}</span>`;
const KIND_FA = { 'iran-isp': 'اپراتور ایرانی', 'iran-dc': 'دیتاسنتر ایرانی', datacenter: 'دیتاسنتر / سرور', cdn: 'Cloudflare', isp: 'اپراتور خارجی' };
function renderNet() {
  const n = netInfo; if (!n) return;
  const loc = [n.city, n.region, n.country].filter(Boolean).map(esc).join('، ');
  const opName = n.operatorFa ? `${esc(n.operatorFa)}${n.isp ? `<small class="num">${esc(n.isp)}</small>` : ''}` : esc(n.isp);
  $('netBanner').hidden = false;
  $('netBanner').className = 'banner' + (n.vpn ? ' vpn' : '');
  $('netBanner').innerHTML = n.vpn
    ? `<b>${esc(n.vpnType || 'VPN / پروکسی')} فعاله</b>اطلاعات «اینترنت» مربوط به IP خروجی تونله${n.direct?.ip ? ` · IP واقعی خطت: <span class="num">${esc(n.direct.ip)}</span>${n.direct.operatorFa || n.direct.isp ? ` (${esc(n.direct.operatorFa || n.direct.isp)})` : ''}` : ''}. نشانه‌ها:<ul>${n.vpnReasons.map(r => `<li>${esc(r)}</li>`).join('')}</ul>`
    : `<b>اتصال مستقیم</b>نشانه‌ای از VPN یا پروکسی پیدا نشد${n.operatorFa || n.isp ? ` · اپراتور: ${esc(n.operatorFa || n.isp)}` : ''}`;
  $('netNotes').innerHTML = (n.notes || []).map(t => `<div class="hint"><i data-lucide="info" width="16" height="16"></i><span>${esc(t)}</span></div>`).join('');
  const w = n.wifi, d = n.direct;
  $('netGrid').innerHTML =
    card(n.vpn ? 'اینترنت (خروجی VPN)' : 'اینترنت', 'globe',
      row('IPv4 عمومی', esc(n.publicIp), 1, n.publicIp)
      + row('IPv6 عمومی', n.publicIp6 ? esc(n.publicIp6) : '<span class="badge">ندارد</span>', 1, n.publicIp6)
      + row('اپراتور (ISP)', opName)
      + row('سازمان', esc(n.org))
      + row('ASN', n.asn ? `${esc(n.asn)}${n.asnKind ? `<span class="badge ${n.asnKind.startsWith('iran') ? 'g' : n.asnKind === 'isp' ? '' : 'y'}">${KIND_FA[n.asnKind] || esc(n.asnKind)}</span>` : ''}` : null, 1)
      + row('نوع خط', n.mobile == null ? null : n.mobile ? 'موبایل (سیم‌کارت)' : 'ثابت / ADSL / فیبر'))
    + (d ? card('خط واقعی (بدون پروکسی)', 'radio-tower',
      row('IP مستقیم', esc(d.ip), 1, d.ip) + row('اپراتور', d.operatorFa ? `${esc(d.operatorFa)}<small class="num">${esc(d.isp || '')}</small>` : esc(d.isp))
      + row('ASN', esc(d.asn), 1) + row('مکان', [d.city, d.country || d.countryCode].filter(Boolean).map(esc).join('، '))
      + row('نوع خط', d.mobile == null ? null : d.mobile ? 'موبایل' : 'ثابت')) : '')
    + card('موقعیت', 'map-pin', row('مکان', loc) + row('کد کشور', esc(n.countryCode || n.location), 1) + row('منطقه‌ی زمانی', esc(n.timezone), 1) + row('دیتاسنتر Cloudflare', esc(n.colo), 1) + row('منبع اطلاعات', (n.geoSources || []).length ? esc(n.geoSources.join('، ')) : '<span class="badge r">هیچ سرویسی جواب نداد</span>'))
    + card('امنیت و مسیر', 'shield',
      row('VPN / پروکسی', n.vpn ? `<span class="badge r">${esc(n.vpnType || 'فعال')}</span>` : '<span class="badge g">غیرفعال</span>')
      + row('پروکسی سیستم', n.systemProxy ? esc(n.systemProxy) : n.pacUrl ? 'PAC: ' + esc(n.pacUrl) : '<span class="badge g">خاموش</span>', 1)
      + row('IP پروکسی شناخته‌شده', yn(n.proxy)) + row('IP دیتاسنتری', yn(n.hosting))
      + row('Cloudflare WARP', esc(n.warp), 1) + row('CGNAT', yn(n.cgnat, 'بله (IP مشترک)', 'خیر'))
      + row('HTTP / TLS', [n.http, n.tls].filter(Boolean).map(esc).join(' · '), 1))
    + card('شبکه‌ی محلی', 'router',
      row('اینترفیس اصلی', n.primaryIface ? `${esc(n.primaryIface)}${n.primaryDesc ? `<small>${esc(n.primaryDesc)}</small>` : ''}` : null)
      + row('IP محلی', esc(n.primaryIp), 1, n.primaryIp) + row('گیت‌وی (مودم)', esc(n.gateway), 1, n.gateway)
      + (n.hotspot ? row('نوع اتصال', `<span class="badge y">${esc(n.hotspot)}</span>`) : '')
      + row('MAC', esc(n.mac), 1) + row('سرعت لینک', esc(n.linkSpeed), 1) + row('MTU', n.mtu, 1)
      + row('DNS سیستم', n.dnsServers.length ? n.dnsServers.map(esc).join('<br>') : null, 1)
      + row('تعداد اینترفیس', n.interfaces.filter(i => !i.loopback).length, 1))
    + (w ? card('Wi‑Fi', 'wifi', row('شبکه (SSID)', esc(w.ssid)) + row('قدرت سیگنال', esc(w.signal), 1) + row('استاندارد', esc(w.radio), 1) + row('کانال', esc(w.channel), 1) + row('سرعت دریافت', esc(w.rate), 1) + row('امنیت', esc(w.auth), 1)) : '');
  $('netGrid').querySelectorAll('[data-cp]').forEach(b => b.onclick = () => copy(b.dataset.cp, b.dataset.cp + ' کپی شد'));
  $('ifRows').innerHTML = n.interfaces.filter(i => !i.loopback).map(i => `<tr><td>${esc(i.name)}${i.primary ? '<span class="badge y">اصلی</span>' : ''}${i.vpn ? '<span class="badge g">VPN</span>' : ''}${i.desc ? `<small>${esc(i.desc)}</small>` : ''}</td><td class="num">${esc(i.ip)}${i.mac ? `<small>${esc(i.mac)}</small>` : ''}</td><td class="num">${esc(i.netmask || '--')}</td><td class="num">${esc(i.gateway || '--')}</td><td>${esc(i.kind)} · ${i.v6 ? 'IPv6' : 'IPv4'}${i.speed ? `<small>${esc(i.speed)}${i.mtu ? ' · MTU ' + i.mtu : ''}</small>` : ''}</td></tr>`).join('') || emptyRow(5, 'network', 'اینترفیسی پیدا نشد');
  $('hisp').textContent = n.operatorFa || n.isp || '--'; $('hloc').textContent = n.city || n.country || n.location || '--'; $('sbIp').textContent = n.publicIp || n.publicIp6 || '--';
  $('hvpn').hidden = !n.vpn;
  renderMode(); icons();
}
let netLoading = null;
function loadNet(announce) {
  if (netLoading) return netLoading; // never run two heavy network_info calls at once
  netLoading = (async () => {
    try { netInfo = await api.netInfo(); }
    catch (e) { $('netGrid').innerHTML = card('وضعیت', 'alert-triangle', row('خطا', esc(errText(e)))); $('hisp').textContent = $('hloc').textContent = '--'; icons(); return; }
    renderNet(); renderDns();
    setAutoVpn(netInfo.vpn || quickHint, announce);
  })().finally(() => { netLoading = null; });
  return netLoading;
}

/* ---------- auto VPN detection ----------
   Every few seconds Rust returns a cheap fingerprint of the local network (adapters, default route IP,
   system proxy). When it changes (VPN turned on/off, proxy toggled, Wi-Fi switched) the full
   network_info runs again and the monitor switches between TCP and HTTP by itself. */
let netSig = null;
async function pollNet() {
  if (!IN_APP) return;
  let s; try { s = await rawInvoke('net_signature'); } catch { return; }
  const changed = netSig !== null && s.sig !== netSig;
  netSig = s.sig; quickHint = !!s.hintVpn;
  if (quickHint && !autoVpn) setAutoVpn(true, true); // proxy / TUN is certain: switch right away
  if (changed) { uiLog('info', 'ui', 'تغییر شبکه دیده شد؛ بررسی دوباره‌ی VPN'); loadNet(true); }
}
$('netRefresh').onclick = e => busy(e.currentTarget, () => loadNet(true));

/* ---------- log tab ---------- */
const LVL_FA = { error: 'خطا', warn: 'هشدار', info: 'اطلاعات' };
let logRaf = 0;
function scheduleLogRender() { if (logRaf) return; logRaf = requestAnimationFrame(() => { logRaf = 0; renderLogs(); updLogBadge(); }); }
function updLogBadge() { const b = document.getElementById('logBadge'); if (!b) return; b.hidden = !logUnseenErr; b.textContent = logUnseenErr > 99 ? '99+' : logUnseenErr; }
const logTime = ts => { const d = new Date(ts); return d.toLocaleTimeString('en-GB') + '.' + String(d.getMilliseconds()).padStart(3, '0'); };
function filteredLogs() {
  const term = ($('logQ')?.value || '').trim().toLowerCase();
  return logs.filter(e => (logLvl === 'all' || e.level === logLvl) && (!term || (e.msg + ' ' + e.src).toLowerCase().includes(term)));
}
function renderLogs(force) {
  if (!force && curTab !== 'logs') return;
  const list = filteredLogs(), view = $('logView');
  const cnt = { error: 0, warn: 0, info: 0 }; logs.forEach(e => cnt[e.level] = (cnt[e.level] || 0) + 1);
  $('logCount').textContent = `${logs.length} مورد · ${cnt.error} خطا · ${cnt.warn} هشدار`;
  view.innerHTML = list.length ? list.slice(-600).map(e => `<div class="lrow ${e.level}"><span class="lt num">${logTime(e.ts)}</span><span class="ll">${LVL_FA[e.level] || e.level}</span><span class="ls num">${esc(e.src)}</span><span class="lm">${esc(e.msg)}</span></div>`).join('')
    : `<div class="empty"><i data-lucide="scroll-text" width="22" height="22"></i>${logs.length ? 'چیزی با این فیلتر پیدا نشد' : 'هنوز لاگی ثبت نشده'}</div>`;
  if ($('logFollow').checked) view.scrollTop = view.scrollHeight;
  if (!list.length) icons();
}
const logText = () => filteredLogs().map(e => `${new Date(e.ts).toISOString()} [${e.level.toUpperCase()}] ${e.src}: ${e.msg}`).join('\n');
$('logLvl').querySelectorAll('button').forEach(b => b.onclick = () => { logLvl = b.dataset.l; $('logLvl').querySelectorAll('button').forEach(x => x.setAttribute('aria-checked', x === b)); renderLogs(); });
$('logLvl').querySelector('[data-l=all]').setAttribute('aria-checked', 'true');
$('logQ').oninput = () => renderLogs();
$('logCopy').onclick = () => copy(`PacketYellow ${navigator.userAgent}\n` + logText(), 'لاگ کپی شد');
$('logSave').onclick = () => {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([logText()], { type: 'text/plain;charset=utf-8' }));
  a.download = `packetyellow-log-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.txt`;
  document.body.appendChild(a); a.click(); setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
};
$('logClear').onclick = () => { logs = []; logUnseenErr = 0; if (IN_APP) rawInvoke('clear_logs').catch(() => {}); renderLogs(true); updLogBadge(); };
async function initLogs() {
  if (!IN_APP) { uiLog('warn', 'ui', 'پیش‌نمایش مرورگر: هسته‌ی Rust در دسترس نیست'); return; }
  try {
    await T.event.listen('log://entry', e => addLog(e.payload));
    const old = await rawInvoke('get_logs');
    const seen = new Set(logs.map(e => e.ts + e.msg));
    logs = [...old.filter(e => !seen.has(e.ts + e.msg)), ...logs].sort((a, b) => a.ts - b.ts);
    logUnseenErr = logs.filter(e => e.level === 'error').length;
    scheduleLogRender();
  } catch (e) { addLog({ ts: Date.now(), level: 'error', src: 'ui', msg: 'اتصال لاگ به هسته‌ی Rust برقرار نشد: ' + errText(e) }); }
}

/* ---------- onboarding ---------- */
const ART = {
  hello: '<div class="art-hello"><span class="ring"></span><span class="ring r2"></span><span class="ring r3"></span><div class="big"><i data-lucide="zap" width="42" height="42"></i></div></div>',
  live: '<svg class="art-line" viewBox="0 0 600 150" preserveAspectRatio="none"><g class="grid"><line x1="0" x2="600" y1="40" y2="40"/><line x1="0" x2="600" y1="80" y2="80"/><line x1="0" x2="600" y1="120" y2="120"/></g><polyline points="0,110 40,104 80,108 120,96 160,100 200,70 240,98 280,92 320,100 360,88 400,94 440,60 480,90 520,86 560,92 600,84"/></svg><div class="art-num">24<small>ms</small></div>',
  games: () => `<div class="art-games">${[['Valorant', 38], ['CS2', 52], ['PUBG', 61], ['Fortnite', 44], ['FC 25', 70], ['Clash', 49], ['LoL', 83], ['Dota 2', 57]].map(([n, p], i) => `<span style="animation-delay:${i * .08}s">${n}<em>${p}</em></span>`).join('')}</div>`,
  vpn: '<div class="art-shield"><div class="node"><i data-lucide="laptop" width="30" height="30"></i></div><div class="wire"></div><div class="node y"><i data-lucide="shield-check" width="32" height="32"></i></div><div class="wire"></div><div class="node"><i data-lucide="globe" width="30" height="30"></i></div></div>',
  speed: '<svg class="art-gauge" viewBox="0 0 260 150"><path class="bg" d="M25 135 A105 105 0 0 1 235 135"/><path class="fg" d="M25 135 A105 105 0 0 1 235 135"/><text x="130" y="128" text-anchor="middle">87.4</text></svg>',
  keys: () => `<div class="art-keys">${[1, 2, 3, 4, 5, 6, 7, 8].map((k, i) => `<kbd style="animation-delay:${i * .06}s">${k}</kbd>`).join('')}</div>`,
};
const ONB = [
  { art: 'hello', t: 'به PacketYellow خوش اومدی', p: 'جعبه‌ابزار شبکه برای اینترنت ایران: پینگ زنده، ۲۰۰+ بازی، تست سایت و فیلترینگ، DNS، تست سرعت و سرورهای تونل. همه‌ی اندازه‌گیری‌ها تو هسته‌ی Rust انجام میشه، نه تو مرورگر.' },
  { art: 'live', t: 'پینگ، جیتر و لاس، لحظه‌ای', p: 'داشبورد هر ثانیه اتصالت رو می‌سنجه و بهت میگه برای بازی یا تماس تصویری آماده‌ای یا نه. می‌تونی هر سروری رو به‌عنوان هدف بذاری.' },
  { art: 'games', t: 'بیش از ۲۰۰ بازی آنلاین', p: 'بازی‌هات رو تیک بزن؛ پینگ تا منطقه‌ی سرورهای هر بازی (بحرین، امارات، فرانکفورت و ...) سنجیده میشه و بهترین منطقه ستاره می‌گیره.' },
  { art: 'vpn', t: 'با VPN وصل میشی؟', p: 'پشت VPN پینگ معمولی گول می‌خوره. تو حالت خودکار برنامه خودش می‌فهمه VPN روشنه یا نه و بین TCP و HTTP جابه‌جا میشه (از داشبورد هم عوض میشه).', pick: true },
  { art: 'speed', t: 'تست سرعت کم‌مصرف', p: 'چند اتصال همزمان با سقف حجم (پیش‌فرض ۱۰ مگ به‌جای ۱۰۰ مگ)، حذف slow-start، و سنجش «تاخیر زیر بار» تا بفهمی مودمت وقت دانلود لگ می‌سازه یا نه.' },
  { art: 'keys', t: 'آماده‌ای!', p: 'با کلیدهای ۱ تا ۹ بین بخش‌ها جابه‌جا شو. هر وقت سؤالی داشتی، بخش «راهنما» توضیح همه‌چیز رو داره.' },
];
let onbI = 0;
function drawOnb() {
  const s = ONB[onbI];
  $('onbArt').innerHTML = typeof ART[s.art] === 'function' ? ART[s.art]() : ART[s.art];
  $('onbStep').textContent = `${onbI + 1} / ${ONB.length}`;
  $('onbTitle').textContent = s.t; $('onbText').textContent = s.p;
  $('onbExtra').innerHTML = s.pick ? `<div class="pick three"><button data-m="auto" aria-pressed="${modePref === 'auto'}"><b>خودکار (پیشنهادی)</b><small>خودش VPN رو تشخیص میده</small></button><button data-m="tcp" aria-pressed="${modePref === 'tcp'}"><b>همیشه TCP</b><small>بدون VPN، دقیق‌ترین حالت</small></button><button data-m="http" aria-pressed="${modePref === 'http'}"><b>همیشه HTTP</b><small>پینگ از داخل تونل</small></button></div>` : '';
  $('onbExtra').querySelectorAll('[data-m]').forEach(b => b.onclick = () => { setMode(b.dataset.m); drawOnb(); });
  $('onbDots').innerHTML = ONB.map((_, i) => `<span class="${i === onbI ? 'on' : ''}"></span>`).join('');
  $('onbPrev').style.visibility = onbI ? 'visible' : 'hidden';
  $('onbNext').textContent = onbI === ONB.length - 1 ? 'بزن بریم' : 'بعدی';
  const body = document.querySelector('.onb-body'); body.style.animation = 'none'; void body.offsetWidth; body.style.animation = '';
  icons();
}
function openOnb() { onbI = 0; $('onb').hidden = false; drawOnb(); requestAnimationFrame(() => $('onb').classList.add('show')); setTimeout(() => $('onbNext').focus(), 50); }
function closeOnb() { store.set('onboarded', 3); $('onb').classList.remove('show'); setTimeout(() => $('onb').hidden = true, 300); }
function onbKey(e) {
  if (e.key === 'Escape') closeOnb();
  else if (e.key === 'ArrowLeft') $('onbNext').click();   // RTL: left = forward
  else if (e.key === 'ArrowRight' && onbI) $('onbPrev').click();
}
$('onbNext').onclick = () => { if (onbI < ONB.length - 1) { onbI++; drawOnb(); } else closeOnb(); };
$('onbPrev').onclick = () => { if (onbI) { onbI--; drawOnb(); } };
$('onbSkip').onclick = closeOnb;
$('replayOnb').onclick = openOnb;

/* ---------- website + update check (GitHub releases) ---------- */
const REPO = 'Mehdi138iimm/PacketYellow';
function openUrl(u) {
  const o = T?.opener;
  if (o?.openUrl) return o.openUrl(u).catch(() => window.open(u, '_blank'));
  window.open(u, '_blank');
}
document.addEventListener('click', e => { const b = e.target.closest('[data-open]'); if (b) { e.preventDefault(); openUrl(b.dataset.open); } });
const verNum = v => String(v || '').replace(/^v/i, '').split(/[.-]/).map(x => parseInt(x, 10) || 0);
const newer = (a, b) => { const x = verNum(a), y = verNum(b); for (let i = 0; i < 3; i++) { if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) > (y[i] || 0); } return false; };
// Inside the app the check runs in Rust (`check_update`): it includes pre-releases (betas) and falls back to
// releases.atom when the GitHub API is rate limited (very common on shared Iranian CGNAT IPs).
async function fetchLatest(cur) {
  if (IN_APP) return await invoke('check_update');
  // browser preview only: same logic with fetch
  const r = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=30`, { headers: { Accept: 'application/vnd.github+json' } });
  if (!r.ok) throw new Error('HTTP ' + r.status);
  const list = (await r.json()).filter(x => !x.draft && x.tag_name);
  const best = list.sort((x, y) => newer(x.tag_name, y.tag_name) ? -1 : newer(y.tag_name, x.tag_name) ? 1 : 0)[0];
  return { current: cur, latest: best?.tag_name || null, url: best?.html_url || `https://github.com/${REPO}/releases`, prerelease: !!best?.prerelease, newer: !!best && newer(best.tag_name, cur) };
}
let updRetry = 0, updShown = '';
async function checkUpdate(manual = false) {
  const cur = await getVersion();
  let u;
  try { u = await fetchLatest(cur); }
  catch (err) {
    if (manual) toast('اتصال به گیت‌هاب نشد: ' + errText(err));
    // auto check failed (filtering, no internet yet, rate limit): try again soon instead of waiting 6 hours
    else if (updRetry < 6) { clearTimeout(checkUpdate._t); checkUpdate._t = setTimeout(checkUpdate, Math.min(30, 2 ** updRetry) * 60000); updRetry++; }
    return;
  }
  updRetry = 0;
  latestTag = u.latest || null; if (curTab === 'settings') renderSettings();
  if (!u.newer) { $('hupd').hidden = true; if (manual) toast(`آخرین نسخه رو داری (v${cur})`); return; }
  const tag = u.latest, label = 'نسخه‌ی جدید ' + tag + (u.prerelease ? ' (بتا)' : '');
  const b = $('hupd'); b.hidden = false; b.querySelector('span').textContent = label;
  b.title = `الان: v${cur} · کلیک کن تا صفحه‌ی دانلود باز بشه`; b.onclick = () => openUrl(u.url);
  icons();
  if (manual || updShown !== tag) {
    toast(`${label} منتشر شده! از دکمه‌ی زرد بالا بگیرش`);
    if (updShown !== tag) uiLog('info', 'ui', `نسخه‌ی جدید منتشر شده: ${tag} (نسخه‌ی فعلی v${cur})`);
    updShown = tag;
  }
}

/* ---------- app version + settings tab ---------- */
const FALLBACK_VER = '0.5.1';
let appInfo = null, latestTag = null;
async function getVersion() {
  if (appInfo?.version) return appInfo.version;
  try { if (IN_APP && T.app?.getVersion) return await T.app.getVersion(); } catch {}
  return FALLBACK_VER;
}
async function loadAppInfo() {
  if (IN_APP) { try { appInfo = await invoke('app_info'); } catch {} }
  const v = 'v' + (appInfo?.version || await getVersion());
  $('tVer').textContent = v + ' بتا'; $('sbVer').textContent = `PacketYellow ${v} · بتا`;
  document.title = `PacketYellow ${v} (بتا)`;
  if (curTab === 'settings') renderSettings();
}
async function relaunchAdmin() {
  if (!IN_APP) return toast('فقط داخل اپ');
  try { await invoke('app_relaunch_admin'); } catch (e) { toast(errText(e)); }
}
function renderSettings() {
  const a = appInfo, v = a?.version || FALLBACK_VER;
  $('setVer').textContent = 'v' + v; $('setVer2').textContent = `${v} (بتا)`;
  $('setLatest').textContent = latestTag ? latestTag + (newer(latestTag, v) ? ' · جدیدتره!' : ' · به‌روزی') : '—';
  $('setOs').textContent = a ? `${a.os} · ${a.arch}` : navigator.platform || '—';
  $('setAdmin').textContent = !a ? '—' : a.admin ? 'بله (TUN آماده‌ست)' : 'نه (برای TUN لازمه)';
  $('setAdmin').style.color = a ? (a.admin ? 'var(--good)' : 'var(--warn)') : '';
  $('setAdminBtn').hidden = !!a?.admin;
  const c = vpn.core;
  $('setCores').textContent = c ? `Xray ${c.xrayPath ? c.xrayVersion || '✓' : '✕'} · sing-box ${c.path ? c.version || '✓' : '✕'}` : '—';
  $('setStart').value = store.get('startTab', 'dash');
  $('setMode').querySelectorAll('button').forEach(b => b.setAttribute('aria-checked', b.dataset.m === modePref));
  $('setAutoUpd').checked = store.get('autoUpdate', true);
  refreshProxyInfo();
}
async function refreshProxyInfo() {
  if (!IN_APP) { $('setProxy').textContent = '—'; return; }
  try { $('setProxy').textContent = await invoke('vpn_proxy_info'); } catch { $('setProxy').textContent = '—'; }
}
$('setUpd').onclick = e => busy(e.currentTarget, () => checkUpdate(true));
$('setStart').onchange = () => { store.set('startTab', $('setStart').value); toast('از دفعه‌ی بعد این صفحه اول باز میشه'); };
$('setMode').querySelectorAll('button').forEach(b => b.onclick = () => { setMode(b.dataset.m); renderSettings(); toast('روش سنجش داشبورد عوض شد'); });
$('setAutoUpd').onchange = () => store.set('autoUpdate', $('setAutoUpd').checked);
$('setOnb').onclick = () => openOnb();
$('setAdminBtn').onclick = e => busy(e.currentTarget, relaunchAdmin);
$('setFixProxy').onclick = e => busy(e.currentTarget, async () => {
  if (!IN_APP) return toast('فقط داخل اپ');
  try { const now = await invoke('vpn_restore_proxy'); $('setProxy').textContent = now; toast('پروکسی ویندوز: ' + now); setTimeout(() => pollNet(), 600); }
  catch (err) { toast(errText(err)); }
});
$('setKill').onclick = e => busy(e.currentTarget, async () => {
  if (!IN_APP) return toast('فقط داخل اپ');
  try { const n = await invoke('vpn_kill_leftovers'); toast(n ? `${n} هسته‌ی جامانده بسته شد` : 'هسته‌ی جامانده‌ای پیدا نشد'); }
  catch (err) { toast(errText(err)); }
});
$('setExport').onclick = () => {
  const data = {};
  for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); if (k.startsWith('py:')) data[k] = localStorage.getItem(k); }
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([JSON.stringify({ app: 'PacketYellow', version: appInfo?.version || FALLBACK_VER, data }, null, 2)], { type: 'application/json' }));
  a.download = `packetyellow-backup-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.appendChild(a); a.click(); setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
  toast('فایل پشتیبان ذخیره شد');
};
$('setImport').onchange = async () => {
  const f = $('setImport').files?.[0]; $('setImport').value = ''; if (!f) return;
  try {
    const j = JSON.parse(await f.text());
    if (j?.app !== 'PacketYellow' || typeof j.data !== 'object') throw new Error('این فایل پشتیبان PacketYellow نیست');
    Object.entries(j.data).forEach(([k, v]) => { if (k.startsWith('py:') && typeof v === 'string') localStorage.setItem(k, v); });
    toast('بازگردانی شد، برنامه دوباره بارگذاری میشه'); setTimeout(() => location.reload(), 900);
  } catch (err) { toast('بازگردانی نشد: ' + errText(err)); }
};
$('setReset').onclick = () => {
  const b = $('setReset');
  if (!b.dataset.sure) { b.dataset.sure = '1'; b.lastChild.textContent = 'مطمئنی؟ دوباره بزن'; setTimeout(() => { delete b.dataset.sure; b.lastChild.textContent = 'پاک کردن همه‌ی داده‌ها'; }, 4000); return; }
  Object.keys(localStorage).filter(k => k.startsWith('py:')).forEach(k => localStorage.removeItem(k));
  toast('همه‌ی داده‌ها پاک شد'); setTimeout(() => location.reload(), 900);
};

/* ---------- boot ---------- */
initLogs(); loadAppInfo();
$('sbMode').textContent = IN_APP ? 'متصل به هسته‌ی Rust' : 'پیش‌نمایش مرورگر: اندازه‌گیری فقط داخل اپ';
setInterval(() => $('sbClock').textContent = new Date().toLocaleTimeString('en-GB'), 1000);
initVpn(); if (IN_APP) initLive();
show(['dash', 'sites', 'games', 'speed', 'tunnel', 'dns', 'net'].includes(store.get('startTab', 'dash')) ? store.get('startTab', 'dash') : 'dash'); renderMode(); renderTargets(); renderSites(); renderCats(); renderGList(); renderGRows(); renderSpHist(); renderDns();
icons();
const splashDone = Promise.race([loadNet(false), new Promise(r => setTimeout(r, 2500))]);
setInterval(pollNet, 4000);
setTimeout(() => store.get('autoUpdate', true) && checkUpdate(), 4000); setInterval(() => store.get('autoUpdate', true) && checkUpdate(), 2 * 3600 * 1000);
Promise.all([splashDone, new Promise(r => setTimeout(r, 900))]).then(() => {
  $('splash').classList.add('hide'); setTimeout(() => $('splash').remove(), 600);
  if (store.get('onboarded', 0) < 3) setTimeout(openOnb, 350);
});
// quick local check first so the monitor starts in the right mode (max 1.5s wait)
Promise.race([pollNet(), new Promise(r => setTimeout(r, 1500))]).finally(() => { mode = effMode(); renderMode(); renderTargets(); restart(); });
