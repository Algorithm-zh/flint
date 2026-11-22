use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, SwitchToThisWindow,
    GWL_EXSTYLE, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOW,
    WS_EX_TOOLWINDOW,
};

/// Bring `hwnd` to the foreground *and* to the top of the z-order.
///
/// `SetForegroundWindow` on its own is not enough for a launcher, for three
/// separate reasons:
///
///  * Windows refuses foreground changes from a background process unless the
///    caller shares an input queue with the current foreground thread, so the
///    threads are attached for the duration of the call;
///  * a minimised window stays in the taskbar — it has to be restored first;
///  * an already-visible window that merely gets activated can still sit
///    *behind* whatever was covering it, because activation does not reorder
///    windows within the same band. `SetWindowPos(HWND_TOP)` fixes that.
///
/// Returns whether the target really ended up in the foreground.
pub fn force_foreground(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let target = HWND(hwnd as *mut core::ffi::c_void);
        if target.is_invalid() {
            return false;
        }

        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        } else {
            let _ = ShowWindow(target, SW_SHOW);
        }

        let ctid = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_tid = if fg.is_invalid() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        let tgt_tid = GetWindowThreadProcessId(target, None);

        // Attach to both the window that currently owns the foreground and the
        // target's own thread: whichever one holds the input queue, we end up
        // sharing it.
        let mut attached: Vec<u32> = Vec::with_capacity(2);
        for tid in [fg_tid, tgt_tid] {
            if tid != 0 && tid != ctid && !attached.contains(&tid)
                && AttachThreadInput(tid, ctid, true).as_bool()
            {
                attached.push(tid);
            }
        }

        // The blunt instrument. Unlike SetForegroundWindow this one is not
        // refused when the caller has no foreground rights, which is exactly
        // the situation a launcher is always in.
        SwitchToThisWindow(target, true);
        let _ = SetForegroundWindow(target);
        let _ = BringWindowToTop(target);
        // Top of the normal band, above whatever was covering it. Deliberately
        // not HWND_TOPMOST: pinning a user's window over everything would
        // outlive the launcher's intent.
        let _ = SetWindowPos(
            target,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );

        for tid in attached {
            let _ = AttachThreadInput(tid, ctid, false);
        }

        GetForegroundWindow() == target
    }
}

/// Keep the palette out of Alt-Tab / the task switcher.
pub fn hide_from_switcher(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut core::ffi::c_void);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// Centre the palette on whichever monitor the cursor is on.
///
/// The window is borderless and fixed-size, so it cannot be dragged somewhere
/// and stay there — "appear where the mouse is" is the whole positioning
/// policy, and it is what multi-monitor users expect from a launcher.
pub fn center_on_cursor_monitor(hwnd: isize, width: i32, height: i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    if hwnd == 0 {
        return;
    }

    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return;
        }

        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return;
        }

        let mut info = MONITORINFO {
            cbSize: core::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }

        // rcWork excludes the taskbar, so the palette never lands under it.
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - width) / 2;
        let y = work.top + ((work.bottom - work.top) - height) / 2;

        let h = HWND(hwnd as *mut core::ffi::c_void);
        let _ = SetWindowPos(
            h,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/* ------------------------------------------------------------------ */
/* terminals                                                          */
/* ------------------------------------------------------------------ */

use std::path::PathBuf;
use std::process::Command;

/// Which terminal emulator to hand a TUI program to.
enum Terminal {
    /// Pebrel — the one installed here. Takes the working directory and the
    /// command on its own command line, so the TUI lands in the right folder
    /// without going through a shell first.
    Pebrel(PathBuf),
    /// Windows Terminal, for machines where Pebrel is not installed.
    Windows,
    /// Last resort: a plain console window.
    Cmd,
}

pub fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Pebrel ships as an installer and does not put itself on PATH, so the
/// documented install location has to be checked as well.
fn pebrel() -> Option<PathBuf> {
    if let Some(p) = on_path("pebrel.exe") {
        return Some(p);
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    let candidate = PathBuf::from(local)
        .join("Programs")
        .join("Pebrel")
        .join("pebrel.exe");
    candidate.is_file().then_some(candidate)
}

fn terminal() -> Terminal {
    if let Some(p) = pebrel() {
        return Terminal::Pebrel(p);
    }
    if on_path("wt.exe").is_some() {
        return Terminal::Windows;
    }
    Terminal::Cmd
}

/// Launch `prog` with `args` in a new terminal window rooted at `dir`.
///
/// One entry point for every "open this in a terminal" action, so adding a
/// terminal later is a change in one place instead of three.
pub fn spawn_in_terminal(dir: &str, prog: &str, args: &[&str]) -> Result<(), String> {
    match terminal() {
        Terminal::Pebrel(exe) => {
            // pebrel --working-directory <dir> -e <prog> <args...>
            // `-e` takes the rest of the command line, so it goes last.
            let mut cmd = Command::new(exe);
            cmd.arg("--working-directory")
                .arg(dir)
                .arg("-e")
                .arg(prog)
                .args(args);
            detach(&mut cmd);
            cmd.spawn()
                .map(|_| ())
                .map_err(|e| format!("Pebrel 启动失败：{e}"))
        }
        Terminal::Windows => {
            let mut cmd = Command::new("wt.exe");
            cmd.arg("-d").arg(dir).arg(prog).args(args);
            detach(&mut cmd);
            cmd.spawn()
                .map(|_| ())
                .map_err(|e| format!("Windows Terminal 启动失败：{e}"))
        }
        Terminal::Cmd => {
            let mut cmd = Command::new("cmd.exe");
            cmd.arg("/c")
                .arg("start")
                .arg("")
                .arg("/D")
                .arg(dir)
                .arg(prog)
                .args(args);
            detach(&mut cmd);
            cmd.spawn()
                .map(|_| ())
                .map_err(|e| format!("启动终端失败：{e}"))
        }
    }
}

/// Launch a GUI program (Neovide) with `dir` as its working directory.
///
/// Unlike the terminal path there is no `--cwd` flag to pass, so the working
/// directory is set on the process and inherited by the nvim child Neovide
/// spawns.
pub fn spawn_gui(dir: &std::path::Path, exe: &std::path::Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new(exe);
    cmd.current_dir(dir).args(args);
    detach(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("启动失败：{e}"))
}

fn detach(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
}
