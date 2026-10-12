// Persistent store for the engine's derived artifacts (translated shaders, bot navigation,
// localization; see crates/vfs/src/blobcache.rs). One pack file in the origin-private file
// system, read whole before the engine starts; new entries are appended in batches in the
// background. Records: [u32 key bytes][key][u32 data bytes][data], little-endian; a later record
// for the same key wins. Everything here is the engine's own output, never game data.
//
// Size management: the pack is append-only, so eviction happens by compaction at open time. If the
// pack is over LIMIT it is rewritten keeping the most recently used entries until it is under
// ~75% of LIMIT. Drop order: `mips/` entries first (oldest first), then everything else (oldest
// first); `fs:` (profile/settings) and `meta:` entries are never evicted. "Last used" comes from a
// sidecar JSON file (key -> epoch ms) that is stamped when an entry is first read through the
// Map (`get`, once per page load) or written (`put`). Tradeoff: entries never read since the
// sidecar was introduced have no stamp and count as oldest, ordered by their position in the pack
// (earlier = older); the stamp is per page load, not per access, to keep `get` cheap.
const PACK = "iw4l-blobs-v1.pack";
const META = "iw4l-blobs-v1.meta.json";
/** Above this the pack is compacted at open. */
const LIMIT = 1536 * 1024 * 1024;
/** Compaction target. */
const TARGET = Math.floor(LIMIT * 0.75);
/** Entries that are never evicted or wiped by the cache-clear button (user settings). */
const KEEP_PREFIXES = ["fs:", "meta:", "manifest/"];
/** Decoded images: only needed while a map loads, large, and cheap to regenerate. */
export const LOAD_ONLY_PREFIXES = ["mips/"];

const isKept = (key) => KEEP_PREFIXES.some((p) => key.startsWith(p));

/** Map that records when entries are first read in this page load (for last-used eviction). */
class TrackedMap extends Map {
  constructor(stamps) {
    super();
    this.stamps = stamps;
    this.seen = new Set();
    this.metaDirty = false;
  }
  get(key) {
    const v = super.get(key);
    if (v !== undefined && !this.seen.has(key)) {
      this.seen.add(key);
      this.stamps.set(key, Date.now());
      this.metaDirty = true;
    }
    return v;
  }
}

const state = {
  blobs: null,
  stamps: null,
  handle: null,
  root: null,
  size: 0,
  pending: [],
  log: console.log,
  onChange: () => {},
  chain: Promise.resolve(),
  metaTimer: 0,
};

/** Serialise everything that touches the pack file (two writables on one file would clobber). */
function exclusive(fn) {
  const run = state.chain.then(fn, fn);
  state.chain = run.catch(() => {});
  return run;
}

function encodeRecords(batch) {
  const encoder = new TextEncoder();
  const parts = [];
  let total = 0;
  for (const [key, bytes] of batch) {
    const k = encoder.encode(key);
    const head = new Uint8Array(8 + k.length);
    const dv = new DataView(head.buffer);
    dv.setUint32(0, k.length, true);
    head.set(k, 4);
    dv.setUint32(4 + k.length, bytes.length, true);
    parts.push(head, bytes);
    total += head.length + bytes.length;
  }
  return { parts, total };
}

/** Parse a pack. Returns [{key, start, end, dataStart}] with later records winning, plus the valid length. */
function parsePack(buf) {
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const text = new TextDecoder();
  const live = new Map(); // key -> record (insertion order = first occurrence; fine, we sort later)
  let at = 0;
  while (at + 4 <= buf.length) {
    const keyLen = view.getUint32(at, true);
    if (at + 4 + keyLen + 4 > buf.length) break;
    const dataLen = view.getUint32(at + 4 + keyLen, true);
    const end = at + 8 + keyLen + dataLen;
    if (end > buf.length) break;
    const key = text.decode(buf.subarray(at + 4, at + 4 + keyLen));
    live.delete(key); // re-insert so Map order follows last write
    live.set(key, { key, start: at, end, dataStart: at + 8 + keyLen });
    at = end;
  }
  return { live, valid: at };
}

