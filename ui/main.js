// PacketYellow v0.3 frontend.
// IMPORTANT: every measurement happens in Rust. This file never times anything; it only renders.
const T = window.__TAURI__;
const IN_APP = !!(T && T.core);
const rawInvoke = (cmd, args) => IN_APP ? T.core.invoke(cmd, args) : Promise.reject('این بخش فقط داخل اپ PacketYellow کار می‌کنه');
// every command goes through here: failures and slow calls land in the «لاگ» tab
const QUIET = new Set(['get_logs', 'clear_logs', 'log_from_ui', 'stop_monitor', 'speed_cancel']);
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
  async speed(onProgress) {
    const un = await listen('speed://progress', e => onProgress(e.payload));
    try { return await invoke('speed_test'); } finally { un(); }
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
  if (e.target.matches('input,textarea') || e.ctrlKey || e.altKey || e.metaKey) return;
  const i = +e.key; if (i >= 1 && i <= tabs.length) show(tabs[i - 1].dataset.tab);
});

/* ---------- window controls (decorations: false) ---------- */
document.querySelectorAll('[data-win]').forEach(b => b.addEventListener('click', () => {
  if (!IN_APP) return;
  Promise.resolve(T.window.getCurrentWindow()[b.dataset.win]()).catch(e => toast('خطا: ' + e));
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
  } catch (e) { if (my === gen) { toast('خطا: ' + e); $('hdot').className = 'sdot off'; $('hstat').textContent = 'خطا'; } }
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
  $('glist').innerHTML = list.length ? list.map(g => `<label class="gitem"><input type="checkbox" data-n="${esc(g.name)}" ${gSel.has(g.name) ? 'checked' : ''}>${esc(g.name)}<small>${g.steam ? 'Steam' : g.servers.length + ' منطقه'}</small>${g.custom ? `<button class="x" data-del="${esc(g.name)}" aria-label="حذف">×</button>` : ''}</label>`).join('')
    : '<div class="empty">چیزی پیدا نشد</div>';
  $('glist').querySelectorAll('input').forEach(c => c.onchange = () => { c.checked ? gSel.add(c.dataset.n) : gSel.delete(c.dataset.n); saveSel(); sumGames(); });
  $('glist').querySelectorAll('[data-del]').forEach(b => b.onclick = e => { e.preventDefault(); customGames = customGames.filter(g => g.name !== b.dataset.del); gSel.delete(b.dataset.del); store.set('customGames', customGames); saveSel(); renderCats(); renderGList(); });
  sumGames();
}
function sumGames() { $('gSum').textContent = `${gSel.size} بازی انتخاب شده از ${allGames().length}`; }
function renderGRows() {
  const best = {}; gRes.forEach((r, i) => { if (r.avgMs != null && best[r.game] == null) best[r.game] = i; });
  $('gRows').innerHTML = gRes.length ? gRes.map((r, i) => `<tr class="${i === 0 && r.avgMs != null ? 'best' : ''}"><td>${esc(r.game)}${best[r.game] === i ? '<span class="star" title="بهترین منطقه برای این بازی">★</span>' : ''}<small>${esc(r.region)} · ${esc(r.host)}:${r.port}</small></td><td class="num ${q(r.avgMs)}" style="font-weight:500">${r.avgMs == null ? 'timeout' : Math.round(r.avgMs) + ' ms'}${r.note === 'fake-ip' ? '<span class="via" title="Fake-IP (TUN): عدد واقعی نیست">VPN?</span>' : ''}</td><td><div class="meter"><span style="width:${r.avgMs != null ? Math.min(100, r.avgMs / 1.6) : 100}%;background:var(--${q(r.avgMs)})"></span></div></td><td class="num">±${f(r.jitterMs, 1)}</td><td class="num">${f(r.lossPct)}%</td></tr>`).join('')
    : emptyRow(5, 'gamepad-2', 'بازی‌ها رو انتخاب کن و «تست» رو بزن');
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
$('gcAdd').onclick = () => {
  const name = $('gcName').value.trim(), t = parseTarget($('gcHost').value);
  if (!name || !t) return toast('نام و host:port رو درست وارد کن');
  customGames = customGames.filter(g => g.name !== name); customGames.unshift({ name, servers: [['دلخواه', t.host, t.port]] });
  store.set('customGames', customGames); gSel.add(name); saveSel(); $('gcName').value = $('gcHost').value = ''; renderCats(); renderGList();
};
$('gRun').onclick = e => busy(e.currentTarget, async () => {
  const sel = allGames().filter(g => gSel.has(g.name)); if (!sel.length) return toast('حداقل یه بازی انتخاب کن');
  // many games share the same server region: ping each host:port once, then fan the result out
  const uniq = new Map(), rows = [];
  for (const g of sel) {
    let servers = g.servers;
    if (g.steam) { try { servers = (await steamServers()).map(s => [dcName(s.dc), s.host, s.port]); } catch { servers = []; toast('لیست سرورهای Steam دریافت نشد'); } }
    for (const [region, host, port] of servers) {
      const k = host + ':' + port;
      if (!uniq.has(k)) uniq.set(k, { name: 'u' + uniq.size, host, port });
      rows.push({ game: g.name, region, key: uniq.get(k).name });
    }
  }
  if (!uniq.size) return;
  $('gRows').innerHTML = emptyRow(5, 'loader', `در حال پینگ ${uniq.size} سرور برای ${sel.length} بازی...`); icons();
  const res = await api.pingMany([...uniq.values()], 6);
  const byKey = new Map(res.map(r => [r.name, r]));
  gRes = rows.map(r => ({ ...byKey.get(r.key), game: r.game, region: r.region }))
    .sort((a, b) => (a.avgMs ?? 1e9) - (b.avgMs ?? 1e9));
  renderGRows();
  if (gRes[0]?.avgMs != null) toast(`بهترین: ${gRes[0].game} · ${gRes[0].region} با ${Math.round(gRes[0].avgMs)}ms`);
});

/* ---------- speed ---------- */
let spHist = store.get('speedHist', []), spRunning = false;
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
  spRunning = true; spBtn();
  ['spDv', 'spUv'].forEach(k => $(k).textContent = '0.0'); ['spDt', 'spUt'].forEach(k => $(k).style.width = '0');
  ['spLat', 'spLoad', 'spJit', 'spColo', 'spData', 'spGrade'].forEach(k => { $(k).textContent = '--'; $(k).className = k === 'spGrade' ? '' : 'num'; });
  $('spD').classList.add('dim'); $('spU').classList.add('dim'); $('spPhase').textContent = 'سنجش تاخیر...'; $('spPhase').className = 'phase'; $('spErr').hidden = true;
  try {
    const r = await api.speed(p => {
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
  finally { spRunning = false; $('spRun').disabled = false; spBtn(); }
};

/* ---------- tunnel servers ---------- */
function b64(s) { s = s.replace(/-/g, '+').replace(/_/g, '/').replace(/\s/g, ''); while (s.length % 4) s += '='; return decodeURIComponent(escape(atob(s))); }
function parseLine(line, i) {
  line = line.trim(); if (!line || line.startsWith('#') || line.startsWith('//')) return null;
  let name = '', proto = 'tcp', rest = line;
  const m = line.match(/^(vless|vmess|trojan|ss|ssr|hysteria2|hy2|hysteria|tuic|wireguard|socks|http):\/\/(.+)$/i);
  if (m) {
    proto = m[1].toLowerCase(); rest = m[2];
    const hash = rest.indexOf('#'); if (hash > -1) { try { name = decodeURIComponent(rest.slice(hash + 1)); } catch { name = rest.slice(hash + 1); } rest = rest.slice(0, hash); }
    if (proto === 'vmess' && !rest.includes('@')) {
      try { const j = JSON.parse(b64(rest)); return { name: j.ps || `vmess-${i + 1}`, host: j.add, port: +j.port || 443, proto }; } catch { return null; }
    }
    rest = rest.split('?')[0].replace(/\/.*$/, '');
    if (proto === 'ss' && !rest.includes('@')) { // ss://base64(method:pass@host:port)
      try { const d = b64(rest); rest = d.slice(d.lastIndexOf('@') + 1); } catch { return null; }
    }
    if (rest.includes('@')) rest = rest.slice(rest.lastIndexOf('@') + 1);
  } else if (line.includes(',')) { [name, rest] = line.split(',').map(s => s.trim()); }
  const hp = rest.match(/^\[?([^\]]+?)\]?:(\d+)$/);
  const host = hp ? hp[1] : rest, port = hp ? +hp[2] : 443;
  if (!host || /\s/.test(host)) return null;
  return { name: name || host, host, port, proto };
}
let tunRes = [];
$('tunIn').value = store.get('tunnels', '');
$('tunIn').addEventListener('input', () => store.set('tunnels', $('tunIn').value));
function renderTun() {
  $('tunRows').innerHTML = tunRes.length ? tunRes.map((r, i) => `<tr class="${i === 0 && r.avgMs != null ? 'best' : ''}"><td>${esc(r.name)}<small>${esc(r.host)}:${r.port}</small></td><td><span class="tag wait num">${esc(r.proto)}</span></td><td class="num ${q(r.avgMs)}" style="font-weight:500">${r.avgMs == null ? 'timeout' : Math.round(r.avgMs) + ' ms'}</td><td class="num">±${f(r.jitterMs, 1)}</td><td class="num">${f(r.lossPct)}%</td><td style="text-align:left"><button class="icon-btn" data-c="${i}" aria-label="کپی"><i data-lucide="copy" width="15" height="15"></i></button></td></tr>`).join('')
    : emptyRow(6, 'waypoints', 'لینک‌هات رو بالا بذار و «تست و مرتب‌سازی» رو بزن');
  $('tunRows').querySelectorAll('[data-c]').forEach(b => b.onclick = () => copy(tunRes[+b.dataset.c].raw, 'لینک کپی شد'));
  const ok = tunRes.filter(r => r.avgMs != null).length; $('tunSum').textContent = tunRes.length ? `${ok} از ${tunRes.length} سرور جواب داد` : '';
  icons();
}
$('tunRun').onclick = e => busy(e.currentTarget, async () => {
  let text = $('tunIn').value.trim();
  if (text && !text.includes('://') && /^[A-Za-z0-9+/=_\-\s]+$/.test(text)) { try { text = b64(text); } catch {} } // base64 subscription
  const parsed = text.split('\n').map((l, i) => { const p = parseLine(l, i); return p && { ...p, raw: l.trim(), key: 'k' + i }; }).filter(Boolean);
  if (!parsed.length) return toast('لینک معتبری پیدا نشد');
  const res = await api.pingMany(parsed.map(p => ({ name: p.key, host: p.host, port: p.port })), 6);
  const byKey = new Map(parsed.map(p => [p.key, p]));
  tunRes = res.map(r => { const p = byKey.get(r.name); return { ...r, name: p.name, proto: p.proto, raw: p.raw }; });
  renderTun();
});

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
  { art: 'speed', t: 'تست سرعت دقیق', p: 'چند اتصال همزمان، حذف ثانیه‌های اول، و سنجش «تاخیر زیر بار» تا بفهمی مودمت وقت دانلود لگ می‌سازه یا نه.' },
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
async function checkUpdate() {
  let cur = '0.3.2';
  try { if (IN_APP && T.app?.getVersion) cur = await T.app.getVersion(); } catch {}
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: 'application/vnd.github+json' } });
    if (!r.ok) return; // 404 = no release yet, 403 = rate limit: stay quiet
    const rel = await r.json();
    if (!newer(rel.tag_name, cur)) return;
    const b = $('hupd'); b.hidden = false; b.querySelector('span').textContent = 'نسخه‌ی جدید ' + rel.tag_name;
    b.title = 'الان: ' + cur; b.onclick = () => openUrl(rel.html_url || `https://github.com/${REPO}/releases/latest`);
    uiLog('info', 'ui', `نسخه‌ی جدید منتشر شده: ${rel.tag_name} (نسخه‌ی فعلی ${cur})`); icons();
  } catch {}
}

/* ---------- boot ---------- */
initLogs();
$('sbMode').textContent = IN_APP ? 'متصل به هسته‌ی Rust' : 'پیش‌نمایش مرورگر: اندازه‌گیری فقط داخل اپ';
setInterval(() => $('sbClock').textContent = new Date().toLocaleTimeString('en-GB'), 1000);
show('dash'); renderMode(); renderTargets(); renderSites(); renderCats(); renderGList(); renderGRows(); renderSpHist(); renderTun(); renderDns();
icons();
const splashDone = Promise.race([loadNet(false), new Promise(r => setTimeout(r, 2500))]);
setInterval(pollNet, 4000);
setTimeout(checkUpdate, 5000); setInterval(checkUpdate, 6 * 3600 * 1000);
Promise.all([splashDone, new Promise(r => setTimeout(r, 900))]).then(() => {
  $('splash').classList.add('hide'); setTimeout(() => $('splash').remove(), 600);
  if (store.get('onboarded', 0) < 3) setTimeout(openOnb, 350);
});
// quick local check first so the monitor starts in the right mode (max 1.5s wait)
Promise.race([pollNet(), new Promise(r => setTimeout(r, 1500))]).finally(() => { mode = effMode(); renderMode(); renderTargets(); restart(); });
