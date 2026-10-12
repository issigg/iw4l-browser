// Engine worker (module worker). Protocol: see PROTOCOL.md.
import { installGames } from './gamefiles.js';
// WASM-SPIKE: mirror console output to the dev server so it can be read as a file.
const q = []; let t = 0;
function flush() { t = 0; if (!q.length) return; if (globalThis.iw4Games) { q.length = 0; return; } // folder mode: no dev server to log to
  const b = q.splice(0).join('\n'); fetch('/__log', { method: 'POST', body: b, keepalive: true }).catch(() => {}); }
for (const k of ['log', 'info', 'warn', 'error']) {
  const o = console[k].bind(console);
  console[k] = (...a) => {
    o(...a);
    if (k === 'error') { try { onEngineLog(String(a[0])); } catch (_) {} }
    q.push('[' + k + '] ' + a.map(x => x && x.stack || String(x)).join(' '));
    if (!t) t = setTimeout(flush, 200);
  };
}
addEventListener('error', e => console.error('worker error', e.message));
addEventListener('unhandledrejection', e => console.error('unhandled rejection', String(e.reason)));

const progress = (text, pct) => postMessage({ type: 'progress', text, pct });

// ---- load progress + post-load memory release, driven by the engine's own log lines ----
// Stage table: [name, weight]. Weights are typical wall-clock ms from a recent mp_rust load log;
// the prefetch owns 0..15 %, the stages share 15..99 %, `load complete` is 100 %. Percentages
// never go backwards (stages overlap and some log out of order). Text describes the NEXT work.
const PREFETCH_PCT = 15;
const STAGES = [
  ['map_assets/open', 230, 'reading common assets'],
  ['common_assets/common_mp', 400, 'reading common assets'],
  ['localization', 650, 'reading common assets'],
  ['common_assets/shared common', 2000, 'reading map data'],
  ['map_assets/header', 10, 'reading map data'],
  ['map_assets/memory', 10, 'reading map data'],
  ['map_assets/walk', 230, 'reading map data'],
  ['map_assets/collision', 25, 'reading map data'],
  ['images/map', 150, 'reading map data'],
  ['map_assets/geometry', 120, 'reading map data'],
  ['map_assets/models', 10, 'reading map data'],
  ['map_assets/lighting', 40, 'reading map data'],
  ['map_assets/handoff', 10, 'loading textures'],
  ['images/common_mp FPV', 60, 'loading textures'],
  ['images/merged', 5, 'loading textures'],
  ['images/attenuation', 5, 'loading textures'],
  ['images/tracers', 5, 'loading textures'],
  ['images/fx_elem', 5, 'setting up the world'],
  ['install', 275, 'building navigation'],
  ['preview', 10, 'building navigation'],
  ['navigation', 180, 'preparing shaders'],
  ['programs', 130, 'preparing shaders'],
  ['program_merge', 10, 'preparing shaders'],
  ['shaders', 180, 'loading textures'],
  ['world_images', 3970, 'uploading textures'],
  ['gpu_textures', 290, 'loading audio'],
  ['audio', 4885, 'building render pipelines'],
  ['pipelines', 870, 'rendering first frames'],
  ['render_frames', 985, 'finishing up'],
  ['first_person', 300, 'finishing up'],
  ['admission', 100, 'finishing up'],
];
const STAGE_INFO = (() => {
  const total = STAGES.reduce((n, s) => n + s[1], 0);
  let acc = 0; const m = new Map();
  for (const [name, w, next] of STAGES) {
    acc += w;
    m.set(name, { pct: PREFETCH_PCT + (acc / total) * (99 - PREFETCH_PCT), next });
  }
  return m;
})();
let loadPct = 0, loadDone = false, memReleased = false;
let releaseHook = null; // set by boot() once the stores exist
let loadMap = null;

// Report progress without ever going backwards; once the stage log starts, prefetch updates stop.
function reportProgress(text, pct) {
  if (pct < loadPct) pct = loadPct;
  loadPct = pct;
  progress(text, pct);
}

