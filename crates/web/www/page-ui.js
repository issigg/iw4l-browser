// Small page UI widgets: corner controls (quality, info, fullscreen), diagnostics panel,
// pointer-capture hint, toasts, fatal startup message.
import { diag, buildReport, copyText } from './page-diag.js';

const FONT = '11px monospace';
const style = document.createElement('style');
style.textContent = `
.iw-btn{all:unset;cursor:pointer;color:#999;background:#000a;border:1px solid #444;border-radius:3px;padding:1px 6px;font:${FONT};opacity:.55}
.iw-btn:hover,.iw-btn:focus-visible{opacity:1;color:#ddd}
.iw-panel{position:fixed;z-index:20;background:#111e;color:#ccc;border:1px solid #444;border-radius:4px;padding:8px 10px;font:${FONT};display:none}
.iw-panel select{font:${FONT};background:#222;color:#ccc;border:1px solid #555}
`;
document.head.appendChild(style);

function el(tag, css, text) {
  const e = document.createElement(tag);
  if (css) e.style.cssText = css;
  if (text != null) e.textContent = text;
  return e;
}

// ---- fatal startup message ----
export function showFatal(title, lines) {
  const d = el('div', 'position:fixed;inset:0;z-index:100;background:#111;color:#ddd;display:flex;align-items:center;justify-content:center;padding:24px;box-sizing:border-box;font:14px monospace');
  const box = el('div', 'max-width:640px');
  box.appendChild(el('div', 'color:#f66;font-size:16px;margin-bottom:12px', title));
  const ul = el('ul', 'margin:0;padding-left:20px;line-height:1.5');
  for (const l of lines) ul.appendChild(el('li', 'margin-bottom:6px', l));
  box.appendChild(ul);
  d.appendChild(box);
  document.body.appendChild(d);
  const b = document.getElementById('boot'); if (b) b.style.display = 'none';
}

// ---- toast + pointer hint ----
const toastEl = el('div', 'position:fixed;left:50%;bottom:48px;transform:translateX(-50%);z-index:30;background:#000c;color:#ddd;border:1px solid #555;border-radius:4px;padding:5px 12px;font:12px monospace;pointer-events:none;display:none');
const hintEl = el('div', 'position:fixed;left:50%;top:50%;transform:translate(-50%,-50%);z-index:12;background:#000b;color:#eee;border:1px solid #666;border-radius:4px;padding:8px 16px;font:14px monospace;pointer-events:none;display:none', 'Click to capture the mouse');
document.body.append(toastEl, hintEl);
let toastTimer = 0;
export function toast(text, ms = 3000) {
  toastEl.textContent = text; toastEl.style.display = '';
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { toastEl.style.display = 'none'; }, ms);
}
export function setHint(visible, text) {
  if (text) hintEl.textContent = text;
  hintEl.style.display = visible ? '' : 'none';
}

