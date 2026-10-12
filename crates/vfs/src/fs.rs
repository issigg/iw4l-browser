//! Wasm implementation of the subset of `std::fs` the game uses.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::XmlHttpRequest;

const REMOTE_ROOT: &str = "/games";
const BLOCK: u64 = 64 << 10;
const MAX_FETCH: u64 = 32 << 20;

#[wasm_bindgen(inline_js = r#"
export function iw4_xhr_range(url, start, end) {
  const cache = globalThis.__iw4_cache;
  if (cache) { const hit = cache.read(url, start, end); if (hit) return hit; }
  const g = globalThis.iw4Games;
  if (g) return g.read(decodeURIComponent(url), start, end);
  const x = new XMLHttpRequest();
  x.open('GET', url, false);
  // Workers may ask for raw bytes from a synchronous XHR: exact and fast. The text route below
  // (page main thread only) is NOT byte-exact: a block starting with FF FE / FE FF / EF BB BF is
  // taken for a byte-order mark and decoded as UTF-16 / stripped.
  const raw = typeof window === 'undefined';
  if (raw) x.responseType = 'arraybuffer';
  else x.overrideMimeType('text/plain; charset=x-user-defined');
  x.setRequestHeader('Range', 'bytes=' + start + '-' + end);
  x.send();
  if (x.status < 200 || x.status >= 300) return x.status;
  if (raw) return new Uint8Array(x.response);
  const s = x.responseText;
  const sent = Number(x.getResponseHeader('content-length'));
  if (sent && s.length !== sent) return 599; // decoded length differs: refuse instead of corrupting
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 255;
  return out;
}
"#)]
extern "C" {
    fn iw4_xhr_range(url: &str, start: f64, end: f64) -> JsValue;
}

#[wasm_bindgen(inline_js = r#"
// Folder mode (globalThis.iw4Games, see gamefiles.js): same answers as /__stat and /__ls.
// `path` is the URL-encoded path (fs.rs `enc`). Returns undefined when not in folder mode.
export function iw4_games_json(kind, path) {
  const g = globalThis.iw4Games;
  if (!g) return undefined;
  const p = decodeURIComponent(path);
  if (kind === 'ls') return JSON.stringify(g.ls(p) || { exists: false });
  return JSON.stringify(g.stat(p));
}
"#)]
extern "C" {
    fn iw4_games_json(kind: &str, path: &str) -> Option<String>;
}

#[wasm_bindgen(inline_js = r#"
const PFX = 'fs:';
export function iw4_ls_keys() {
  const out = [];
  const m = globalThis.__iw4_blobs;
  if (m && m.forEach) m.forEach((v, k) => { if (typeof k === 'string' && k.startsWith(PFX) && v && v.length > 0) out.push(k.slice(PFX.length)); });
  return out;
}
export function iw4_ls_read(path) {
  const m = globalThis.__iw4_blobs;
  const v = m && m.get(PFX + path);
  if (!v || v.length === 0) return undefined;
  return v;
}
export function iw4_ls_write(path, bytes) {
  const put = globalThis.__iw4_blob_put;
  if (typeof put !== 'function') return false;
  try { put(PFX + path, bytes.slice()); return true; }
  catch (e) { console.warn('persist failed', path, String(e)); return false; }
}
export function iw4_ls_remove(path) {
  const put = globalThis.__iw4_blob_put;
  if (typeof put !== 'function') return;
  try { put(PFX + path, new Uint8Array(0)); } catch (e) {}
}
"#)]
extern "C" {
    fn iw4_ls_keys() -> js_sys::Array;
    fn iw4_ls_read(path: &str) -> JsValue;
    fn iw4_ls_write(path: &str, bytes: &[u8]) -> bool;
    fn iw4_ls_remove(path: &str);
}

/// Files under the artifacts root (profile, classes, settings, account) survive page reloads in
/// the blob store; caches and temp files do not.
fn persistable(path: &Path) -> bool {
    path.starts_with("/artifacts")
        && !path.starts_with("/artifacts/cache")
        && path.extension().is_none_or(|e| e != "tmp" && !e.to_string_lossy().ends_with(".tmp"))
}

