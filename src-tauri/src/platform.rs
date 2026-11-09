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
            if tid != 0 && tid != ctid && !attached.contains(&tid) {
                if AttachThreadInput(tid, ctid, true).as_bool() {
                    attached.push(tid);
                }
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
