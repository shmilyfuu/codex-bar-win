use std::{ptr::null, thread, time::Duration};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{FindWindowW, PostMessageW},
};

use crate::windows_app::ACTIVATE_EXISTING_MESSAGE;

const MUTEX_NAME: &str = "Local\\CodexBarWin.SingleInstance";
const WINDOW_CLASS_NAME: &str = "CodexBarWinPopup";
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
            // Ask the primary process to reveal/foreground itself. Keeping the
            // activation logic in the primary avoids coupling a second launch
            // to tray-click toggle/deactivation ordering.
            PostMessageW(hwnd, ACTIVATE_EXISTING_MESSAGE, 0, 0);
            return;
        }
        thread::sleep(FIND_RETRY_DELAY);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