fn persist(path: &Path) {
    if !persistable(path) {
        return;
    }
    let key = path.to_string_lossy().into_owned();
    let bytes = STATE.with(|s| s.borrow().files.get(path).cloned());
    match bytes {
        Some(b) if b.len() <= 512 * 1024 => {
            iw4_ls_write(&key, &b);
        }
        Some(_) => {}
        None => iw4_ls_remove(&key),
    }
}

fn load_persisted() {
    for key in iw4_ls_keys().iter() {
        let Some(path) = key.as_string() else { continue };
        let value = iw4_ls_read(&path);
        if value.is_undefined() {
            continue;
        }
        let arr: js_sys::Uint8Array = value.unchecked_into();
        let mut bytes = vec![0u8; arr.length() as usize];
        arr.copy_to(&mut bytes);
        let path = PathBuf::from(path);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let mut p = path.as_path();
            while let Some(parent) = p.parent() {
                if parent == Path::new("/") {
                    break;
                }
                s.dirs.insert(parent.to_owned(), ());
                p = parent;
            }
            s.files.insert(path.clone(), bytes);
        });
    }
}

#[wasm_bindgen(inline_js = "export function iw4_vfs_log(text) { console.warn(text); }")]
extern "C" {
    fn iw4_vfs_log(text: &str);
}

fn web_log(text: &str) {
    iw4_vfs_log(text);
}

fn unsupported(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, msg.to_owned())
}

