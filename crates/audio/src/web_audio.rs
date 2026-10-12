//! wasm audio transport: the lock-free mixer runs on the main thread and its
//! PCM is pushed to a Web Audio `AudioWorklet` (see the inline JS module).

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use wasm_bindgen::prelude::*;
use web_time::Instant;

use crate::render_core::{QUANTUM, RenderShared, SAMPLE_RATE};

/// Lead (frames queued ahead of the worklet) the transport tries to keep.
const TARGET_LEAD_FRAMES: usize = SAMPLE_RATE as usize * 125 / 1000;
/// Never render more than this many quanta in one frame (about 170 ms).
const MAX_QUANTA_PER_PASS: usize = 64;

#[wasm_bindgen(inline_js = r#"
const WORKLET = `
class Iw4Out extends AudioWorkletProcessor {
  constructor() {
    super();
    this.q = []; this.head = 0; this.off = 0; this.queued = 0;
    this.consumed = 0; this.n = 0; this.tone = null;
    this.port.onmessage = (e) => {
      const d = e.data;
      if (d && d.tone) { this.tone = { f: d.tone.f, left: Math.floor(d.tone.secs * sampleRate), ph: 0 }; return; }
      this.q.push(d); this.queued += d.length >> 1;
    };
  }
  process(inputs, outputs) {
    const out = outputs[0], l = out[0], r = out[1] || out[0], n = l.length;
    let i = 0;
    while (i < n && this.head < this.q.length) {
      const c = this.q[this.head];
      const have = (c.length >> 1) - this.off, take = Math.min(have, n - i);
      for (let k = 0; k < take; k++) {
        l[i + k] = c[(this.off + k) * 2]; r[i + k] = c[(this.off + k) * 2 + 1];
      }
      i += take; this.off += take; this.queued -= take; this.consumed += take;
      if (this.off >= (c.length >> 1)) { this.head++; this.off = 0; }
    }
    for (; i < n; i++) { l[i] = 0; r[i] = 0; }
    if (this.head > 32) { this.q.splice(0, this.head); this.head = 0; }
    if (this.tone) {
      const t = this.tone;
      for (let k = 0; k < n && t.left > 0; k++, t.left--) {
        const v = 0.2 * Math.sin(t.ph); t.ph += 2 * Math.PI * t.f / sampleRate;
        l[k] += v; r[k] += v;
      }
      if (t.left <= 0) this.tone = null;
    }
    if ((++this.n & 3) === 0) this.port.postMessage({ consumed: this.consumed, time: currentTime, queued: this.queued });
    return true;
  }
}
registerProcessor('iw4-out', Iw4Out);
`;

const A = globalThis.__iw4_audio = globalThis.__iw4_audio || {
  ctx: null, node: null, ready: false, started: false, listening: false,
  pushed: 0, consumed: 0, reportTime: 0,
};

async function start() {
  if (A.started) return;
  A.started = true;
  try {
    const ctx = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
    A.ctx = ctx;
    ctx.resume().catch(() => {});
    const url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
    await ctx.audioWorklet.addModule(url);
    URL.revokeObjectURL(url);
    const node = new AudioWorkletNode(ctx, 'iw4-out', { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2] });
    node.port.onmessage = (e) => {
      A.consumed = e.data.consumed; A.reportTime = e.data.time;
    };
    node.connect(ctx.destination);
    A.node = node; A.ready = true;
    ctx.resume().catch(() => {});
  } catch (e) {
    console.error('iw4 audio init failed', e);
    A.started = false; A.failed = true;
  }
}

const IN_WORKER = typeof window === "undefined";
const W = { pushed: 0, firstPush: 0, inited: false };

export function iw4_audio_init() {
  if (IN_WORKER) {
    if (W.inited) return;
    W.inited = true;
    globalThis.__iw4_audio = globalThis.__iw4_audio || { state: 0, consumed: 0, queued: 0, reportAt: 0 };
    self.postMessage({ type: 'audio-init' });
    globalThis.iw4_audio_sine = (f = 440, secs = 1) => self.postMessage({ type: 'audio-tone', f, secs });
    return;
  }
  if (A.listening) return;
  A.listening = true;
  const go = () => {
    if (!A.started && !A.failed) start();
    if (A.ctx && A.ctx.state !== 'running') A.ctx.resume().catch(() => {});
  };
  for (const ev of ['pointerdown', 'keydown', 'touchend', 'mousedown'])
    window.addEventListener(ev, go, { capture: true, passive: true });
  globalThis.iw4_audio_sine = (freq = 440, secs = 1) => {
    go();
    if (A.node) A.node.port.postMessage({ tone: { f: freq, secs } });
    else console.log('iw4 audio: not ready yet (click the page first)');
  };
}