async function readMeta(root) {
  try {
    const h = await root.getFileHandle(META);
    const json = JSON.parse(await (await h.getFile()).text());
    return new Map(Object.entries(json).filter(([, v]) => typeof v === "number"));
  } catch (_) {
    return new Map();
  }
}

async function writeMeta() {
  if (!state.root) return;
  state.blobs.metaDirty = false;
  const obj = {};
  for (const [k, v] of state.stamps) obj[k] = v; // includes released (still packed) keys
  try {
    const h = await state.root.getFileHandle(META, { create: true });
    const w = await h.createWritable();
    await w.write(JSON.stringify(obj));
    await w.close();
  } catch (error) {
    state.log(`[blobs] meta write failed: ${error}`);
  }
}

function scheduleMeta() {
  if (state.metaTimer) return;
  state.metaTimer = setTimeout(async () => {
    state.metaTimer = 0;
    if (state.blobs.metaDirty) await writeMeta();
  }, 5000);
}

/** Rewrite the pack under TARGET, dropping the least recently used entries (mips first). */
async function compact(handle, root, stamps, log) {
  const file = await handle.getFile();
  const buf = new Uint8Array(await file.arrayBuffer());
  const { live } = parsePack(buf);
  const recs = [...live.values()];
  let total = 0;
  for (const r of recs) total += r.end - r.start;
  const order = new Map(recs.map((r, i) => [r.key, i]));
  const candidates = recs
    .filter((r) => !isKept(r.key))
    .sort((a, b) => {
      const am = a.key.startsWith("mips/") ? 0 : 1;
      const bm = b.key.startsWith("mips/") ? 0 : 1;
      if (am !== bm) return am - bm;
      const at = stamps.get(a.key) || 0;
      const bt = stamps.get(b.key) || 0;
      return at - bt || order.get(a.key) - order.get(b.key);
    });
  const drop = new Set();
  for (const r of candidates) {
    if (total <= TARGET) break;
    total -= r.end - r.start;
    drop.add(r.key);
  }
  // Keep file order (oldest write first) so "later wins" semantics and mtime-ish ordering hold.
  const keep = recs.filter((r) => !drop.has(r.key));
  const w = await handle.createWritable();
  await w.truncate(0);
  // Settings first: if the tab dies mid-rewrite the most valuable records are already on disk.
  for (const r of keep.filter((r) => isKept(r.key))) await w.write(buf.subarray(r.start, r.end));
  for (const r of keep.filter((r) => !isKept(r.key))) await w.write(buf.subarray(r.start, r.end));
  await w.close();
  for (const k of drop) stamps.delete(k);
  log(`[blobs] compacted pack: dropped ${drop.size} oldest entries, ${(file.size / 1048576).toFixed(0)} -> ${(total / 1048576).toFixed(0)} MiB`);
}

