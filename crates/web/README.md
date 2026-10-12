# IW4L in the browser

An experimental port of IW4L to WebAssembly and WebGPU. It runs offline, against bots,
in Chrome, using Modern Warfare 2 (2009) files **you already own**. No game assets are
included or uploaded anywhere: files are read locally, either from the dev server on your
machine or from a folder you pick in the browser.

Status: experimental. Menus, class selection, maps, bots, HUD, shadows, audio and gamepad
work. Networking and the updater are stubbed out, so there is no online play.

## Requirements

- A desktop Chromium browser with WebGPU (Chrome on macOS has been tested).
- Your own MW2 Multiplayer install (Steam PC version).
- About 4 GB of addressable wasm memory per tab. Map loads peak at roughly 3.0-3.4 GiB
  of the 4 GiB wasm32 limit, so close other heavy tabs.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
scripts/build-web.sh
```

## Run

```bash
node crates/web/www/serve.mjs "/path/to/your/MW2 folder" 8137
```

Open <http://localhost:8137/index.html>. Keep the tab in the foreground; Chrome slows
background tabs a lot, which skews load times.

To skip the menu: `index.html?map=mp_rust`.

### Playing without the dev server

Serve `crates/web/www` from any static server (it must be `localhost` or HTTPS, and
serve `.wasm` as `application/wasm`), then choose **Choose your Modern Warfare 2 folder**.
The folder handle is remembered, so the next visit offers to reuse it. This path uses the
File System Access API, so it is Chromium only. Verified next to the dev server with
`?folder=1`; a run with no dev server at all has not been tested yet.

### URL options

| Option | Effect |
|---|---|
| `?map=mp_rust` | Start a map directly, with bots |
| `?folder=1` | Force the folder picker even when the dev server is present |
| `?server=1` | Force dev-server mode |
| `?scale=0.75` | Render scale (also the Quality button) |

In the page: **F2** shows diagnostics, **Alt+Enter** toggles fullscreen, and click the
canvas to capture the mouse.

## How it works

- The engine runs in a dedicated module worker and renders to an `OffscreenCanvas`.
  `crates/web_window` replaces `bevy_winit`. The page forwards input and owns the
  `AudioContext`; the protocol is in [`www/PROTOCOL.md`](www/PROTOCOL.md).
- Game files are read with synchronous range reads (`crates/vfs`), with a prefetch cache
  so a map does not wait on many small requests.
- Derived data (shader, nav, localization and mip caches) and saved settings live in an
  append-only blob store in the browser's origin-private file system (OPFS). Use the cache
  controls in the page to clear them.
- WebGPU has no binding arrays, so the wasm build binds textures per slot per draw.
- To fit the 4 GiB limit, textures are capped at 512 and some per-vertex data is derived
  instead of stored.

## Known gaps

- No networking or updater.
- Only Chromium; folder mode needs the File System Access API.
- Tight on memory for the biggest maps (afghan, nightshift, invasion, terminal).
- Texture quality is capped for memory.
- Not every map has been played through, only loaded.

An experiment with wasm threads (nightly Rust, shared memory) worked but was not faster,
so it is not part of this build. It is kept on the `web-threads` branch.

## Credits

- **[IW4L](https://github.com/vladtrc/iw4L)** by vladtrc is the engine this is a port of.
  All the engine, asset readers and gameplay code are theirs; this crate is the browser
  layer on top.
- A closed-source browser build of MW2 found online (no author or licence stated) showed
  that this was feasible and inspired the design: engine in a worker, range-based
  prefetch, and OPFS caches. No code or assets from it are used here.
- [Bevy](https://bevy.org/), [wgpu](https://wgpu.rs/) and
  [wasm-bindgen](https://github.com/rustwasm/wasm-bindgen) make the wasm build possible.

Call of Duty and Modern Warfare 2 belong to their owners. This project is unaffiliated
with them and distributes no game assets.
