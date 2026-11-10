//! Enumerate the windows currently on screen.
//!
//! This is what makes Flint more than a launcher: if the thing is already
//! running, searching for it should take you *there*, not start a second copy.

use serde::Serialize;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
};

#[derive(Clone, Debug, Serialize)]
pub struct WinEntry {
    pub hwnd: isize,
    pub title: String,
    /// Bare executable name, e.g. "Code.exe" — used as a searchable alias.
    pub exe: String,
    /// Full path, used to resolve the icon.
    pub exe_path: String,
}

/// Titles that belong to the shell itself rather than to a user window.
const SHELL_TITLES: &[&str] = &[
    "Program Manager",
    "Windows Input Experience",
    "MSCTFIME UI",
    "Default IME",
    "PopupHost",
    "Windows Shell Experience Host",
];

pub fn list(exclude_hwnd: isize) -> Vec<WinEntry> {
    let mut out: Vec<WinEntry> = Vec::with_capacity(64);
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut out as *mut Vec<WinEntry> as isize),
        );
    }
    out.retain(|w| {
        w.hwnd != exclude_hwnd
            && !w.title.is_empty()
            && !SHELL_TITLES.iter().any(|t| w.title.eq_ignore_ascii_case(t))
    });

    // Same app can own several windows; keep them, but drop exact duplicates.
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out.dedup_by(|a, b| a.hwnd == b.hwnd);
    out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<WinEntry>);

    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    // Owned windows are dialogs; they'd clutter the list.
    if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
        if !owner.is_invalid() {
            return BOOL(1);
        }
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 {
        return BOOL(1);
    }

    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return BOOL(1);
    }
    let mut buf = vec![0u16; len as usize + 2];
    let got = GetWindowTextW(hwnd, &mut buf);
    if got <= 0 {
        return BOOL(1);
    }
    let title = String::from_utf16_lossy(&buf[..got as usize]);
    if title.trim().is_empty() {
        return BOOL(1);
    }

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return BOOL(1);
    }

    let (exe, exe_path) = exe_of(pid).unwrap_or_default();

    out.push(WinEntry {
        hwnd: hwnd.0 as isize,
        title,
        exe,
        exe_path,
    });
    BOOL(1)
}

/// Resolve a pid to (file name, full path). Needs only limited query rights,
/// which avoids elevation failures on system processes.
fn exe_of(pid: u32) -> Option<(String, String)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        res.ok()?;

        let path = String::from_utf16_lossy(&buf[..size as usize]);
        if path.is_empty() {
            return None;
        }
        let name = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&path)
            .to_string();
        Some((name, path))
    }
}

/// Bring an existing window to the front — and above whatever was covering it.
pub fn focus(hwnd: isize) -> bool {
    crate::platform::force_foreground(hwnd)
}

/// If this entry points at a program that already owns a visible window,
/// return that window.
///
/// Handles both a plain `.exe` and the Start-menu `.lnk` that most entries
/// actually are: the shortcut has to be resolved to its target first, because
/// the window's process reports the real executable path.
pub fn window_for_path(path: &str, wins: &[WinEntry]) -> Option<isize> {
    let lower = path.to_ascii_lowercase();
    let target = if lower.ends_with(".lnk") {
        lnk_target(path)?
    } else if lower.ends_with(".exe") {
        path.to_string()
    } else {
        return None;
    };

    wins.iter()
        .find(|w| !w.exe_path.is_empty() && w.exe_path.eq_ignore_ascii_case(&target))
        .map(|w| w.hwnd)
}

/// Resolve a `.lnk` to the executable it launches.
fn lnk_target(lnk: &str) -> Option<String> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;

        let mut wide: Vec<u16> = lnk.encode_utf16().collect();
        wide.push(0);
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;

        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

