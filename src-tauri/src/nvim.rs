//! Neovim hand-off.
//!
//! Same shape as the omp integration: Flint never hosts the editor, it finds it
//! and gives it a terminal. Opening the directory you searched for is the whole
//! point — no `cd`, no copying a path.

use std::path::{Path, PathBuf};

/// Where Neovim lands when it was installed by the official installer.
const KNOWN: &[&str] = &[
    r"C:\Program Files\Neovim\bin\nvim.exe",
    r"C:\Program Files (x86)\Neovim\bin\nvim.exe",
];

pub fn locate() -> Option<PathBuf> {
    if let Some(p) = crate::platform::on_path("nvim.exe") {
        return Some(p);
    }
    for candidate in KNOWN {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}


/// Open `target` in Neovim, in a terminal rooted at the right directory.
///
/// A directory opens as the working directory; a file opens with its folder as
/// the working directory, so `:e` and LazyVim's pickers start from the project
/// rather than somewhere unrelated.
pub fn open(target: &str) -> Result<(), String> {
    let path = Path::new(target);
    if !path.exists() {
        return Err(format!("路径不存在：{target}"));
    }

    let (dir, arg) = if path.is_dir() {
        (target.to_string(), ".".to_string())
    } else {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| target.to_string());
        (dir, path.to_string_lossy().to_string())
    };

    // One terminal abstraction for the whole app: WezTerm when present, then
    // Windows Terminal, then a bare console.
    crate::platform::spawn_in_terminal(&dir, "nvim", &[&arg])
}
