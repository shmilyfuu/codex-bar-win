use std::{ptr::null, thread, time::Duration};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{
        FindWindowW, IsWindowVisible, PostMessageW, SetForegroundWindow, WM_APP, WM_LBUTTONUP,
    },
};

const MUTEX_NAME: &str = "Local\\CodexBarWin.SingleInstance";
const WINDOW_CLASS_NAME: &str = "CodexBarWinPopup";
const WM_TRAY: u32 = WM_APP + 1;
const FIND_RETRY_COUNT: usize = 20;
const FIND_RETRY_DELAY: Duration = Duration::from_millis(50);

pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.handle.is_null() {
                CloseHandle(self.handle);
            }
        }
    }
}

pub fn acquire_or_activate_existing() -> Result<Option<SingleInstanceGuard>, String> {
    let name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!(
            "Cannot create single-instance mutex (Windows error {})",
            unsafe { GetLastError() }
        ));
    }

    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        unsafe {
            activate_existing_instance();
            CloseHandle(handle);
        }
        return Ok(None);
    }

    Ok(Some(SingleInstanceGuard { handle }))
}

unsafe fn activate_existing_instance() {
    let class_name = wide(WINDOW_CLASS_NAME);

    for _ in 0..FIND_RETRY_COUNT {
        let hwnd = FindWindowW(class_name.as_ptr(), null());
        if !hwnd.is_null() {
            if IsWindowVisible(hwnd) != 0 {
                SetForegroundWindow(hwnd);
            } else {
                // Reuse the already-tested tray-left-click path so positioning,
                // refresh throttling, and the 8-second auto-hide behavior stay identical.
                PostMessageW(hwnd, WM_TRAY, 0, WM_LBUTTONUP as isize);
            }
            return;
        }
        thread::sleep(FIND_RETRY_DELAY);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
