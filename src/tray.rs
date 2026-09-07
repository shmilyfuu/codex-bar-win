use std::{mem::size_of, ptr::null_mut, sync::OnceLock};

use async_channel::Sender;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON,
            NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetMonitorInfoW, LoadIconW,
            MonitorFromPoint, SetForegroundWindow, SetWindowPos, ShowWindow, TrackPopupMenu,
            HWND_TOPMOST, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MONITORINFO,
            MONITOR_DEFAULTTONEAREST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW,
            TPM_RETURNCMD, TPM_RIGHTBUTTON, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_LBUTTONUP,
            WM_NCDESTROY, WM_RBUTTONUP,
        },
    },
};

pub const PANEL_WIDTH: i32 = 360;
pub const PANEL_HEIGHT: i32 = 236;

const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 41;
const SUBCLASS_ID: usize = 0xC0D3_0001;
const CMD_REFRESH: usize = 1001;
const CMD_EXIT: usize = 1002;

static COMMAND_SENDER: OnceLock<Sender<TrayCommand>> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    Show,
    Hide,
    Refresh,
    Exit,
}

pub fn install(hwnd: HWND, sender: Sender<TrayCommand>) -> Result<(), String> {
    COMMAND_SENDER
        .set(sender)
        .map_err(|_| "Tray command channel is already initialized".to_string())?;

    unsafe {
        if SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) == 0 {
            return Err("Cannot attach tray message handler".to_string());
        }

        let mut icon = NOTIFYICONDATAW::default();
        icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = hwnd;
        icon.uID = TRAY_ID;
        icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        icon.uCallbackMessage = WM_TRAY;
        icon.hIcon = application_icon();
        copy_wide_fixed("Codex Usage", &mut icon.szTip);

        if Shell_NotifyIconW(NIM_ADD, &icon) == 0 {
            RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            return Err("Cannot add tray icon".to_string());
        }
    }

    Ok(())
}

pub fn show_popup(hwnd: HWND) {
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) == 0 {
            return;
        }

        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }

        let work = info.rcWork;
        let min_x = work.left + 10;
        let max_x = (work.right - PANEL_WIDTH - 10).max(min_x);
        let x = (cursor.x - PANEL_WIDTH / 2).clamp(min_x, max_x);
        let y = (work.bottom - PANEL_HEIGHT - 12).max(work.top + 10);

        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetForegroundWindow(hwnd);
    }
}

pub fn hide_popup(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let mut icon = NOTIFYICONDATAW::default();
        icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = hwnd;
        icon.uID = TRAY_ID;
        Shell_NotifyIconW(NIM_DELETE, &icon);
        RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _ref_data: usize,
) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => send(TrayCommand::Show),
                WM_RBUTTONUP => show_context_menu(hwnd),
                _ => {}
            }
            return 0;
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                send(TrayCommand::Hide);
            }
        }
        WM_NCDESTROY => {
            let mut icon = NOTIFYICONDATAW::default();
            icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            icon.hWnd = hwnd;
            icon.uID = TRAY_ID;
            Shell_NotifyIconW(NIM_DELETE, &icon);
            RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
        }
        _ => {}
    }

    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    let refresh = wide("刷新");
    let exit = wide("退出");
    AppendMenuW(menu, MF_STRING, CMD_REFRESH, refresh.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
    AppendMenuW(menu, MF_STRING, CMD_EXIT, exit.as_ptr());

    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor);
    SetForegroundWindow(hwnd);

    let command = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        std::ptr::null(),
    ) as usize;
    DestroyMenu(menu);

    match command {
        CMD_REFRESH => send(TrayCommand::Refresh),
        CMD_EXIT => send(TrayCommand::Exit),
        _ => {}
    }
}

fn send(command: TrayCommand) {
    if let Some(sender) = COMMAND_SENDER.get() {
        let _ = sender.try_send(command);
    }
}

unsafe fn application_icon() -> *mut core::ffi::c_void {
    let instance = GetModuleHandleW(std::ptr::null());
    let icon = if !instance.is_null() {
        LoadIconW(instance, 1usize as *const u16)
    } else {
        null_mut()
    };

    if icon.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        icon
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide_fixed<const N: usize>(value: &str, target: &mut [u16; N]) {
    let encoded = value.encode_utf16().take(N.saturating_sub(1));
    for (index, character) in encoded.enumerate() {
        target[index] = character;
    }
}