function onEngineLog(line) {
  if (loadDone) return;
  if (line.includes('load complete:') || (!loadMap && line.includes('menu open: iw4l_main'))) {
    loadDone = true;
    const ms = Number((line.match(/load complete: (\d+)ms/) || [])[1]);
    if (ms) postMessage({ type: 'loaded', ms });
    reportProgress('ready', 100);
    if (globalThis.iw4Games) finishRecording(); else fetch('/__done').catch(() => {});
    if (releaseHook) releaseHook();
    return;
  }
  const i = line.indexOf('load stage: ');
  if (i < 0) return;
  const rest = line.slice(i + 12);
  const d = rest.indexOf(' done');
  if (d < 0) return; // `at=` lines from the world report
  if (line.includes('world report:')) return;
  const name = rest.slice(0, d);
  const info = STAGE_INFO.get(name);
  if (info) reportProgress(info.next, info.pct);
  else reportProgress('loading', Math.min(98, loadPct + 0.5));
}

// Drop the prefetch cache (hundreds of MB of fetched ranges the engine has consumed) and the
// load-only blob entries. The Rust side only checks that globalThis.__iw4_cache is truthy.
let cacheTimer = 0;
function releaseAfterLoad() {
  if (memReleased) return;
  memReleased = true;
  const c = globalThis.__iw4_cache;
  if (cacheTimer) { clearInterval(cacheTimer); cacheTimer = 0; }
  if (c) {
    const mb = Math.round(c.bytes / 1048576);
    c.files.clear(); c.bytes = 0;
    globalThis.__iw4_cache = null;
    console.log('[prefetch] cache released after load (' + mb + ' MiB unconsumed)');
  }
  if (storeApi) { storeApi.releaseLoadOnlyEntries(); postCacheInfo(); }
}
releaseHook = releaseAfterLoad;

// ---- cache management (see PROTOCOL.md) ----
let storeApi = null;
let resolveStore; const storeReady = new Promise(r => { resolveStore = r; });
function postCacheInfo() { if (storeApi) postMessage({ type: 'cache-info', ...storeApi.cacheInfo() }); }
async function onCacheClear() {
  await storeReady;
  await storeApi.clearCache();
  postMessage({ type: 'cache-cleared' });
  postCacheInfo();
}

// Input inbox, drained once per frame by Rust through globalThis.__iw4_drain().
globalThis.__iw4_inbox = [];
globalThis.__iw4_audio = { state: 0, consumed: 0, queued: 0, reportAt: 0 };
const INPUT = new Set(['key', 'mousebtn', 'mousepos', 'motion', 'wheel', 'resize', 'focus', 'cursorin', 'pad']);
let engineStarted = false;
const MAX_PRE_START = 512; // bound the inbox while the engine is still loading

function onInput(m) {
  const inbox = globalThis.__iw4_inbox;
  if (!engineStarted && inbox.length >= MAX_PRE_START && (m.type === 'motion' || m.type === 'mousepos' || m.type === 'wheel')) return;
  if (!engineStarted && m.type === 'pad') {
    // Pad messages are full snapshots: before the engine runs keep only the latest per pad.
    for (let i = inbox.length - 1; i >= 0; i--) if (inbox[i].type === 'pad' && inbox[i].index === m.index) { inbox[i] = m; return; }
  }
  inbox.push(m);
}

onmessage = (e) => {
  const m = e.data;
  if (!m || typeof m.type !== 'string') return;
  if (m.type === 'boot') { boot(m).catch(err => fail(err)); return; }
  if (INPUT.has(m.type)) { onInput(m); return; }
  // The page tells us when it is hidden: animation frames stop then, so the engine ticks on a timer.
  if (m.type === 'visibility') { globalThis.__iw4_hidden = !!m.hidden; return; }
  if (m.type === 'cache-clear') { onCacheClear().catch(err => console.warn('cache-clear failed', String(err))); return; }
  if (m.type === 'audio-state') { globalThis.__iw4_audio.state = m.state; return; }
  if (m.type === 'audio-progress') {
    const a = globalThis.__iw4_audio;
    a.consumed = m.consumed; a.queued = m.queued; a.reportAt = performance.now();
  }
};

