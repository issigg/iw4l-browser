// WASM-SPIKE dev server: static page + read-only game tree with Range support.
// usage: node serve.mjs <path-to-MW2-folder> [port]
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const GAME = path.resolve(process.argv[2] || '.');
const PORT = Number(process.argv[3] || 8137);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json' };

// /games/iw4/<rest> -> GAME/<rest>; /games lists only `iw4`.
function real(p) {
  const parts = p.split('/').filter(Boolean);
  if (parts[0] !== 'games') return null;
  if (parts.length === 1) return { virtual: true };
  if (parts[1] !== 'iw4') return null;
  const full = path.resolve(GAME, ...parts.slice(2));
  if (full !== GAME && !full.startsWith(GAME + path.sep)) return null;
  return { full };
}
// Prefetch manifests: the first visit to ?map=X records the byte ranges the engine reads; later
// visits fetch them in parallel up front (with a progress bar) so the engine's blocking reads
// are answered from memory.
const PREFETCH_DIR = path.join(HERE, 'prefetch');
let rec = null; // { map, ranges: Map<url, Array<[start,end]>> }
function saveManifest() {
  if (!rec) return;
  const files = [];
  let total = 0;
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
  fs.mkdirSync(PREFETCH_DIR, { recursive: true });
  fs.writeFileSync(path.join(PREFETCH_DIR, rec.map + '.json'), JSON.stringify({ map: rec.map, total, files }));
  console.log(`prefetch manifest ${rec.map}: ${files.length} files, ${(total / 1048576).toFixed(0)} MB`);
  rec = null;
}
const json = (res, code, v) => { res.writeHead(code, { 'content-type': 'application/json', 'cache-control': 'no-store' }); res.end(JSON.stringify(v)); };

http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  if (!req.url.startsWith('/__log') && !req.url.startsWith('/pkg')) fs.appendFileSync('/tmp/iw4_req.log', `${req.method} ${req.url} ${req.headers.range || ''}\n`);
  const p = decodeURIComponent(url.pathname);
  if (p === '/__start') {
    const map = (url.searchParams.get('map') || 'menu').replace(/[^a-z0-9_]/gi, '');
    const file = path.join(PREFETCH_DIR, map + '.json');
    if (fs.existsSync(file) && !url.searchParams.has('rerecord')) { rec = null; res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' }); return fs.createReadStream(file).pipe(res); }
    rec = { map, ranges: new Map() };
    return json(res, 200, { record: true });
  }
  if (p === '/__done') { saveManifest(); return json(res, 200, { ok: true }); }
  if (p === '/__log' && req.method === 'POST') {
    let b = ''; req.on('data', (d) => (b += d)); req.on('end', () => { fs.appendFileSync('/tmp/iw4_browser.log', b + '\n'); res.writeHead(204); res.end(); });
    return;
  }
  if (p === '/__stat' || p === '/__ls') {
    const q = url.searchParams.get('p') || '';
    const r = real(q);
    if (!r) return json(res, 404, { exists: false });
    if (r.virtual) return p === '/__stat' ? json(res, 200, { exists: true, dir: true, size: 0 }) : json(res, 200, [{ name: 'iw4', dir: true, size: 0 }]);
    let st; try { st = fs.statSync(r.full); } catch { return json(res, 404, { exists: false }); }
    if (p === '/__stat') return json(res, 200, { exists: true, dir: st.isDirectory(), size: st.size });
    if (!st.isDirectory()) return json(res, 404, []);
    return json(res, 200, fs.readdirSync(r.full, { withFileTypes: true }).map((e) => {
      let s = { size: 0 }; try { s = fs.statSync(path.join(r.full, e.name)); } catch {}
      return { name: e.name, dir: s.isDirectory?.() ?? e.isDirectory(), size: s.size };
    }));
  }
  const g = real(p);
  if (g && g.full) {
    let st; try { st = fs.statSync(g.full); } catch { res.writeHead(404); return res.end(); }
    if (!st.isFile()) { res.writeHead(404); return res.end(); }
    const m = /bytes=(\d+)-(\d*)/.exec(req.headers.range || '');
    let start = 0, end = st.size - 1, code = 200;
    if (m) { start = Number(m[1]); end = m[2] ? Math.min(Number(m[2]), st.size - 1) : st.size - 1; code = 206; }
    const h = { 'content-length': end - start + 1, 'accept-ranges': 'bytes', 'content-type': 'application/octet-stream' };
    if (code === 206) h['content-range'] = `bytes ${start}-${end}/${st.size}`;
    res.writeHead(code, h);
    if (rec && code === 206 && req.method === 'GET' && end >= start) {
      const list = rec.ranges.get(req.url) || [];
      list.push([start, end]);
      rec.ranges.set(req.url, list);
    }
    if (req.method === 'HEAD' || end < start) return res.end();
    return fs.createReadStream(g.full, { start, end }).pipe(res);
  }
  const f = path.join(HERE, p === '/' ? 'index.html' : p);
  if (!f.startsWith(HERE) || !fs.existsSync(f) || fs.statSync(f).isDirectory()) { res.writeHead(404); return res.end(); }
  res.writeHead(200, { 'content-type': MIME[path.extname(f)] || 'application/octet-stream', 'cache-control': 'no-store' });
  fs.createReadStream(f).pipe(res);
}).listen(PORT, '127.0.0.1', () => console.log(`http://localhost:${PORT}/ serving ${GAME} at /games/iw4`));
