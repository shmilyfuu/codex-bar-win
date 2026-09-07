use std::{
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
};

use chrono::{Local, TimeZone};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW,
        GetStockObject, GetSysColor, InvalidateRect, MonitorFromPoint, SelectObject, SetBkMode,
        SetTextColor, COLOR_BTNFACE, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_WINDOW,
        COLOR_WINDOWTEXT, DEFAULT_GUI_FONT, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
        MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
            NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
            DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, KillTimer,
            LoadCursorW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
            SetForegroundWindow, SetProcessDPIAware, SetTimer, SetWindowPos, ShowWindow,
            TrackPopupMenu, TranslateMessage, WNDCLASSW, HWND_TOPMOST, IDC_ARROW, IDI_APPLICATION,
            MB_ICONERROR, MB_OK, MF_SEPARATOR, MF_STRING, MSG, SW_HIDE, SW_SHOW, SWP_SHOWWINDOW,
            TPM_RETURNCMD, TPM_RIGHTBUTTON, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_COMMAND,
            WM_DESTROY, WM_LBUTTONUP, WM_PAINT, WM_RBUTTONUP, WM_TIMER, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::usage::{self, UsageSnapshot, UsageWindow};

const CLASS_NAME: &str = "CodexBarWinPopup";
const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_USAGE_UPDATED: u32 = WM_APP + 2;
const TIMER_REFRESH: usize = 1;
const TIMER_HIDE: usize = 2;
const REFRESH_INTERVAL_MS: u32 = 5 * 60 * 1000;
const HIDE_DELAY_MS: u32 = 8 * 1000;
const PANEL_WIDTH: i32 = 320;
const PANEL_HEIGHT: i32 = 198;
const CMD_REFRESH: usize = 1001;
const CMD_EXIT: usize = 1002;

#[derive(Clone, Default)]
struct DisplayState {
    snapshot: Option<UsageSnapshot>,
    last_error: Option<String>,
}

static STATE: OnceLock<Arc<Mutex<DisplayState>>> = OnceLock::new();
static REFRESHING: AtomicBool = AtomicBool::new(false);

pub fn run() -> Result<(), String> {
    STATE
        .set(Arc::new(Mutex::new(DisplayState::default())))
        .map_err(|_| "Application state is already initialized".to_string())?;

    unsafe {
        SetProcessDPIAware();

        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err("Cannot get application module handle".to_string());
        }

        let class_name = wide(CLASS_NAME);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: LoadIconW(null_mut(), IDI_APPLICATION),
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };

        if RegisterClassW(&window_class) == 0 {
            return Err("Cannot register popup window class".to_string());
        }

        let title = wide("Codex Usage");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );

        if hwnd.is_null() {
            return Err("Cannot create popup window".to_string());
        }

        add_tray_icon(hwnd)?;
        SetTimer(hwnd, TIMER_REFRESH, REFRESH_INTERVAL_MS, None);
        refresh_usage_async(hwnd);

        let mut message = MSG::default();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        remove_tray_icon(hwnd);
    }

    Ok(())
}

