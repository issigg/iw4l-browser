//! Wasm side of the persistent derived-artifact store (translated shaders, bot navigation,
//! localization). The page (`www/blobstore.js`) loads the previous session's entries into
//! `globalThis.__iw4_blobs` before the engine starts and persists new ones to the origin-private
//! file system, so a second load skips that work. Entries are the engine's own output, never game
//! data. Without the page's store both calls are no-ops.
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen(inline_js = r#"
export function iw4_blob_get(key) { const m = globalThis.__iw4_blobs; return m ? m.get(key) : undefined; }
export function iw4_blob_put(key, bytes) { const p = globalThis.__iw4_blob_put; if (p) p(key, bytes.slice()); }
"#)]
extern "C" {
    fn iw4_blob_get(key: &str) -> JsValue;
    fn iw4_blob_put(key: &str, bytes: &[u8]);
}

pub fn get(key: &str) -> Option<Vec<u8>> {
    let value = iw4_blob_get(key);
    if value.is_undefined() {
        return None;
    }
    let bytes: js_sys::Uint8Array = value.dyn_into().ok()?;
    let mut out = vec![0u8; bytes.length() as usize];
    bytes.copy_to(&mut out);
    Some(out)
}

pub fn put(key: &str, bytes: &[u8]) {
    iw4_blob_put(key, bytes);
}