fn norm(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

fn is_remote(path: &Path) -> bool {
    path.starts_with(REMOTE_ROOT)
}

#[derive(Default)]
struct State {
    files: HashMap<PathBuf, Vec<u8>>,
    dirs: BTreeMap<PathBuf, ()>,
    stat: HashMap<PathBuf, Option<(bool, u64)>>,
    ls: HashMap<PathBuf, Option<Vec<(String, bool, u64)>>>,
    block: HashMap<(PathBuf, u64), std::rc::Rc<Vec<u8>>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn ensure_loaded() {
    if !LOADED.with(|l| l.replace(true)) {
        load_persisted();
    }
}

fn xhr_get(url: &str, range: Option<(u64, u64)>, binary: bool) -> io::Result<String> {
    let xhr = XmlHttpRequest::new().map_err(|_| unsupported("xhr"))?;
    xhr.open_with_async("GET", url, false)
        .map_err(|_| unsupported("xhr open"))?;
    if binary {
        xhr.override_mime_type("text/plain; charset=x-user-defined")
            .map_err(|_| unsupported("xhr mime"))?;
    }
    if let Some((a, b)) = range {
        xhr.set_request_header("Range", &format!("bytes={a}-{b}"))
            .map_err(|_| unsupported("xhr range"))?;
    }
    xhr.send().map_err(|e| io::Error::other(format!("xhr send: {e:?}")))?;
    let status = xhr.status().unwrap_or(0);
    if status == 404 {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    if !(200..300).contains(&status) {
        return Err(io::Error::other(format!("http {status} for {url}")));
    }
    let text: JsValue = xhr.response_text().ok().flatten().map(JsValue::from).unwrap_or(JsValue::NULL);
    Ok(text.as_string().unwrap_or_default())
}

fn enc(path: &Path) -> String {
    path.to_string_lossy().replace('%', "%25").replace('#', "%23").replace('?', "%3F").replace(' ', "%20")
}

fn remote_stat(path: &Path) -> io::Result<(bool, u64)> {
    if let Some(hit) = STATE.with(|s| s.borrow().stat.get(path).cloned()) {
        return hit.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound));
    }
    let res = match iw4_games_json("stat", &enc(path)) {
        Some(text) => Ok(text),
        None => xhr_get(&format!("/__stat?p={}", enc(path)), None, false),
    };
    let parsed = match res {
        Ok(text) => {
            let v: serde_json::Value = serde_json::from_str(&text).map_err(io::Error::other)?;
            if v["exists"].as_bool() == Some(true) {
                Some((v["dir"].as_bool().unwrap_or(false), v["size"].as_u64().unwrap_or(0)))
            } else {
                None
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    STATE.with(|s| s.borrow_mut().stat.insert(path.to_owned(), parsed));
    parsed.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

/// Fetch `dst.len()` bytes at `start` straight into `dst` (no string/UTF-8 round trip).
fn remote_range_into(path: &Path, start: u64, dst: &mut [u8]) -> io::Result<usize> {
    if dst.is_empty() {
        return Ok(0);
    }
    let end = start + dst.len() as u64 - 1;
    let res = iw4_xhr_range(&enc(path), start as f64, end as f64);
    if let Some(code) = res.as_f64() {
        return Err(if code as u32 == 404 {
            io::Error::from(io::ErrorKind::NotFound)
        } else {
            io::Error::other(format!("http {code} for {}", path.display()))
        });
    }
    let arr: js_sys::Uint8Array = res.dyn_into().map_err(|_| io::Error::other("xhr: bad response"))?;
    let n = (arr.length() as usize).min(dst.len());
    arr.subarray(0, n as u32).copy_to(&mut dst[..n]);
    Ok(n)
}

fn remote_range(path: &Path, start: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; len as usize];
    let n = remote_range_into(path, start, &mut v)?;
    v.truncate(n);
    Ok(v)
}

fn block(path: &Path, index: u64, size: u64) -> io::Result<std::rc::Rc<Vec<u8>>> {
    let key = (path.to_owned(), index);
    if let Some(b) = STATE.with(|s| s.borrow().block.get(&key).cloned()) {
        return Ok(b);
    }
    let start = index * BLOCK;
    let len = BLOCK.min(size - start);
    let data = std::rc::Rc::new(remote_range(path, start, len)?);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.block.len() > 2048 {
            s.block.clear();
        }
        s.block.insert(key, data.clone());
    });
    Ok(data)
}

// ---- types ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    dir: bool,
}
impl FileType {
    pub fn is_dir(&self) -> bool {
        self.dir
    }
    pub fn is_file(&self) -> bool {
        !self.dir
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    dir: bool,
    len: u64,
}
impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.dir
    }
    pub fn is_file(&self) -> bool {
        !self.dir
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn file_type(&self) -> FileType {
        FileType { dir: self.dir }
    }
    pub fn modified(&self) -> io::Result<web_time::SystemTime> {
        Err(unsupported("modified"))
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
    pub fn created(&self) -> io::Result<web_time::SystemTime> {
        Err(unsupported("created"))
    }
}

pub struct DirEntry {
    path: PathBuf,
    dir: bool,
    len: u64,
}
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_owned()
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(Metadata { dir: self.dir, len: self.len })
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType { dir: self.dir })
    }
}
pub struct ReadDir(std::vec::IntoIter<DirEntry>);
impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(Ok)
    }
}

