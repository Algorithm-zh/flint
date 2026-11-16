//! Neovim hand-off.
//!
//! Same shape as the omp integration: Flint never hosts the editor, it finds it
//! and gives it a window. Opening the directory you searched for is the whole
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

/// Neovide, when installed.
///
/// It is a GUI front end rather than a terminal, so it is used only for the
/// editor: it draws its own window (no terminal padding frame), renders the
/// cursor itself (guicursor needs no escape codes), and still runs the exact
/// same config.
pub fn locate_gui() -> Option<PathBuf> {
    crate::platform::on_path("neovide.exe")
}

/// Open `target` in Neovim.
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

    if let Some(gui) = locate_gui() {
        return crate::platform::spawn_gui(Path::new(&dir), &gui, &[&arg]);
    }

    // No Neovide: one terminal abstraction for the whole app — WezTerm when
    // present, then Windows Terminal, then a bare console.
    crate::platform::spawn_in_terminal(&dir, "nvim", &[&arg])
}