export async function openBlobStore(log = console.log, onChange = () => {}) {
  state.log = log;
  state.onChange = onChange;
  state.stamps = new Map();
  const blobs = new TrackedMap(state.stamps);
  state.blobs = blobs;
  globalThis.__iw4_blobs = blobs;
  try {
    const root = await navigator.storage.getDirectory();
    state.root = root;
    const handle = await root.getFileHandle(PACK, { create: true });
    state.handle = handle;
    for (const [k, v] of await readMeta(root)) state.stamps.set(k, v);
    let file = await handle.getFile();
    if (file.size > LIMIT) {
      try {
        await compact(handle, root, state.stamps, log);
        blobs.metaDirty = true;
      } catch (error) {
        // Never leave an unloadable giant pack behind: fall back to the old policy.
        log(`[blobs] compaction failed (${error}), clearing pack`);
        const w = await handle.createWritable();
        await w.truncate(0);
        await w.close();
      }
      file = await handle.getFile();
    }
    const buf = new Uint8Array(await file.arrayBuffer());
    const { live, valid } = parsePack(buf);
    for (const r of live.values()) blobs.set(r.key, buf.slice(r.dataStart, r.end));
    // Entries that were loaded straight from the pack are not "used" yet.
    blobs.seen.clear();
    state.size = valid;
    if (valid < buf.length) {
      // A torn tail from an interrupted write: drop it so later appends stay aligned.
      const w = await handle.createWritable({ keepExistingData: true });
      await w.truncate(valid);
      await w.close();
    }
    log(`[blobs] loaded ${blobs.size} entries, ${(valid / 1048576).toFixed(1)} MiB`);
    if (blobs.metaDirty) await writeMeta();
  } catch (error) {
    log(`[blobs] persistent store unavailable: ${error}`);
    state.handle = null;
  }

  let flushing = null;
  let putCount = 0;
  const flush = () =>
    exclusive(async () => {
      if (!state.handle || state.pending.length === 0) return;
      const batch = state.pending.splice(0);
      const { parts, total } = encodeRecords(batch);
      try {
        const w = await state.handle.createWritable({ keepExistingData: true });
        await w.seek(state.size);
        for (const part of parts) await w.write(part);
        await w.close();
        state.size += total;
      } catch (error) {
        log(`[blobs] write failed: ${error}`);
        state.handle = null;
      }
      state.onChange();
    });
  const schedule = () => {
    if (flushing) return;
    flushing = setTimeout(async () => {
      await flush();
      flushing = null;
      if (state.pending.length) schedule();
    }, 1000);
  };
  globalThis.__iw4_blob_put = (key, bytes) => {
    blobs.set(key, bytes);
    blobs.seen.add(key);
    state.stamps.set(key, Date.now());
    blobs.metaDirty = true;
    putCount++;
    if (putCount % 500 === 1) log(`[blobs] put #${putCount} ${key} (${bytes.length} B)`);
    if (state.handle) {
      state.pending.push([key, bytes]);
      schedule();
    }
    scheduleMeta();
  };
  addEventListener("pagehide", () => void flush());
  scheduleMeta();
  return blobs;
}

/**
 * Drop load-only entries (decoded images, ~330 MB per map) from the in-memory Map only; the pack
 * file keeps them, so the next page load still finds them. Returns bytes freed. A later
 * `__iw4_blob_put` for such a key simply re-adds it (and appends a fresh record to the pack).
 */
export function releaseLoadOnlyEntries(prefixes = LOAD_ONLY_PREFIXES) {
  const blobs = state.blobs;
  if (!blobs) return 0;
  let freed = 0;
  let n = 0;
  for (const key of [...blobs.keys()]) {
    if (!prefixes.some((p) => key.startsWith(p))) continue;
    const v = Map.prototype.get.call(blobs, key);
    freed += (v && v.byteLength) || 0;
    blobs.delete(key);
    n++;
  }
  state.log(`[blobs] released ${(freed / 1048576).toFixed(0)} MiB after load (${n} entries)`);
  if (blobs.metaDirty) scheduleMeta();
  return freed;
}

export function cacheInfo() {
  return { entries: state.blobs ? state.blobs.size : 0, bytes: state.size, limit: LIMIT };
}

/** Wipe the pack (except `fs:`/`meta:` user settings) and the in-memory Map. */
export function clearCache() {
  return exclusive(async () => {
    const blobs = state.blobs;
    if (!blobs) return;
    const keep = [...blobs.entries()].filter(([k]) => isKept(k));
    blobs.clear();
    blobs.seen.clear();
    state.pending = [];
    for (const k of [...state.stamps.keys()]) if (!isKept(k)) state.stamps.delete(k);
    if (state.handle) {
      try {
        const w = await state.handle.createWritable();
        await w.truncate(0);
        if (keep.length) {
          const { parts } = encodeRecords(keep);
          for (const p of parts) await w.write(p);
        }
        await w.close();
        state.size = (await state.handle.getFile()).size;
      } catch (error) {
        state.log(`[blobs] clear failed: ${error}`);
      }
    }
    for (const [k, v] of keep) blobs.set(k, v);
    blobs.metaDirty = true;
    await writeMeta();
    state.log("[blobs] cache cleared");
    state.onChange();
  });
}