pub fn show_fatal_error(message: &str) {
    unsafe {
        let text = wide(message);
        let title = wide("Codex Usage");
        MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => show_popup(hwnd),
                WM_RBUTTONUP => show_context_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            match wparam & 0xffff {
                CMD_REFRESH => refresh_usage_async(hwnd),
                CMD_EXIT => {
                    remove_tray_icon(hwnd);
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_TIMER => {
            match wparam {
                TIMER_REFRESH => refresh_usage_async(hwnd),
                TIMER_HIDE => {
                    KillTimer(hwnd, TIMER_HIDE);
                    ShowWindow(hwnd, SW_HIDE);
                }
                _ => {}
            }
            0
        }
        WM_USAGE_UPDATED => {
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                KillTimer(hwnd, TIMER_HIDE);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_PAINT => {
            paint_popup(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_REFRESH);
            KillTimer(hwnd, TIMER_HIDE);
            remove_tray_icon(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn add_tray_icon(hwnd: HWND) -> Result<(), String> {
    let mut icon = NOTIFYICONDATAW::default();
    icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    icon.hWnd = hwnd;
    icon.uID = TRAY_ID;
    icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    icon.uCallbackMessage = WM_TRAY;
    icon.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
    copy_wide_fixed("Codex Usage", &mut icon.szTip);

    if Shell_NotifyIconW(NIM_ADD, &icon) == 0 {
        return Err("Cannot add tray icon".to_string());
    }

    Ok(())
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut icon = NOTIFYICONDATAW::default();
    icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    icon.hWnd = hwnd;
    icon.uID = TRAY_ID;
    Shell_NotifyIconW(NIM_DELETE, &icon);
}

unsafe fn show_popup(hwnd: HWND) {
    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor);

    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    GetMonitorInfoW(monitor, &mut monitor_info);

    let work = monitor_info.rcWork;
    let min_x = work.left + 8;
    let max_x = (work.right - PANEL_WIDTH - 8).max(min_x);
    let x = (cursor.x - PANEL_WIDTH / 2).clamp(min_x, max_x);
    let y = (work.bottom - PANEL_HEIGHT - 10).max(work.top + 8);

    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        PANEL_WIDTH,
        PANEL_HEIGHT,
        SWP_SHOWWINDOW,
    );
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER_HIDE, HIDE_DELAY_MS, None);
    refresh_usage_async(hwnd);
    InvalidateRect(hwnd, null(), 1);
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    let refresh = wide("刷新");
    let exit = wide("退出");
    AppendMenuW(menu, MF_STRING, CMD_REFRESH, refresh.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
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
        null(),
    ) as usize;

    DestroyMenu(menu);

    match command {
        CMD_REFRESH => refresh_usage_async(hwnd),
        CMD_EXIT => {
            remove_tray_icon(hwnd);
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn refresh_usage_async(hwnd: HWND) {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }

    let hwnd_value = hwnd as isize;
    thread::spawn(move || {
        let result = usage::fetch_usage();

        if let Some(state) = STATE.get() {
            if let Ok(mut state) = state.lock() {
                match result {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.last_error = None;
                    }
                    Err(error) => {
                        state.last_error = Some(error);
                    }
                }
            }
        }

        REFRESHING.store(false, Ordering::Release);
        unsafe {
            PostMessageW(hwnd_value as HWND, WM_USAGE_UPDATED, 0, 0);
        }
    });
}

unsafe fn paint_popup(hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut paint);
    if hdc.is_null() {
        return;
    }

    let mut client = RECT::default();
    GetClientRect(hwnd, &mut client);

    let background = CreateSolidBrush(GetSysColor(COLOR_WINDOW));
    FillRect(hdc, &client, background);
    DeleteObject(background);

    let old_font = SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));

    draw_text(hdc, "Codex Usage", 16, 12, 198, 34, DT_LEFT | DT_VCENTER);

    let state = STATE
        .get()
        .and_then(|state| state.lock().ok())
        .map(|state| state.clone())
        .unwrap_or_default();

    let status = if REFRESHING.load(Ordering::Acquire) {
        "正在更新…"
    } else if state.last_error.is_some() {
        "更新失败"
    } else if state.snapshot.is_some() {
        "已更新"
    } else {
        "等待更新"
    };
    SetTextColor(hdc, GetSysColor(COLOR_GRAYTEXT));
    draw_text(hdc, status, 200, 12, 304, 34, DT_RIGHT | DT_VCENTER);

    SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
    draw_usage_row(
        hdc,
        "5 小时",
        state.snapshot.as_ref().and_then(|snapshot| snapshot.primary.as_ref()),
        46,
    );
    draw_usage_row(
        hdc,
        "每周",
        state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.secondary.as_ref()),
        108,
    );

    SetTextColor(hdc, GetSysColor(COLOR_GRAYTEXT));
    let footer = if let Some(error) = state.last_error.as_deref() {
        compact_error(error)
    } else if let Some(snapshot) = state.snapshot.as_ref() {
        let fetched = Local
            .timestamp_opt(snapshot.fetched_at, 0)
            .single()
            .map(|time| time.format("更新 %H:%M").to_string())
            .unwrap_or_else(|| "已更新".to_string());
        if let Some(plan) = snapshot.plan_type.as_deref() {
            format!("{} · {}", plan, fetched)
        } else {
            fetched
        }
    } else {
        "读取 Codex 登录信息后显示用量".to_string()
    };
    draw_text(hdc, &footer, 16, 172, 304, 190, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

    SelectObject(hdc, old_font);
    EndPaint(hwnd, &paint);
}

unsafe fn draw_usage_row(hdc: *mut core::ffi::c_void, label: &str, window: Option<&UsageWindow>, y: i32) {
    draw_text(hdc, label, 16, y, 150, y + 20, DT_LEFT | DT_VCENTER);

    let value = window
        .map(|window| format!("{:.0}% 已用", window.used_percent))
        .unwrap_or_else(|| "--".to_string());
    draw_text(hdc, &value, 150, y, 304, y + 20, DT_RIGHT | DT_VCENTER);

    let track_rect = RECT {
        left: 16,
        top: y + 25,
        right: 304,
        bottom: y + 33,
    };
    let track = CreateSolidBrush(GetSysColor(COLOR_BTNFACE));
    FillRect(hdc, &track_rect, track);
    DeleteObject(track);

    if let Some(window) = window {
        let width = ((track_rect.right - track_rect.left) as f64 * window.used_percent / 100.0)
            .round() as i32;
        if width > 0 {
            let fill_rect = RECT {
                right: track_rect.left + width,
                ..track_rect
            };
            let fill = CreateSolidBrush(GetSysColor(COLOR_HIGHLIGHT));
            FillRect(hdc, &fill_rect, fill);
            DeleteObject(fill);
        }
    }

    SetTextColor(hdc, GetSysColor(COLOR_GRAYTEXT));
    let reset = window
        .and_then(|window| window.reset_at)
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|time| format!("重置 {}", time.format("%m-%d %H:%M")))
        .unwrap_or_else(|| "重置时间 --".to_string());
    draw_text(hdc, &reset, 16, y + 36, 304, y + 54, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
}

unsafe fn draw_text(
    hdc: *mut core::ffi::c_void,
    text: &str,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    format: u32,
) {
    let text = wide(text);
    let mut rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    DrawTextW(
        hdc,
        text.as_ptr(),
        text.len().saturating_sub(1) as i32,
        &mut rect,
        format,
    );
}

fn compact_error(error: &str) -> String {
    let single_line = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= 52 {
        single_line
    } else {
        format!("{}…", single_line.chars().take(51).collect::<String>())
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide_fixed<const N: usize>(value: &str, target: &mut [u16; N]) {
    let encoded = value.encode_utf16().take(N.saturating_sub(1));
    for (index, unit) in encoded.enumerate() {
        target[index] = unit;
    }
}
