//! WASM-SPIKE virtual filesystem.
//!
//! Native: `vfs::fs` is `std::fs`. Wasm: paths under `/games` are served read-only by the dev
//! server over synchronous XHR with Range requests (works on the main thread, is deprecated but
//! functional); everything else lives in an in-memory overlay.

use std::path::Path;

#[cfg(not(target_family = "wasm"))]
pub use std::fs;

#[cfg(target_family = "wasm")]
pub mod fs;

#[cfg(target_family = "wasm")]
pub mod blobcache;

/// Method names that cannot collide with `Path::{is_dir,is_file,exists}`, which call `std::fs`
/// directly and so fail on wasm. Also implemented for metadata/file-type values.
pub trait VfsExt {
    fn vdir(&self) -> bool;
    fn vfile(&self) -> bool;
    fn vexists(&self) -> bool;
}

#[cfg(not(target_family = "wasm"))]
impl VfsExt for Path {
    fn vdir(&self) -> bool {
        self.is_dir()
    }
    fn vfile(&self) -> bool {
        self.is_file()
    }
    fn vexists(&self) -> bool {
        self.exists()
    }
}
#[cfg(not(target_family = "wasm"))]
impl VfsExt for std::path::PathBuf {
    fn vdir(&self) -> bool {
        self.as_path().vdir()
    }
    fn vfile(&self) -> bool {
        self.as_path().vfile()
    }
    fn vexists(&self) -> bool {
        self.as_path().vexists()
    }
}
#[cfg(not(target_family = "wasm"))]
impl VfsExt for std::fs::Metadata {
    fn vdir(&self) -> bool {
        self.is_dir()
    }
    fn vfile(&self) -> bool {
        self.is_file()
    }
    fn vexists(&self) -> bool {
        true
    }
}
#[cfg(not(target_family = "wasm"))]
impl VfsExt for std::fs::FileType {
    fn vdir(&self) -> bool {
        self.is_dir()
    }
    fn vfile(&self) -> bool {
        self.is_file()
    }
    fn vexists(&self) -> bool {
        true
    }
}

#[cfg(target_family = "wasm")]
impl VfsExt for Path {
    fn vdir(&self) -> bool {
        fs::metadata(self).is_ok_and(|m| m.is_dir())
    }
    fn vfile(&self) -> bool {
        fs::metadata(self).is_ok_and(|m| m.is_file())
    }
    fn vexists(&self) -> bool {
        fs::metadata(self).is_ok()
    }
}
#[cfg(target_family = "wasm")]
impl VfsExt for std::path::PathBuf {
    fn vdir(&self) -> bool {
        self.as_path().vdir()
    }
    fn vfile(&self) -> bool {
        self.as_path().vfile()
    }
    fn vexists(&self) -> bool {
        self.as_path().vexists()
    }
}
#[cfg(target_family = "wasm")]
impl VfsExt for fs::Metadata {
    fn vdir(&self) -> bool {
        self.is_dir()
    }
    fn vfile(&self) -> bool {
        self.is_file()
    }
    fn vexists(&self) -> bool {
        true
    }
}
#[cfg(target_family = "wasm")]
impl VfsExt for fs::FileType {
    fn vdir(&self) -> bool {
        self.is_dir()
    }
    fn vfile(&self) -> bool {
        self.is_file()
    }
    fn vexists(&self) -> bool {
        true
    }
}

/// `vfs::process_id()` panics on wasm.
pub fn process_id() -> u32 {
    #[cfg(target_family = "wasm")]
    {
        1
    }
    #[cfg(not(target_family = "wasm"))]
    std::process::id()
}
