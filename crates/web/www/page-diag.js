// Page-side diagnostics state, startup capability check and plain-text debug report.
// No personal data is collected (no local paths, no file names).
export const diag = {
  errors: [],          // last 30 error/panic/warn lines
  fps: null,           // last {fps, worst, memMB}
  memPeakMB: 0,
  loadedMs: 0,
  cache: null,         // last cache-info message
  adapter: null,       // {vendor, architecture, device, description} or null
  adapterNote: '',
  warnings: [],        // non-fatal startup findings
  gamesMode: 'server',
  render: () => ({}),  // set by index.html: () => ({width,height,scale,quality})
  onChange: null,
};

const hhmmss = () => new Date().toISOString().slice(11, 19);
export function addError(kind, text) {
  let t;
  try { t = typeof text === 'string' ? text : (text && (text.stack || text.message)) || String(text); } catch (_) { t = '?'; }
  diag.errors.push(hhmmss() + ' [' + kind + '] ' + t.slice(0, 600));
  while (diag.errors.length > 30) diag.errors.shift();
  if (diag.onChange) diag.onChange();
}
addEventListener('error', (e) => addError('window.error', (e.message || 'error') + (e.filename ? ' @' + String(e.filename).split('/').pop() + ':' + e.lineno : '')));
addEventListener('unhandledrejection', (e) => addError('unhandledrejection', e.reason));

export function browserBrand() {
  const ud = navigator.userAgentData;
  let brand = '';
  try {
    if (ud && ud.brands) brand = ud.brands.filter((b) => !/not.?a.?brand/i.test(b.brand)).map((b) => b.brand + ' ' + b.version).join(', ');
  } catch (_) {}
  return brand || 'unknown';
}
export function isChromiumLike() {
  try {
    const ud = navigator.userAgentData;
    if (ud && ud.brands && ud.brands.some((b) => /chromium/i.test(b.brand))) return true;
  } catch (_) {}
  return /Chrome\/|Chromium\//.test(navigator.userAgent) && !/Firefox\//.test(navigator.userAgent);
}

// Returns {ok, problems: string[]}. Fills diag.adapter / diag.warnings.
export async function startupCheck() {
  const problems = [];
  const hint = 'Use a recent desktop Chrome, Edge or another Chromium-based browser (Chrome 113+ with WebGPU enabled).';
  if (!window.isSecureContext) problems.push('This page is not in a secure context (WebGPU needs https:// or http://localhost). Open it via localhost or HTTPS.');
  if (typeof Worker === 'undefined') problems.push('Web Workers are not available.');
  if (typeof OffscreenCanvas === 'undefined') problems.push('OffscreenCanvas is not supported by this browser.');
  if (!(window.HTMLCanvasElement && HTMLCanvasElement.prototype.transferControlToOffscreen)) problems.push('canvas.transferControlToOffscreen() is not supported by this browser.');
  if (!navigator.gpu) {
    problems.push('WebGPU (navigator.gpu) is not available in this browser.');
  } else {
    let adapter = null;
    try { adapter = await navigator.gpu.requestAdapter(); } catch (e) { diag.adapterNote = 'requestAdapter threw: ' + (e && e.message || e); }
    if (!adapter) {
      problems.push('WebGPU is present but no GPU adapter was returned (blocked GPU, disabled hardware acceleration, or unsupported driver). Check chrome://gpu and that "Use hardware acceleration" is enabled.');
    } else {
      try {
        let info = adapter.info;
        if (!info && adapter.requestAdapterInfo) info = await adapter.requestAdapterInfo();
        if (info) diag.adapter = { vendor: info.vendor || '', architecture: info.architecture || '', device: info.device || '', description: info.description || '' };
        else diag.adapterNote = diag.adapterNote || 'adapter info not exposed';
      } catch (e) { diag.adapterNote = 'adapter info failed: ' + (e && e.message || e); }
    }
  }
  if (!isChromiumLike()) {
    const w = 'Browser does not look Chromium-based (' + browserBrand() + '); the engine is only tested in Chrome/Edge.';
    if (!navigator.gpu) problems.push('Detected browser: ' + (navigator.userAgent || '?').slice(0, 120));
    else diag.warnings.push(w);
  }
  if (problems.length) problems.push(hint);
  return { ok: problems.length === 0, problems };
}

export function buildReport() {
  const r = (() => { try { return diag.render() || {}; } catch (_) { return {}; } })();
  const L = [];
  L.push('iw4l-browser debug info (' + new Date().toISOString() + ')');
  L.push('browser: ' + browserBrand());
  L.push('user agent: ' + navigator.userAgent);
  L.push('platform: ' + (navigator.userAgentData && navigator.userAgentData.platform || navigator.platform || '?'));
  L.push('secure context: ' + window.isSecureContext + '   crossOriginIsolated: ' + window.crossOriginIsolated);
  if (diag.adapter) {
    L.push('gpu vendor: ' + (diag.adapter.vendor || '?') + '   architecture: ' + (diag.adapter.architecture || '?'));
    L.push('gpu device: ' + (diag.adapter.device || '?') + '   description: ' + (diag.adapter.description || '?'));
  } else L.push('gpu: ' + (diag.adapterNote || 'unknown'));
  L.push('render size: ' + (r.width || '?') + 'x' + (r.height || '?') + '   render scale: ' + (r.scale != null ? (+r.scale).toFixed(3) : '?') + '   quality setting: ' + (r.quality || '?'));
  L.push('devicePixelRatio: ' + (window.devicePixelRatio || 1) + '   css size: ' + window.innerWidth + 'x' + window.innerHeight);
  L.push('games source: ' + diag.gamesMode);
  L.push(diag.fps ? 'fps: ' + diag.fps.fps.toFixed(1) + '   worst frame: ' + Math.round(diag.fps.worst) + ' ms   mem: ' + Math.round(diag.fps.memMB) + ' MB (peak ' + Math.round(diag.memPeakMB) + ' MB)' : 'fps: n/a');
  L.push('last load time: ' + (diag.loadedMs ? (diag.loadedMs / 1000).toFixed(1) + ' s' : 'n/a'));
  L.push('cache: ' + (diag.cache ? diag.cache.entries + ' entries, ' + Math.round(diag.cache.bytes / 1048576) + ' MiB of ' + Math.round(diag.cache.limit / 1048576) + ' MiB' : 'n/a'));
  if (diag.warnings.length) L.push('warnings: ' + diag.warnings.join(' | '));
  L.push('last errors (' + diag.errors.length + '):');
  if (!diag.errors.length) L.push('  (none)');
  for (const e of diag.errors) L.push('  ' + e);
  return L.join('\n');
}

export async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch (_) {}
  try {
    const ta = document.createElement('textarea');
    ta.value = text; ta.style.cssText = 'position:fixed;left:-9999px;top:0';
    document.body.appendChild(ta); ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return !!ok;
  } catch (_) { return false; }
}
