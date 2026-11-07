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

/// Relaunch an item elevated. Windows shows the UAC prompt; a cancelled prompt
/// surfaces as error 1223 (ERROR_CANCELLED).
pub fn run_as_admin(path: &str) -> Result<(), String> {
    let file = HSTRING::from(path);
    let op = HSTRING::from("runas");
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
    } else if code == 5 {
        Err("已取消".into())
    } else {
        Err(format!("无法以管理员运行（{}）", code))
    }
}

/// Open a shell already sitting in `dir`. Prefers Windows Terminal when it's
/// installed, falls back to a plain console.
pub fn open_terminal(dir: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new("wt.exe");
    cmd.args(["-d", dir]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    if cmd.spawn().is_ok() {
        return Ok(());
    }

    // No Windows Terminal: hand it to cmd with a working directory.
    let mut fallback = std::process::Command::new("cmd.exe");
    fallback.args(["/c", "start", "", "cmd.exe", "/k", "cd", "/d", dir]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        fallback.creation_flags(0x0800_0000);
    }
    fallback
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开终端：{e}"))
}
