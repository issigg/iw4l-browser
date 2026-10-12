// Worker side of folder mode: serves the user's picked MW2 folder as the virtual tree
// /games/iw4/<rel>, with the same semantics as serve.mjs (/__stat, /__ls, ranged GET).
// installGames(games) sets globalThis.iw4Games = { stat, ls, read } (all synchronous).

export function installGames(games) {
  const files = new Map();      // exact rel path -> File
  const dirs = new Map();       // exact rel dir ('' = root) -> Map(name -> { dir, size })
  const lowerFiles = new Map(); // lowercase rel path -> exact
  const lowerDirs = new Map();
  dirs.set('', new Map());
  for (const [rel, file] of games.files) {
    const parts = rel.split('/').filter(Boolean);
    if (!parts.length) continue;
    const full = parts.join('/');
    files.set(full, file);
    lowerFiles.set(full.toLowerCase(), full);
    let dir = '';
    for (let i = 0; i < parts.length - 1; i++) {
      const next = dir ? dir + '/' + parts[i] : parts[i];
      if (!dirs.has(next)) { dirs.set(next, new Map()); lowerDirs.set(next.toLowerCase(), next); }
      dirs.get(dir).set(parts[i], { dir: true, size: 0 });
      dir = next;
    }
    dirs.get(dir).set(parts[parts.length - 1], { dir: false, size: file.size });
  }

  // Returns null (not ours / missing), 'virtual' (/games), or { rel }.
  function resolve(p) {
    const parts = String(p).split('/').filter(Boolean);
    if (parts[0] !== 'games') return null;
    if (parts.length === 1) return 'virtual';
    if (parts[1] !== 'iw4') return null;
    const rest = parts.slice(2);
    if (rest.some((s) => s === '..')) return null;
    return { rel: rest.filter((s) => s !== '.').join('/') };
  }
  function lookup(rel) {
    if (files.has(rel)) return { file: files.get(rel), rel };
    if (dirs.has(rel)) return { dir: dirs.get(rel), rel };
    const l = rel.toLowerCase();
    if (lowerFiles.has(l)) { const r = lowerFiles.get(l); return { file: files.get(r), rel: r }; }
    if (lowerDirs.has(l)) { const r = lowerDirs.get(l); return { dir: dirs.get(r), rel: r }; }
    return null;
  }

  const api = {
    stat(p) {
      const r = resolve(p);
      if (!r) return { exists: false };
      if (r === 'virtual') return { exists: true, dir: true, size: 0 };
      const h = lookup(r.rel);
      if (!h) return { exists: false };
      return h.file ? { exists: true, dir: false, size: h.file.size } : { exists: true, dir: true, size: 0 };
    },
    ls(p) {
      const r = resolve(p);
      if (!r) return null;
      if (r === 'virtual') return [{ name: 'iw4', dir: true, size: 0 }];
      const h = lookup(r.rel);
      if (!h || !h.dir) return null;
      return [...h.dir].map(([name, e]) => ({ name, dir: e.dir, size: e.size }));
    },
    read(p, start, end) {
      const r = resolve(p);
      if (!r || r === 'virtual') return 404;
      const h = lookup(r.rel);
      if (!h || !h.file) return 404;
      const size = h.file.size;
      end = Math.min(end, size - 1);
      if (end < start) return new Uint8Array(0);
      try {
        return new Uint8Array(new FileReaderSync().readAsArrayBuffer(h.file.slice(start, end + 1)));
      } catch (e) {
        console.warn('iw4Games.read failed', p, start, end, String(e));
        return 500;
      }
    },
  };
  globalThis.iw4Games = api;
  console.log('[games] folder mode: ' + games.name + ', ' + files.size + ' files');
  return api;
}
