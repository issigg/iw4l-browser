# Worker mode: page <-> engine worker protocol

The engine runs in a dedicated module worker (`worker.js`) on an `OffscreenCanvas`; the page
(`index.html`) owns the DOM, input capture, pointer lock, audio output and the loading UI.
The same wasm still supports the old main-thread path (`run(map)`), so any JS that Rust inlines
must work in both: branch on `typeof window === "undefined"` (true in the worker).

## Boot (page -> worker, first message, `postMessage(msg, [canvas])`)

    { type: "boot", canvas: OffscreenCanvas, map: string|null,
      width: number, height: number,   // physical pixels of the canvas
      scale: number,                   // devicePixelRatio
      games: null | { mode: 'folder', name: string, files: [[path: string, File], ...] } }
        // null: the dev server (serve.mjs) provides the game files. Folder mode (no server): the page
        // got the user's MW2 install folder through the File System Access API (folder-picker.js,
        // `getGamesSource()`), `path` is relative with forward slashes (`zone/english/mp_rust.ff`),
        // `File`s are structured-cloned to the worker.

The worker then: opens the blob store (`blobstore.js`, unchanged: sets `globalThis.__iw4_blobs` /
`__iw4_blob_put`), runs the prefetch (moved out of index.html; sets `globalThis.__iw4_cache`),
imports `./pkg/web.js`, calls `init()`, then calls the wasm export
`worker_main(canvas, map, width, height, scale)`. Loading progress goes to the page:

    worker -> page: { type: "progress", text: string, pct: number }   // pct 0..100
    worker -> page: { type: "started" }   // right before worker_main
    worker -> page: { type: "failed", message: string }

## Input (page -> worker)

The worker's `onmessage` appends every message whose `type` is one of the input types below to
`globalThis.__iw4_inbox` (an Array). Rust drains it once per frame through
`globalThis.__iw4_drain()` (defined by Rust's inline JS; returns a JSON string `"[...]"` and
empties the array). Do not process input messages any other way in the worker.

    { type: "key",    code: string, key: string, down: bool, repeat: bool }
        // KeyboardEvent.code / .key; `key` of a printable char is the char itself
    { type: "mousebtn", button: number, down: bool }       // MouseEvent.button (0 left,1 mid,2 right,3 back,4 fwd)
    { type: "mousepos", x: number, y: number }             // CSS px * scale = physical px, relative to canvas
    { type: "motion", dx: number, dy: number }             // MouseEvent.movementX/Y (physical px), always sent (also when locked)
    { type: "wheel",  dx: number, dy: number, line: bool } // line=true when deltaMode is lines
    { type: "resize", width: number, height: number, scale: number }   // physical canvas size
    { type: "focus",  focused: bool }
    { type: "cursorin", inside: bool }                     // pointer entered/left the canvas

The page sets `canvas.width/height` is NOT possible after transfer: it sends `resize` and the
engine resizes the OffscreenCanvas itself.

## Cursor (worker -> page)

    { type: "cursor", locked: bool, visible: bool }
        // locked=true -> page calls canvas.requestPointerLock() (it needs a user gesture; if it
        // fails, retry on the next mousedown); locked=false -> document.exitPointerLock().
        // visible=false and not locked -> style the canvas `cursor: none`.

## Audio

Audio output lives in the page (AudioContext + the `iw4-out` worklet from the old
web_audio.rs). The worker only mixes.

    worker -> page: { type: "audio-init" }                       // once; page installs first-gesture listeners
    worker -> page: { type: "audio-push", samples: Float32Array } // interleaved stereo 48 kHz, transferred
    worker -> page: { type: "audio-tone", f: number, secs: number }  // debug sine
    page -> worker: { type: "audio-state", state: 0|1|2 }         // 0 no context, 1 pending/suspended, 2 running+worklet attached
    page -> worker: { type: "audio-progress", consumed: number, queued: number, time: number }
        // from the worklet's report (frames consumed, frames queued, ctx.currentTime), sent as
        // often as the worklet reports (every ~4 quanta)

The worker keeps `globalThis.__iw4_audio = { state, consumed, queued, reportAt }` up to date
(`reportAt` = `performance.now()` when the progress message was handled) so Rust can estimate
buffered frames synchronously.

## Telemetry (worker -> page)

    { type: "fps", fps: number, worst: number, memMB: number }    // once per second, from the engine's frame loop
    { type: "log", level: "log"|"warn"|"error", text: string }    // optional; page may ignore

## Persistence

