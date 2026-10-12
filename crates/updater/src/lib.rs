// WASM-SPIKE: the web build has no self-update or community config; keep only the surface other crates call.
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(target_family = "wasm")]
mod web {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    pub type Result<T> = std::result::Result<T, String>;

    #[derive(Clone, Debug)]
    pub struct CommunityFile {
        pub path: PathBuf,
        pub name: String,
    }
    #[derive(Clone, Debug)]
    pub struct Community {
        pub name: String,
    }
    pub fn communities() -> &'static [CommunityFile] {
        &[]
    }
    pub fn selected_path() -> Option<&'static Path> {
        None
    }
    pub fn selected() -> Option<&'static Community> {
        None
    }
    pub fn restart_with_community(_path: &Path) -> Result<()> {
        Err("unavailable in the browser build".into())
    }
    pub fn startup() -> Result<Option<Vec<OsString>>> {
        Ok(None)
    }
}
#[cfg(target_family = "wasm")]
pub use web::*;
