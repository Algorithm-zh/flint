use std::path::Path;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Hand a path (exe, lnk, document, `shell:AppsFolder\...` AUMID) to the shell.
pub fn open(path: &str) -> Result<(), String> {
    let file = HSTRING::from(path);
    let op = HSTRING::from("open");
    let r = unsafe {
        ShellExecuteW(
            None,
            &op,
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = r.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("无法打开（ShellExecute 返回 {}）", code))
    }
}

/// Open the containing folder with the item selected.
///
/// `explorer /select,<path>` is fussy about quoting: the path must be wrapped
/// on its own, not the whole argument. `std::process::Command` quotes whole
/// arguments, which silently degrades to "open the default folder", so the
/// command line goes through ShellExecuteW where we control it exactly.
pub fn reveal(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_dir() {
        return open(path);
    }
    // Virtual entries (UWP / AUMID) have no folder behind them.
    if !p.exists() {
        return open(path);
    }

    let exe = HSTRING::from("explorer.exe");
    let op = HSTRING::from("open");
    let params = HSTRING::from(format!("/select,\"{}\"", path));
    let r = unsafe {
        ShellExecuteW(
            None,
            &op,
            &exe,
            &params,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = r.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("无法打开所在位置（ShellExecute 返回 {}）", code))
    }
}