#[derive(Debug)]
enum Backing {
    Remote { size: u64 },
    Mem,
}
#[derive(Debug)]
pub struct File {
    path: PathBuf,
    pos: u64,
    backing: Backing,
    writable: bool,
    append: bool,
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}
impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read(&mut self, v: bool) -> &mut Self {
        self.read = v;
        self
    }
    pub fn write(&mut self, v: bool) -> &mut Self {
        self.write = v;
        self
    }
    pub fn append(&mut self, v: bool) -> &mut Self {
        self.append = v;
        self
    }
    pub fn truncate(&mut self, v: bool) -> &mut Self {
        self.truncate = v;
        self
    }
    pub fn create(&mut self, v: bool) -> &mut Self {
        self.create = v;
        self
    }
    pub fn create_new(&mut self, v: bool) -> &mut Self {
        self.create_new = v;
        self
    }
    pub fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        ensure_loaded();
        let path = norm(path.as_ref());
        let writing = self.write || self.append;
        if is_remote(&path) {
            if writing {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            let (dir, size) = remote_stat(&path)?;
            if dir {
                return Err(io::Error::other("is a directory"));
            }
            return Ok(File { path, pos: 0, backing: Backing::Remote { size }, writable: false, append: false });
        }
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let exists = s.files.contains_key(&path);
            if self.create_new && exists {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            if !exists && !(self.create || self.create_new) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let entry = s.files.entry(path.clone()).or_default();
            if self.truncate {
                entry.clear();
            }
            Ok(())
        })?;
        Ok(File { path, pos: 0, backing: Backing::Mem, writable: writing, append: self.append })
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if self.writable && matches!(self.backing, Backing::Mem) {
            persist(&self.path);
        }
    }
}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }
    pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path)
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        let len = match self.backing {
            Backing::Remote { size } => size,
            Backing::Mem => STATE.with(|s| s.borrow().files.get(&self.path).map_or(0, |f| f.len() as u64)),
        };
        Ok(Metadata { dir: false, len })
    }
    pub fn sync_all(&self) -> io::Result<()> {
        Ok(())
    }
    /// Single-threaded: advisory locks are no-ops.
    pub fn lock(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn unlock(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn set_len(&self, len: u64) -> io::Result<()> {
        STATE.with(|s| {
            if let Some(f) = s.borrow_mut().files.get_mut(&self.path) {
                f.resize(len as usize, 0);
            }
        });
        Ok(())
    }
    pub fn try_clone(&self) -> io::Result<File> {
        Ok(File {
            path: self.path.clone(),
            pos: self.pos,
            backing: match self.backing {
                Backing::Remote { size } => Backing::Remote { size },
                Backing::Mem => Backing::Mem,
            },
            writable: self.writable,
            append: self.append,
        })
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.backing {
            Backing::Mem => STATE.with(|s| {
                let s = s.borrow();
                let data = s.files.get(&self.path).map(Vec::as_slice).unwrap_or(&[]);
                let pos = (self.pos as usize).min(data.len());
                let n = buf.len().min(data.len() - pos);
                buf[..n].copy_from_slice(&data[pos..pos + n]);
                self.pos += n as u64;
                Ok(n)
            }),
            Backing::Remote { size } => {
                if self.pos >= size || buf.is_empty() {
                    return Ok(0);
                }
                let want = (buf.len() as u64).min(size - self.pos);
                if want >= BLOCK {
                    let n = want.min(MAX_FETCH) as usize;
                    let got = remote_range_into(&self.path, self.pos, &mut buf[..n])?;
                    self.pos += got as u64;
                    return Ok(got);
                }
                let index = self.pos / BLOCK;
                let b = block(&self.path, index, size)?;
                let off = (self.pos - index * BLOCK) as usize;
                if off >= b.len() {
                    // The server returned fewer bytes than the file size promised for this block.
                    // Report it (once per file) and treat it as end of data instead of panicking.
                    web_log(&format!(
                        "vfs: short block in {} at {}: block {} has {} bytes but offset {} within it was wanted (size {})",
                        self.path.display(), self.pos, index, b.len(), off, size
                    ));
                    return Ok(0);
                }
                let n = (want as usize).min(b.len() - off);
                buf[..n].copy_from_slice(&b[off..off + n]);
                self.pos += n as u64;
                Ok(n)
            }
        }
    }
}

impl Seek for File {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let len = self.metadata()?.len as i64;
        let next = match from {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(d) => len + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if next < 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.writable {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let data = s.files.entry(self.path.clone()).or_default();
            if self.append {
                self.pos = data.len() as u64;
            }
            let pos = self.pos as usize;
            if data.len() < pos + buf.len() {
                data.resize(pos + buf.len(), 0);
            }
            data[pos..pos + buf.len()].copy_from_slice(buf);
            self.pos += buf.len() as u64;
            Ok(buf.len())
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Read for &File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut c = self.try_clone()?;
        c.read(buf)
    }
}

// ---- free functions ----

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    ensure_loaded();
    let path = norm(path.as_ref());
    if is_remote(&path) {
        let (dir, len) = remote_stat(&path)?;
        return Ok(Metadata { dir, len });
    }
    STATE.with(|s| {
        let s = s.borrow();
        if let Some(f) = s.files.get(&path) {
            Ok(Metadata { dir: false, len: f.len() as u64 })
        } else if path == Path::new("/") || s.dirs.contains_key(&path) {
            Ok(Metadata { dir: true, len: 0 })
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    })
}
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    metadata(path)
}
pub fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    Ok(metadata(path).is_ok())
}
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let p = norm(path.as_ref());
    metadata(&p)?;
    Ok(p)
}
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len;
    let mut out = Vec::with_capacity(len as usize);
    f.read_to_end(&mut out)?;
    Ok(out)
}
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    File::create(path)?.write_all(contents.as_ref())
}
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    ensure_loaded();
    let path = norm(path.as_ref());
    let mut entries = Vec::new();
    if is_remote(&path) {
        let cached = STATE.with(|s| s.borrow().ls.get(&path).cloned());
        let rows = match cached {
            Some(Some(r)) => r,
            Some(None) => return Err(io::Error::from(io::ErrorKind::NotFound)),
            None => {
                let text = match iw4_games_json("ls", &enc(&path)).map(Ok).unwrap_or_else(|| xhr_get(&format!("/__ls?p={}", enc(&path)), None, false)) {
                    Ok(t) => t,
                    Err(e) => {
                        if e.kind() == io::ErrorKind::NotFound {
                            STATE.with(|s| s.borrow_mut().ls.insert(path.clone(), None));
                        }
                        return Err(e);
                    }
                };
                let v: serde_json::Value = serde_json::from_str(&text).map_err(io::Error::other)?;
                let Some(arr) = v.as_array() else {
                    STATE.with(|s| s.borrow_mut().ls.insert(path.clone(), None));
                    return Err(io::Error::from(io::ErrorKind::NotFound));
                };
                let rows: Vec<(String, bool, u64)> = arr
                    .iter()
                    .map(|e| {
                        (
                            e["name"].as_str().unwrap_or_default().to_owned(),
                            e["dir"].as_bool().unwrap_or(false),
                            e["size"].as_u64().unwrap_or(0),
                        )
                    })
                    .collect();
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    for (n, d, l) in &rows {
                        s.stat.insert(path.join(n), Some((*d, *l)));
                    }
                    s.ls.insert(path.clone(), Some(rows.clone()));
                });
                rows
            }
        };
        for (n, d, l) in rows {
            entries.push(DirEntry { path: path.join(n), dir: d, len: l });
        }
    } else {
        STATE.with(|s| {
            let s = s.borrow();
            for (p, f) in &s.files {
                if p.parent() == Some(path.as_path()) {
                    entries.push(DirEntry { path: p.clone(), dir: false, len: f.len() as u64 });
                }
            }
            for p in s.dirs.keys() {
                if p.parent() == Some(path.as_path()) {
                    entries.push(DirEntry { path: p.clone(), dir: true, len: 0 });
                }
            }
        });
    }
    Ok(ReadDir(entries.into_iter()))
}
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = norm(path.as_ref());
    if is_remote(&path) {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut p = path.as_path();
        while p != Path::new("/") {
            s.dirs.insert(p.to_owned(), ());
            match p.parent() {
                Some(q) => p = q,
                None => break,
            }
        }
    });
    Ok(())
}
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    create_dir_all(path)
}
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = norm(path.as_ref());
    let removed = STATE.with(|s| s.borrow_mut().files.remove(&path)).map(|_| ()).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound));
    persist(&path);
    removed
}
pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = norm(path.as_ref());
    STATE.with(|s| s.borrow_mut().dirs.remove(&path));
    Ok(())
}
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = norm(path.as_ref());
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.files.retain(|p, _| !p.starts_with(&path));
        s.dirs.retain(|p, _| !p.starts_with(&path));
    });
    Ok(())
}
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (norm(from.as_ref()), norm(to.as_ref()));
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let data = s.files.remove(&from).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        s.files.insert(to.clone(), data);
        Ok(to)
    })
    .map(|to: PathBuf| {
        persist(&to);
    })
}
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let data = read(from)?;
    let n = data.len() as u64;
    write(to, data)?;
    Ok(n)
}
pub fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    copy(from, to).map(|_| ())
}