`localStorage` does not exist in the worker. Rust persists small profile/settings files through
the blob store instead: key `"fs:" + absolutePath`, value = file bytes; a zero-length value
means deleted. Rust reads them from `globalThis.__iw4_blobs` at startup (iterate keys with the
`fs:` prefix) and writes through `globalThis.__iw4_blob_put(key, bytes)`.

## Gamepads (added)

`navigator.getGamepads()` only exists on the page, so the page polls it (once per animation frame
while the tab is visible, 250 ms timer otherwise) and posts a message when a pad connects,
disconnects, or its state changes (a button value or axis moves by more than 0.01):

    page -> worker: { type: "pad", index: number, id: string, connected: bool,
                      buttons: number[], axes: number[] }
        // `standard` mapping order (W3C): buttons[0..16] = A,B,X,Y,LB,RB,LT,RT,Back,Start,
        // L3,R3,DUp,DDown,DLeft,DRight,Home with values 0..1; axes[0..4] = LX,LY,RX,RY, -1..1
        // with Y down-positive as the browser reports it (Rust flips it).
        // Pads whose `mapping` is not "standard" are ignored by the page.

Rust turns these into Bevy raw gamepad events (connection once, then button/axis changes).

## Loading progress (added)

The worker derives progress from the engine's own log lines (its `console.error` override already
sees them) and posts `{ type: "progress", text, pct }` as before, now covering the whole load, not
only the prefetch. The page hides the boot box on the first `fps` message after `started` (the
engine only posts `fps` once its frame loop runs; the load itself happens inside the first frames).

## Cache management (added)

    page -> worker: { type: "cache-clear" }                  // wipe the OPFS blob store (all entries)
    worker -> page: { type: "cache-info", entries: number, bytes: number, limit: number }  // after open and after any change
    worker -> page: { type: "cache-cleared" }

### Implementation notes (worker/page)

- `pad` is an input type: the worker appends it to `__iw4_inbox` like the others. Before the engine
  starts only the latest `pad` per index is kept (messages are full snapshots).
- Progress: prefetch maps to 0..15 %, the engine's `load stage: <name> done` lines advance 15..99 %
  by typical stage duration (table in worker.js), `load complete:` (or `menu open: iw4l_main` when
  no map was requested) is 100 %. pct never decreases.
- After `load complete` the worker sets `globalThis.__iw4_cache = null` (clears its Map, stops the
  stats timer) and drops `mips/` entries from the in-memory `__iw4_blobs` Map (the OPFS pack keeps
  them). A later `__iw4_blob_put` for such a key re-adds it and appends a new pack record.
  Rust must treat a missing `__iw4_cache` and a missing `mips/` blob as normal misses.
- Blob pack eviction: when the pack is over 1536 MiB at open it is compacted to <= 75 %, dropping
  `mips/` entries first then others, oldest first (last-used stamps in the sidecar
  `iw4l-blobs-v1.meta.json`). `fs:` and `meta:` entries are never evicted. Pack format unchanged.
- `cache-clear` wipes everything except `fs:` / `meta:` entries (profile/settings), so settings
  survive; the page UI is the `cache: N MiB · clear` button (boot box; always with `?cache=1`).

## Folder mode (no dev server)

With `games.mode === 'folder'` the worker calls `installGames(games)` (`gamefiles.js`), which sets
`globalThis.iw4Games` (synchronous, paths are decoded `/games/iw4/<rel>` like the server's):
`stat(p) -> {exists,dir,size}`, `ls(p) -> [{name,dir,size}] | null`, `read(p, start, endInclusive) ->
Uint8Array | 404` (FileReaderSync). `/games` lists only `iw4`; lookups are exact, with a
case-insensitive fallback. `fs.rs` uses `iw4Games` instead of `/__stat`, `/__ls` and the ranged GET
(after the `__iw4_cache` check) whenever it exists; with the dev server nothing changes.

Prefetch manifests are browser-side: on a map's first load the worker wraps `iw4Games.read`, records
the ranges that missed the cache, and at `load complete` stores `manifest/<map>` (`menu` for menu-only
loads; JSON `{map,total,files:[[path,[[a,b],...]],...]}`, coalesced like serve.mjs) in the blob store.
Later loads read those ranges straight from the folder into the `RangeCache` (keyed by the encoded
path) before starting the engine. `manifest/` entries are never evicted (blobstore.js `KEEP_PREFIXES`).
The `/__log` mirror and `/__done` call are skipped in folder mode.
