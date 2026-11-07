use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    SetForegroundWindow, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

/// Windows refuses `SetForegroundWindow` from a background process unless it
/// owns the foreground thread's input queue. Temporarily attaching our thread
/// to it is the standard, reliable way around that rule — without it the
/// palette appears but swallows the first keystrokes.
pub fn force_foreground(hwnd: isize) {
    unsafe {
        let target = HWND(hwnd as *mut core::ffi::c_void);
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        let ftid = if fg.is_invalid() {
            0
        } else {
            GetWindowThreadProcessId(fg, Some(&mut pid))
        };
        let ctid = GetCurrentThreadId();

        if ftid != 0 && ftid != ctid {
            let _ = AttachThreadInput(ftid, ctid, true);
            let _ = SetForegroundWindow(target);
            let _ = BringWindowToTop(target);
            let _ = AttachThreadInput(ftid, ctid, false);
        } else {
            let _ = SetForegroundWindow(target);
            let _ = BringWindowToTop(target);
        }
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
