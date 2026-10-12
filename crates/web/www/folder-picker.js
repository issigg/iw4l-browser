// Page side of folder mode: pick (or re-open) the user's Modern Warfare 2 install folder via the
// File System Access API. getGamesSource() resolves null (dev server mode) or
// { mode: 'folder', name, files: [[relativePath, File], ...] }.

const DB = 'iw4-folder-picker', STORE = 'kv', KEY = 'iw4-games-handle';

function idb() {
  return new Promise((res, rej) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE);
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}
async function loadHandle() {
  try {
    const db = await idb();
    return await new Promise((res, rej) => {
      const q = db.transaction(STORE).objectStore(STORE).get(KEY);
      q.onsuccess = () => res(q.result || null);
      q.onerror = () => rej(q.error);
    });
  } catch (_) { return null; }
}
async function saveHandle(h) {
  try {
    const db = await idb();
    await new Promise((res, rej) => {
      const t = db.transaction(STORE, 'readwrite');
      t.objectStore(STORE).put(h, KEY);
      t.oncomplete = res; t.onerror = () => rej(t.error);
    });
  } catch (_) {}
}

async function devServerAvailable() {
  const q = new URLSearchParams(location.search);
  if (q.get('folder') === '1') return false; // force folder mode (e.g. to test it next to the dev server)
  if (q.get('server') === '1') return true;
  try {
    const r = await fetch('/__stat?p=/games/iw4', { cache: 'no-store' });
    if (!r.ok) return false;
    const j = await r.json();
    return !!(j && j.exists);
  } catch (_) { return false; }
}

async function hasDir(h, name) {
  try { await h.getDirectoryHandle(name); return true; }
  catch (_) {
    try { for await (const [n, e] of h.entries()) if (e.kind === 'directory' && n.toLowerCase() === name) return true; } catch (_) {}
    return false;
  }
}
async function validFolder(h) { return (await hasDir(h, 'zone')) && (await hasDir(h, 'main')); }

async function enumerate(root, onCount) {
  const found = [];
  async function walk(dir, prefix) {
    const subs = [];
    for await (const [name, h] of dir.entries()) {
      if (name.startsWith('.')) continue;
      if (h.kind === 'directory') subs.push([h, prefix + name + '/']);
      else found.push([prefix + name, h]);
    }
    for (const [h, p] of subs) await walk(h, p);
  }
  await walk(root, '');
  const out = new Array(found.length);
  let next = 0, done = 0;
  const work = async () => {
    while (next < found.length) {
      const i = next++;
      out[i] = [found[i][0], await found[i][1].getFile()];
      onCount(++done, found.length);
    }
  };
  await Promise.all(Array.from({ length: 16 }, work));
  return out;
}

function buildOverlay() {
  const style = document.createElement('style');
  style.textContent = `
#iw4-picker{position:fixed;inset:0;z-index:100;background:#111;display:flex;align-items:center;justify-content:center;color:#ddd;font:15px monospace;text-align:center}
#iw4-picker .box{max-width:520px;padding:24px}
#iw4-picker button{display:block;margin:12px auto;padding:10px 18px;background:#333;color:#ddd;border:1px solid #555;border-radius:5px;font:15px monospace;cursor:pointer}
#iw4-picker button:hover{background:#444;border-color:#7c7}
#iw4-picker .msg{margin:12px 0;color:#aaa;min-height:1.2em}
#iw4-picker .err{color:#e88}
#iw4-picker .hint{font-size:12px;color:#777}`;
  const root = document.createElement('div');
  root.id = 'iw4-picker';
  root.innerHTML = '<div class=box><div class=msg id=iw4-pmsg></div><div id=iw4-pbtns></div><div class=hint id=iw4-phint></div></div>';
  document.head.appendChild(style);
  document.body.appendChild(root);
  return { root, style,
    msg: (t, err) => { const m = root.querySelector('#iw4-pmsg'); m.textContent = t; m.className = 'msg' + (err ? ' err' : ''); },
    hint: (t) => { root.querySelector('#iw4-phint').textContent = t || ''; },
    buttons: (list) => {
      const b = root.querySelector('#iw4-pbtns'); b.textContent = '';
      for (const [label, fn] of list) { const x = document.createElement('button'); x.textContent = label; x.onclick = fn; b.appendChild(x); }
    },
    remove: () => { root.remove(); style.remove(); } };
}

export async function getGamesSource() {
  if (await devServerAvailable()) return null;
  const ui = buildOverlay();
  if (typeof window.showDirectoryPicker !== 'function') {
    ui.msg('This browser cannot open local folders. Use a Chromium-based browser (Chrome, Edge) to play from your own Modern Warfare 2 folder.', true);
    return new Promise(() => {});
  }
  const saved = await loadHandle();
  return new Promise((resolve) => {
    const finish = async (handle) => {
      ui.buttons([]);
      ui.hint('');
      ui.msg('Reading folder...');
      try {
        const files = await enumerate(handle, (n, t) => ui.msg('Reading folder: ' + n + ' / ' + t + ' files'));
        ui.remove();
        resolve({ mode: 'folder', name: handle.name, files });
      } catch (e) {
        ui.msg('Could not read the folder: ' + (e && e.message || e), true);
        offer();
      }
    };
    const accept = async (handle) => {
      if (!(await validFolder(handle))) {
        ui.msg('That folder does not look like Modern Warfare 2: it must contain the "zone" and "main" folders. Pick the game install folder.', true);
        return false;
      }
      await saveHandle(handle);
      await finish(handle);
      return true;
    };
    const choose = async () => {
      let h;
      try { h = await window.showDirectoryPicker({ mode: 'read' }); }
      catch (e) { if (e && e.name !== 'AbortError') ui.msg('Could not open the folder picker: ' + e.message, true); return; }
      await accept(h);
    };
    const useSaved = async () => {
      try {
        let p = await saved.queryPermission({ mode: 'read' });
        if (p !== 'granted') p = await saved.requestPermission({ mode: 'read' });
        if (p !== 'granted') { ui.msg('Permission to read "' + saved.name + '" was not granted.', true); return; }
      } catch (e) { ui.msg('Could not reopen the saved folder: ' + e.message, true); return; }
      await accept(saved);
    };
    function offer() {
      const list = [];
      if (saved) list.push(['Use saved folder: ' + saved.name, useSaved]);
      list.push(['Choose your Modern Warfare 2 folder', choose]);
      ui.buttons(list);
      ui.hint('Files stay on your computer; nothing is uploaded.');
    }
    if (!ui.root.querySelector('#iw4-pmsg').textContent) ui.msg('Select the folder where Modern Warfare 2 is installed (it contains "zone" and "main").');
    offer();
  });
}