function fail(err) {
  const message = String(err && (err.stack || err.message) || err);
  console.error('worker failed', message);
  postMessage({ type: 'failed', message });
}

class RangeCache {
  constructor() { this.files = new Map(); this.bytes = 0; this.hits = 0; this.misses = 0; }
  add(url, start, data) { let l = this.files.get(url); if (!l) { l = []; this.files.set(url, l); } l.push({ start, end: start + data.length - 1, data, served: 0 }); this.bytes += data.length; }
  seal() { for (const l of this.files.values()) l.sort((a, b) => a.start - b.start); }
  read(url, start, end) {
    const l = this.files.get(url);
    if (l) {
      let lo = 0, hi = l.length - 1;
      while (lo <= hi) {
        const m = (lo + hi) >> 1; const c = l[m];
        if (end < c.start) hi = m - 1;
        else if (start > c.end) lo = m + 1;
        else {
          if (start >= c.start && end <= c.end) {
            c.served += end - start + 1; const out = c.data.subarray(start - c.start, end - c.start + 1);
            if (c.served >= c.data.length) { l.splice(m, 1); this.bytes -= c.data.length; }
            this.hits++; return out;
          }
          break;
        }
      }
    }
    this.misses++; return null;
  }
}

// ---- folder mode: browser-side prefetch manifests (see PROTOCOL.md) ----
// Cache keys must match what Rust passes to iw4_xhr_range: the URL-encoded path (fs.rs `enc`).
const encKey = (p) => p.replace(/%/g, '%25').replace(/#/g, '%23').replace(/\?/g, '%3F').replace(/ /g, '%20');
let recorder = null; // { map, ranges: Map<path, Array<[a,b]>> }

function startRecording(map) {
  recorder = { map, ranges: new Map() };
  const raw = globalThis.iw4Games.read;
  // Reads that reach iw4Games.read already missed the prefetch cache (fs.rs checks it first).
  globalThis.iw4Games.read = (p, a, b) => {
    const r = raw(p, a, b);
    if (recorder && typeof r !== 'number' && r.length) {
      let l = recorder.ranges.get(p); if (!l) { l = []; recorder.ranges.set(p, l); }
      l.push([a, a + r.length - 1]);
    }
    return r;
  };
}

function finishRecording() {
  if (!recorder) return;
  const rec = recorder; recorder = null;
  const files = []; let total = 0;
  for (const [url, list] of rec.ranges) {
    list.sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const [a, b] of list) {
      const last = merged[merged.length - 1];
      if (last && a <= last[1] + 1 && Math.max(b, last[1]) - last[0] < 8 * 1024 * 1024) last[1] = Math.max(b, last[1]);
      else merged.push([a, b]);
    }
    for (const [a, b] of merged) total += b - a + 1;
    files.push([url, merged]);
  }
  try {
    globalThis.__iw4_blob_put('manifest/' + rec.map, new TextEncoder().encode(JSON.stringify({ map: rec.map, total, files })));
    console.log('[prefetch] saved manifest ' + rec.map + ': ' + files.length + ' files, ' + Math.round(total / 1048576) + ' MB');
  } catch (e) { console.warn('manifest save failed', String(e)); }
}

async function prefetchFolder(map, blobs) {
  const name = map || 'menu';
  const raw = globalThis.iw4Games.read;
  let man = null;
  try { const b = blobs.get('manifest/' + name); if (b && b.length) man = JSON.parse(new TextDecoder().decode(b)); } catch (e) { console.warn('bad manifest', String(e)); }
  if (!man || !man.files) {
    reportProgress('first visit: recording what ' + name + ' needs...', 5);
    startRecording(name);
    return null;
  }
  const cache = new RangeCache(); let done = 0; const total = man.total || 1;
  const t0 = performance.now(); let n = 0, lastYield = t0;
  for (const [url, ranges] of man.files) {
    for (const [a, b] of ranges) {
      const r = raw(url, a, b); n++;
      if (typeof r === 'number') { console.warn('prefetch miss', url, a, b, r); continue; }
      cache.add(encKey(url), a, r); done += b - a + 1;
      if (performance.now() - lastYield > 100) {
        reportProgress('reading ' + name + ': ' + Math.round(done / 1048576) + ' / ' + Math.round(total / 1048576) + ' MB', Math.min(1, done / total) * PREFETCH_PCT);
        await new Promise((r2) => setTimeout(r2, 0)); lastYield = performance.now();
      }
    }
  }
  cache.seal(); console.log('[prefetch] folder ' + Math.round(done / 1048576) + ' MB in ' + Math.round(performance.now() - t0) + ' ms, ' + n + ' reads');
  return cache;
}