// ---- corner controls ----
// opts: {quality:{get,set,options:[[value,label]]}, focusCanvas, getRender}
export function initControls(opts) {
  const bar = el('div', 'position:fixed;right:8px;bottom:6px;z-index:21;display:flex;gap:4px');
  const bQ = el('button', '', 'Quality'); bQ.type = 'button'; bQ.className = 'iw-btn'; bQ.title = 'Render resolution';
  const bI = el('button', '', 'info'); bI.type = 'button'; bI.className = 'iw-btn'; bI.title = 'Diagnostics (F2)';
  const bF = el('button', '', 'fullscreen'); bF.type = 'button'; bF.className = 'iw-btn'; bF.title = 'Fullscreen (Alt+Enter)';
  bar.append(bQ, bI, bF);
  document.body.appendChild(bar);

  // quality panel
  const pQ = el('div'); pQ.className = 'iw-panel'; pQ.style.cssText += ';right:8px;bottom:30px';
  pQ.appendChild(el('div', 'margin-bottom:4px;color:#999', 'Render resolution'));
  const sel = el('select');
  pQ.appendChild(sel);
  const note = el('div', 'margin-top:4px;color:#777');
  pQ.appendChild(note);
  document.body.appendChild(pQ);
  function fillSel() {
    sel.textContent = '';
    const cur = opts.quality.get();
    let found = false;
    for (const [v, label] of opts.quality.options) { const o = el('option', '', label); o.value = v; if (v === cur) { o.selected = true; found = true; } sel.appendChild(o); }
    if (!found) { const o = el('option', '', cur + 'x (custom)'); o.value = cur; o.selected = true; sel.appendChild(o); }
    const r = opts.getRender(); note.textContent = 'now ' + r.width + 'x' + r.height;
  }
  sel.addEventListener('change', () => { opts.quality.set(sel.value); fillSel(); sel.blur(); opts.focusCanvas(); });

  // diagnostics panel
  const pI = el('div'); pI.className = 'iw-panel'; pI.style.cssText += ';right:8px;bottom:30px;max-width:min(640px,92vw);max-height:70vh;overflow:auto';
  const pre = el('pre', 'margin:0 0 6px;white-space:pre-wrap;word-break:break-all;font:11px monospace');
  const row = el('div', 'display:flex;gap:6px;align-items:center');
  const bCopy = el('button', '', 'Copy debug info'); bCopy.type = 'button'; bCopy.className = 'iw-btn'; bCopy.style.opacity = '1';
  const bClose = el('button', '', 'close'); bClose.type = 'button'; bClose.className = 'iw-btn'; bClose.style.opacity = '1';
  const status = el('span', 'color:#7c7');
  row.append(bCopy, bClose, status);
  pI.append(pre, row);
  document.body.appendChild(pI);
  const refresh = () => { if (pI.style.display !== 'none') pre.textContent = buildReport(); };
  setInterval(refresh, 1000);
  diag.onChange = refresh;
  bCopy.addEventListener('click', async () => {
    const ok = await copyText(buildReport());
    status.textContent = ok ? 'copied' : 'copy failed';
    setTimeout(() => { status.textContent = ''; }, 2000);
  });

  const hide = (p) => { p.style.display = 'none'; };
  function toggle(p) {
    const open = p.style.display === 'none' || p.style.display === '';
    hide(pQ); hide(pI);
    if (open) { p.style.display = 'block'; if (p === pQ) fillSel(); else pre.textContent = buildReport(); }
    else opts.focusCanvas();
  }
  const toggleInfo = () => toggle(pI);
  bQ.addEventListener('click', () => toggle(pQ));
  bI.addEventListener('click', toggleInfo);
  bClose.addEventListener('click', () => { hide(pI); opts.focusCanvas(); });

  // fullscreen
  const isFs = () => !!document.fullscreenElement;
  async function toggleFullscreen() {
    try {
      if (isFs()) await document.exitFullscreen();
      else await document.documentElement.requestFullscreen({ navigationUI: 'hide' });
    } catch (e) { toast('Fullscreen failed: ' + (e && e.message || e), 4000); }
    opts.focusCanvas();
  }
  bF.addEventListener('click', toggleFullscreen);
  document.addEventListener('fullscreenchange', () => { bF.textContent = isFs() ? 'exit fullscreen' : 'fullscreen'; });
  document.addEventListener('fullscreenerror', () => toast('Fullscreen was refused by the browser', 4000));

  // F2 / Alt+Enter: capture phase, before the engine key forwarder, and not forwarded to the engine.
  addEventListener('keydown', (e) => {
    if (e.code === 'F2' && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault(); e.stopImmediatePropagation();
      if (!e.repeat) toggleInfo();
    } else if (e.code === 'Enter' && e.altKey && !e.ctrlKey && !e.metaKey) {
      e.preventDefault(); e.stopImmediatePropagation();
      if (!e.repeat) toggleFullscreen();
    }
  }, true);
  return { fillSel };
}