// 0 = no context, 1 = context pending or suspended, 2 = running.
export function iw4_audio_state() {
  if (IN_WORKER) return (globalThis.__iw4_audio && globalThis.__iw4_audio.state) | 0;
  if (!A.ctx) return 0;
  return A.ready && A.ctx.state === 'running' ? 2 : 1;
}

export function iw4_audio_buffered_frames() {
  if (IN_WORKER) {
    const a = globalThis.__iw4_audio;
    if (!a || !W.firstPush) return 0;
    const consumed = a.reportAt ? a.consumed : 0;
    const at = a.reportAt || W.firstPush;
    return Math.max(0, Math.floor(W.pushed - consumed - (performance.now() - at) / 1000 * 48000));
  }
  if (!A.ready || !A.ctx) return 0;
  const since = Math.max(0, A.ctx.currentTime - A.reportTime) * 48000;
  return Math.max(0, Math.floor(A.pushed - A.consumed - since));
}

export function iw4_audio_push(samples) {
  if (IN_WORKER) {
    const a = globalThis.__iw4_audio;
    if (!a || a.state !== 2) return;
    if (!W.firstPush) W.firstPush = performance.now();
    const copy = samples.slice();
    W.pushed += copy.length >> 1;
    self.postMessage({ type: 'audio-push', samples: copy }, [copy.buffer]);
    return;
  }
  if (!A.ready) return;
  const c = samples.slice();
  A.pushed += c.length >> 1;
  A.node.port.postMessage(c, [c.buffer]);
}
"#)]
extern "C" {
    fn iw4_audio_init();
    fn iw4_audio_state() -> u32;
    fn iw4_audio_buffered_frames() -> f64;
    fn iw4_audio_push(samples: &[f32]);
}

thread_local! {
    static LAST_STATE: Cell<u32> = const { Cell::new(0) };
    static PUSHED_FRAMES: Cell<u64> = const { Cell::new(0) };
    static LAST_LOG: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Installs the first-gesture listeners; idempotent.
pub(crate) fn init() {
    iw4_audio_init();
}

/// True once the AudioContext is running and the worklet is attached.
pub(crate) fn running() -> bool {
    let state = iw4_audio_state();
    LAST_STATE.with(|last| {
        if last.replace(state) != state {
            let what = match state {
                0 => "no AudioContext (waiting for a click or key press)",
                1 => "AudioContext created, worklet loading or context suspended",
                _ => "AudioContext running, worklet attached",
            };
            diag::info!(Audio, "audio: web transport state={state}: {what}");
        }
    });
    state == 2
}

/// Renders enough quanta to restore the lead and pushes them to the worklet.
pub(crate) fn render_and_push(shared: &Arc<RenderShared>) {
    let buffered = iw4_audio_buffered_frames() as usize;
    let missing = TARGET_LEAD_FRAMES.saturating_sub(buffered);
    let quanta = missing.div_ceil(QUANTUM).min(MAX_QUANTA_PER_PASS);
    if quanta != 0 {
        let mut block = [[0.0f32; 2]; QUANTUM];
        let mut out = Vec::with_capacity(quanta * QUANTUM * 2);
        for _ in 0..quanta {
            shared.render_for(&mut block, Some(true));
            for frame in &block {
                out.extend_from_slice(frame);
            }
        }
        if PUSHED_FRAMES.with(Cell::get) == 0 {
            diag::info!(
                Audio,
                "audio: first PCM pushed to the worklet ({} frames)",
                out.len() / 2
            );
        }
        iw4_audio_push(&out);
        PUSHED_FRAMES.with(|total| total.set(total.get() + (out.len() / 2) as u64));
    }
    let now = Instant::now();
    LAST_LOG.with(|last| {
        let due = last
            .get()
            .is_none_or(|at| now.duration_since(at).as_secs() >= 5);
        if due {
            last.set(Some(now));
            diag::info!(
                Audio,
                "audio: web transport pushed_frames={} buffered_ms={:.0} mixer_frame={} underruns={}",
                PUSHED_FRAMES.with(Cell::get),
                buffered as f64 * 1000.0 / f64::from(SAMPLE_RATE),
                shared.frame.load(Ordering::Acquire),
                shared.device_underruns.load(Ordering::Relaxed)
            );
        }
    });
}