// Prefetch phase: fetch the byte ranges recorded for this map in parallel (progress), then start.
async function prefetch(map) {
  const res = await fetch('/__start?map=' + encodeURIComponent(map || 'menu'));
  const man = await res.json();
  if (!man.files) { reportProgress('first visit: recording what ' + (map || 'menu') + ' needs (slow once)...', 5); return null; }
  const jobs = []; for (const [url, ranges] of man.files) for (const [a, b] of ranges) jobs.push([url, a, b]);
  const cache = new RangeCache(); let done = 0, next = 0; const total = man.total;
  const t0 = performance.now();
  const work = async () => {
    while (next < jobs.length) {
      const [url, a, b] = jobs[next++];
      try { const r = await fetch(url, { headers: { Range: 'bytes=' + a + '-' + b } }); const buf = new Uint8Array(await r.arrayBuffer()); cache.add(url, a, buf); done += b - a + 1; }
      catch (e) { console.warn('prefetch miss', url, a, b, String(e)); }
      reportProgress('downloading ' + (map || 'menu') + ': ' + Math.round(done / 1048576) + ' / ' + Math.round(total / 1048576) + ' MB', Math.min(1, done / total) * PREFETCH_PCT);
    }
  };
  await Promise.all(Array.from({ length: 24 }, work));
  cache.seal(); console.log('[prefetch] ' + Math.round(done / 1048576) + ' MB in ' + Math.round(performance.now() - t0) + ' ms, ' + jobs.length + ' requests');
  return cache;
}

async function boot({ canvas, map, width, height, scale, legacyFs, games }) {
  map = map || undefined;
  loadMap = map || null;
  reportProgress('opening storage...', 0);
  storeApi = await import('./blobstore.js');
  const blobs = await storeApi.openBlobStore((m) => console.log(m), postCacheInfo);
  resolveStore();
  postCacheInfo();
  // One-time move of the main-thread build's saved files (localStorage) into this store; they win
  // over anything the worker build wrote before the move.
  if (legacyFs && legacyFs.length && !blobs.has('meta:fs-migrated') && globalThis.__iw4_blob_put) {
    for (const [path, bytes] of legacyFs) globalThis.__iw4_blob_put('fs:' + path, bytes);
    globalThis.__iw4_blob_put('meta:fs-migrated', new Uint8Array([1]));
    console.log('[blobs] migrated ' + legacyFs.length + ' saved files from localStorage');
  }
  reportProgress('downloading ' + (map || 'menu') + '...', 0);
  let cache;
  if (games && games.mode === 'folder') {
    installGames(games);
    cache = await prefetchFolder(map, blobs);
  } else cache = await prefetch(map);
  if (cache) {
    globalThis.__iw4_cache = cache;
    cacheTimer = setInterval(() => console.log('[prefetch] cache hits=' + cache.hits + ' misses=' + cache.misses + ' held_mb=' + Math.round(cache.bytes / 1048576)), 5000);
  }
  reportProgress('loading engine...', PREFETCH_PCT);
  const mod = await import('./pkg/web.js');
  await mod.default();
  console.log('wasm loaded (worker)');
  postMessage({ type: 'started' });
  engineStarted = true;
  try {
    mod.worker_main(canvas, map, width, height, scale);
  } catch (err) {
    fail(err);
  }
}
