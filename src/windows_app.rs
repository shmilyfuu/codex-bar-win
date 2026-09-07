use std::{
    cell::RefCell,
    mem::{size_of, size_of_val},
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
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::{
            BeginPaint, EndPaint, GetMonitorInfoW, GetSysColor, InvalidateRect, MonitorFromPoint,
            COLOR_HIGHLIGHT, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
        },
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
    },
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
            WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP, WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE,
            WM_THEMECHANGED, WM_TIMER, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::{
    fluent_renderer::{self, FluentRenderer, RenderModel, RenderRow},
    usage::{self, UsageSnapshot, UsageWindow},
};

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

// Documented dwmapi.h values. DWM is used only for native dark mode and rounded corners;
// the popup intentionally uses no Mica/Acrylic/system-backdrop material.
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: i32 = 2;

#[derive(Clone, Default)]
struct DisplayState {
    snapshot: Option<UsageSnapshot>,
    last_error: Option<String>,
}

static STATE: OnceLock<Arc<Mutex<DisplayState>>> = OnceLock::new();
static REFRESHING: AtomicBool = AtomicBool::new(false);

thread_local! {
    static RENDERER: RefCell<Option<FluentRenderer>> = const { RefCell::new(None) };
}

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

        let app_icon = load_app_icon(instance);
        let class_name = wide(CLASS_NAME);
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: app_icon,
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

        apply_fluent_window_attributes(hwnd);

        let renderer = match FluentRenderer::new(hwnd as isize, PANEL_WIDTH as u32, PANEL_HEIGHT as u32) {
            Ok(renderer) => renderer,
            Err(error) => {
                DestroyWindow(hwnd);
                return Err(error);
            }
        };
        RENDERER.with(|slot| *slot.borrow_mut() = Some(renderer));

        add_tray_icon(hwnd, app_icon)?;
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

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                KillTimer(hwnd, TIMER_HIDE);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            apply_fluent_window_attributes(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_popup(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_REFRESH);
            KillTimer(hwnd, TIMER_HIDE);
            remove_tray_icon(hwnd);
            RENDERER.with(|renderer| *renderer.borrow_mut() = None);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn load_app_icon(instance: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    // winresource embeds the generated icon as resource ID 1.
    let embedded = LoadIconW(instance, 1usize as *const u16);
    if embedded.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        embedded
    }
}

unsafe fn add_tray_icon(hwnd: HWND, app_icon: *mut core::ffi::c_void) -> Result<(), String> {
    let mut icon = NOTIFYICONDATAW::default();
    icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    icon.hWnd = hwnd;
    icon.uID = TRAY_ID;
    icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    icon.uCallbackMessage = WM_TRAY;
    icon.hIcon = app_icon;
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

    SetWindowPos(hwnd, HWND_TOPMOST, x, y, PANEL_WIDTH, PANEL_HEIGHT, SWP_SHOWWINDOW);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER_HIDE, HIDE_DELAY_MS, None);
    refresh_usage_async(hwnd);
    InvalidateRect(hwnd, null(), 0);
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
                    Err(error) => state.last_error = Some(error),
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

    let primary = row_strings(state.snapshot.as_ref().and_then(|snapshot| snapshot.primary.as_ref()));
    let secondary = row_strings(
        state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.secondary.as_ref()),
    );
    let footer = footer_text(&state);

    let model = RenderModel {
        status,
        status_is_error: state.last_error.is_some(),
        primary: RenderRow {
            label: "5 小时",
            value: &primary.0,
            reset: &primary.1,
            used_percent: primary.2,
        },
        secondary: RenderRow {
            label: "每周",
            value: &secondary.0,
            reset: &secondary.1,
            used_percent: secondary.2,
        },
        footer: &footer,
    };

    let mut client = RECT::default();
    GetClientRect(hwnd, &mut client);
    let width = (client.right - client.left).max(1) as f32;
    let height = (client.bottom - client.top).max(1) as f32;
    let theme = fluent_renderer::system_theme(apps_use_dark_theme(), system_highlight_rgb());

    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let result = slot
            .as_ref()
            .ok_or_else(|| "Direct2D renderer unavailable".to_string())
            .and_then(|renderer| renderer.draw(&model, theme, width, height));

        if result.is_err() {
            // Recreate once after device loss.
            if let Ok(replacement) = FluentRenderer::new(hwnd as isize, width as u32, height as u32) {
                *slot = Some(replacement);
                if let Some(renderer) = slot.as_ref() {
                    let _ = renderer.draw(&model, theme, width, height);
                }
            }
        }
    });

    EndPaint(hwnd, &paint);
}

fn row_strings(window: Option<&UsageWindow>) -> (String, String, Option<f64>) {
    let value = window
        .map(|window| format!("{:.0}% 已用", window.used_percent))
        .unwrap_or_else(|| "--".to_string());
    let reset = window
        .and_then(|window| window.reset_at)
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|time| format!("重置 {}", time.format("%m-%d %H:%M")))
        .unwrap_or_else(|| "重置时间 --".to_string());
    (value, reset, window.map(|window| window.used_percent))
}

fn footer_text(state: &DisplayState) -> String {
    if let Some(error) = state.last_error.as_deref() {
        return compact_error(error);
    }

    if let Some(snapshot) = state.snapshot.as_ref() {
        let fetched = Local
            .timestamp_opt(snapshot.fetched_at, 0)
            .single()
            .map(|time| time.format("更新 %H:%M").to_string())
            .unwrap_or_else(|| "已更新".to_string());
        return if let Some(plan) = snapshot.plan_type.as_deref() {
            format!("{} · {}", plan, fetched)
        } else {
            fetched
        };
    }

    "读取 Codex 登录信息后显示用量".to_string()
}

unsafe fn apply_fluent_window_attributes(hwnd: HWND) {
    let dark_mode = i32::from(apps_use_dark_theme());
    let _ = set_dwm_i32(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, dark_mode);
    let _ = set_dwm_i32(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
}

unsafe fn set_dwm_i32(hwnd: HWND, attribute: u32, value: i32) -> bool {
    DwmSetWindowAttribute(
        hwnd,
        attribute as _,
        &value as *const i32 as *const core::ffi::c_void,
        size_of_val(&value) as u32,
    ) >= 0
}

fn apps_use_dark_theme() -> bool {
    unsafe {
        let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let value_name = wide("AppsUseLightTheme");
        let mut value = 1_u32;
        let mut size = size_of::<u32>() as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut value as *mut u32 as *mut core::ffi::c_void,
            &mut size,
        );
        status == 0 && value == 0
    }
}

fn system_highlight_rgb() -> (u8, u8, u8) {
    unsafe {
        let color = GetSysColor(COLOR_HIGHLIGHT);
        (
            (color & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            ((color >> 16) & 0xff) as u8,
        )
    }
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
    for (index, unit) in value.encode_utf16().take(N.saturating_sub(1)).enumerate() {
        target[index] = unit;
    }
}