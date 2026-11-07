//! A throwaway directory tree for the checks' own tests (std only).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct Tree(PathBuf);

impl Tree {
    /// Creates `(relative path, contents)` files under a fresh directory.
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "xtask-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for (path, text) in files {
            let target = dir.join(path);
            fs::create_dir_all(target.parent().expect("file has a parent")).expect("mkdir");
            fs::write(target, text).expect("write fixture");
        }
        fs::create_dir_all(&dir).expect("mkdir root");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
