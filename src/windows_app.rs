use std::{
    cell::RefCell,
    mem::{size_of, size_of_val},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
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
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            GetCursorPos, GetMessageW, GetWindowRect, IsWindowVisible, KillTimer, LoadCursorW,
            LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
            SetForegroundWindow, SetProcessDPIAware, SetTimer, SetWindowPos, ShowWindow,
            TranslateMessage, WNDCLASSW, HWND_TOPMOST, IDC_ARROW, IDI_APPLICATION, MB_ICONERROR,
            MB_OK, MSG, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_SHOWWINDOW, WA_INACTIVE,
            WM_ACTIVATE, WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE,
            WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_THEMECHANGED, WM_TIMER,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::{
    fluent_renderer::{self, FluentRenderer, RenderCredit, RenderModel, RenderRow},
    menu_renderer::{self, MenuItem, MenuModel, MenuRenderer, MENU_HEIGHT, MENU_WIDTH},
    usage::{self, ResetCreditsSnapshot, UsageSnapshot, UsageWindow},
};

const CLASS_NAME: &str = "CodexBarWinPopup";
const MENU_CLASS_NAME: &str = "CodexBarWinMenu";
const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_USAGE_UPDATED: u32 = WM_APP + 2;
const WM_CREDITS_UPDATED: u32 = WM_APP + 3;
const TIMER_REFRESH: usize = 1;
const TIMER_HIDE: usize = 2;
const HIDE_DELAY_MS: u32 = 8 * 1000;
const PANEL_WIDTH: i32 = 320;
const PANEL_HEIGHT_BASE: i32 = 212;
const PANEL_HEIGHT_CREDITS: i32 = 252;
const USAGE_DEBOUNCE: Duration = Duration::from_secs(10);
const TRAY_DEACTIVATE_TOGGLE_WINDOW: Duration = Duration::from_millis(750);
const CREDITS_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);

const DISPLAY_REMAINING: u32 = 0;
const DISPLAY_USED: u32 = 1;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: i32 = 2;

#[derive(Clone, Default)]
struct DisplayState {
    snapshot: Option<UsageSnapshot>,
    credits: Option<ResetCreditsSnapshot>,
    last_error: Option<String>,
}

static STATE: OnceLock<Arc<Mutex<DisplayState>>> = OnceLock::new();
static MAIN_HWND: OnceLock<isize> = OnceLock::new();
static MENU_HWND: OnceLock<isize> = OnceLock::new();
static REFRESHING: AtomicBool = AtomicBool::new(false);
static CREDITS_REFRESHING: AtomicBool = AtomicBool::new(false);
static DISPLAY_MODE: AtomicU32 = AtomicU32::new(DISPLAY_REMAINING);
static REFRESH_MINUTES: AtomicU32 = AtomicU32::new(5);
static LAST_USAGE_REQUEST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static LAST_CREDITS_REQUEST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static LAST_POPUP_DEACTIVATE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

thread_local! {
    static RENDERER: RefCell<Option<FluentRenderer>> = const { RefCell::new(None) };
    static MENU_RENDERER: RefCell<Option<MenuRenderer>> = const { RefCell::new(None) };
    static MENU_HOVER: RefCell<Option<MenuItem>> = const { RefCell::new(None) };
}

pub fn run() -> Result<(), String> {
    STATE
        .set(Arc::new(Mutex::new(DisplayState::default())))
        .map_err(|_| "Application state is already initialized".to_string())?;
    let _ = LAST_USAGE_REQUEST.set(Mutex::new(None));
    let _ = LAST_CREDITS_REQUEST.set(Mutex::new(None));
    let _ = LAST_POPUP_DEACTIVATE.set(Mutex::new(None));

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

        let menu_class_name = wide(MENU_CLASS_NAME);
        let menu_class = WNDCLASSW {
            lpfnWndProc: Some(menu_window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: menu_class_name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&menu_class) == 0 {
            return Err("Cannot register tray menu window class".to_string());
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
            PANEL_HEIGHT_BASE,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err("Cannot create popup window".to_string());
        }

        let menu_title = wide("Codex Usage Menu");
        let menu_hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            menu_class_name.as_ptr(),
            menu_title.as_ptr(),
            WS_POPUP,
            0,
            0,
            MENU_WIDTH,
            MENU_HEIGHT,
            hwnd,
            null_mut(),
            instance,
            null(),
        );
        if menu_hwnd.is_null() {
            DestroyWindow(hwnd);
            return Err("Cannot create tray menu window".to_string());
        }

        let _ = MAIN_HWND.set(hwnd as isize);
        let _ = MENU_HWND.set(menu_hwnd as isize);
        apply_fluent_window_attributes(hwnd);
        apply_fluent_window_attributes(menu_hwnd);

        let renderer = match FluentRenderer::new(
            hwnd as isize,
            PANEL_WIDTH as u32,
            PANEL_HEIGHT_CREDITS as u32,
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                DestroyWindow(menu_hwnd);
                DestroyWindow(hwnd);
                return Err(error);
            }
        };
        RENDERER.with(|slot| *slot.borrow_mut() = Some(renderer));

        let menu_renderer = match MenuRenderer::new(menu_hwnd as isize) {
            Ok(renderer) => renderer,
            Err(error) => {
                DestroyWindow(menu_hwnd);
                DestroyWindow(hwnd);
                return Err(error);
            }
        };
        MENU_RENDERER.with(|slot| *slot.borrow_mut() = Some(menu_renderer));

        add_tray_icon(hwnd, app_icon)?;
        set_refresh_timer(hwnd);
        refresh_usage_async(hwnd);
        refresh_reset_credits_if_due(hwnd);

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
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => toggle_popup(hwnd),
                WM_RBUTTONUP => show_context_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            match _wparam {
                TIMER_REFRESH => {
                    refresh_usage_async(hwnd);
                    refresh_reset_credits_if_due(hwnd);
                }
                TIMER_HIDE => {
                    KillTimer(hwnd, TIMER_HIDE);
                    ShowWindow(hwnd, SW_HIDE);
                }
                _ => {}
            }
            0
        }
        WM_USAGE_UPDATED | WM_CREDITS_UPDATED => {
            resize_popup_for_content(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ACTIVATE => {
            if (_wparam & 0xffff) as u32 == WA_INACTIVE && IsWindowVisible(hwnd) != 0 {
                KillTimer(hwnd, TIMER_HIDE);
                mark_popup_deactivated();
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            apply_fluent_window_attributes(hwnd);
            InvalidateRect(hwnd, null(), 0);
            if let Some(menu) = menu_hwnd() {
                apply_fluent_window_attributes(menu);
                InvalidateRect(menu, null(), 0);
            }
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
            if let Some(menu) = menu_hwnd() {
                if !menu.is_null() {
                    DestroyWindow(menu);
                }
            }
            RENDERER.with(|renderer| *renderer.borrow_mut() = None);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, _wparam, lparam),
    }
}

unsafe extern "system" fn menu_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEMOVE => {
            let (x, y) = point_from_lparam(lparam);
            let item = menu_renderer::hit_test(x, y);
            let changed = MENU_HOVER.with(|hover| {
                let mut hover = hover.borrow_mut();
                if *hover != item {
                    *hover = item;
                    true
                } else {
                    false
                }
            });
            if changed {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = point_from_lparam(lparam);
            if let Some(item) = menu_renderer::hit_test(x, y) {
                apply_menu_action(hwnd, item);
            }
            0
        }
        WM_RBUTTONUP => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                ShowWindow(hwnd, SW_HIDE);
                MENU_HOVER.with(|hover| *hover.borrow_mut() = None);
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
            paint_menu(hwnd);
            0
        }
        WM_DESTROY => {
            MENU_RENDERER.with(|renderer| *renderer.borrow_mut() = None);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn load_app_icon(instance: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
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

unsafe fn toggle_popup(hwnd: HWND) {
    if IsWindowVisible(hwnd) != 0 {
        KillTimer(hwnd, TIMER_HIDE);
        clear_popup_deactivate_marker();
        ShowWindow(hwnd, SW_HIDE);
        return;
    }

    // Clicking the tray icon while the popup owns focus first causes WA_INACTIVE.
    // The shell then delivers WM_TRAY. Treat that very recent deactivate as the
    // same toggle gesture instead of reopening the popup immediately.
    if consume_recent_popup_deactivate() {
        return;
    }

    if let Some(menu) = menu_hwnd() {
        ShowWindow(menu, SW_HIDE);
    }
    show_popup(hwnd);
}

unsafe fn show_popup(hwnd: HWND) {
    clear_popup_deactivate_marker();

    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor);

    let height = current_panel_height();
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
    let y = (work.bottom - height - 10).max(work.top + 8);

    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        PANEL_WIDTH,
        height,
        SWP_SHOWWINDOW,
    );
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER_HIDE, HIDE_DELAY_MS, None);
    refresh_usage_async(hwnd);
    refresh_reset_credits_if_due(hwnd);
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn show_context_menu(main_hwnd: HWND) {
    clear_popup_deactivate_marker();

    let Some(menu_hwnd) = menu_hwnd() else {
        return;
    };

    if IsWindowVisible(menu_hwnd) != 0 {
        ShowWindow(menu_hwnd, SW_HIDE);
        return;
    }

    KillTimer(main_hwnd, TIMER_HIDE);
    ShowWindow(main_hwnd, SW_HIDE);

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
    let max_x = (work.right - MENU_WIDTH - 8).max(min_x);
    let x = (cursor.x - MENU_WIDTH / 2).clamp(min_x, max_x);
    let min_y = work.top + 8;
    let max_y = (work.bottom - MENU_HEIGHT - 8).max(min_y);
    let y = (cursor.y - MENU_HEIGHT - 8).clamp(min_y, max_y);

    MENU_HOVER.with(|hover| *hover.borrow_mut() = None);
    SetWindowPos(
        menu_hwnd,
        HWND_TOPMOST,
        x,
        y,
        MENU_WIDTH,
        MENU_HEIGHT,
        SWP_SHOWWINDOW,
    );
    ShowWindow(menu_hwnd, SW_SHOW);
    SetForegroundWindow(menu_hwnd);
    InvalidateRect(menu_hwnd, null(), 0);
}

unsafe fn apply_menu_action(menu_hwnd: HWND, item: MenuItem) {
    let Some(main_hwnd) = main_hwnd() else {
        ShowWindow(menu_hwnd, SW_HIDE);
        return;
    };

    match item {
        MenuItem::Refresh => {
            refresh_usage_async(main_hwnd);
            refresh_reset_credits_if_due(main_hwnd);
        }
        MenuItem::Remaining => {
            DISPLAY_MODE.store(DISPLAY_REMAINING, Ordering::Release);
            InvalidateRect(main_hwnd, null(), 0);
        }
        MenuItem::Used => {
            DISPLAY_MODE.store(DISPLAY_USED, Ordering::Release);
            InvalidateRect(main_hwnd, null(), 0);
        }
        MenuItem::Refresh5 => set_refresh_minutes(main_hwnd, 5),
        MenuItem::Refresh30 => set_refresh_minutes(main_hwnd, 30),
        MenuItem::Refresh60 => set_refresh_minutes(main_hwnd, 60),
        MenuItem::Exit => {
            ShowWindow(menu_hwnd, SW_HIDE);
            remove_tray_icon(main_hwnd);
            DestroyWindow(main_hwnd);
            return;
        }
    }

    ShowWindow(menu_hwnd, SW_HIDE);
}

fn refresh_usage_async(hwnd: HWND) {
    if REFRESHING.load(Ordering::Acquire) || !mark_request_due(&LAST_USAGE_REQUEST, USAGE_DEBOUNCE) {
        return;
    }
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
                        if let Some(count) = snapshot.reset_credit_count {
                            if count == 0 {
                                state.credits = Some(ResetCreditsSnapshot::default());
                            } else if let Some(credits) = state.credits.as_mut() {
                                credits.available_count = count;
                            } else {
                                state.credits = Some(ResetCreditsSnapshot {
                                    available_count: count,
                                    earliest_expires_at: None,
                                });
                            }
                        }
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

fn refresh_reset_credits_if_due(hwnd: HWND) {
    if CREDITS_REFRESHING.load(Ordering::Acquire)
        || !mark_request_due(&LAST_CREDITS_REQUEST, CREDITS_REFRESH_INTERVAL)
    {
        return;
    }
    if CREDITS_REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }

    let hwnd_value = hwnd as isize;
    thread::spawn(move || {
        if let Ok(snapshot) = usage::fetch_reset_credits() {
            if let Some(state) = STATE.get() {
                if let Ok(mut state) = state.lock() {
                    state.credits = Some(snapshot);
                }
            }
        }

        CREDITS_REFRESHING.store(false, Ordering::Release);
        unsafe {
            PostMessageW(hwnd_value as HWND, WM_CREDITS_UPDATED, 0, 0);
        }
    });
}

fn mark_request_due(slot: &OnceLock<Mutex<Option<Instant>>>, min_interval: Duration) -> bool {
    let Some(slot) = slot.get() else {
        return true;
    };
    let Ok(mut last) = slot.lock() else {
        return false;
    };
    let now = Instant::now();
    if last
        .as_ref()
        .is_some_and(|previous| now.duration_since(*previous) < min_interval)
    {
        return false;
    }
    *last = Some(now);
    true
}

fn mark_popup_deactivated() {
    let Some(slot) = LAST_POPUP_DEACTIVATE.get() else {
        return;
    };
    if let Ok(mut last) = slot.lock() {
        *last = Some(Instant::now());
    }
}

fn consume_recent_popup_deactivate() -> bool {
    let Some(slot) = LAST_POPUP_DEACTIVATE.get() else {
        return false;
    };
    let Ok(mut last) = slot.lock() else {
        return false;
    };
    let now = Instant::now();
    let recent = last
        .as_ref()
        .is_some_and(|previous| now.duration_since(*previous) <= TRAY_DEACTIVATE_TOGGLE_WINDOW);
    *last = None;
    recent
}

fn clear_popup_deactivate_marker() {
    let Some(slot) = LAST_POPUP_DEACTIVATE.get() else {
        return;
    };
    if let Ok(mut last) = slot.lock() {
        *last = None;
    }
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

    let remaining = DISPLAY_MODE.load(Ordering::Acquire) == DISPLAY_REMAINING;
    let primary = row_strings(
        state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.primary.as_ref()),
        remaining,
    );
    let secondary = row_strings(
        state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.secondary.as_ref()),
        remaining,
    );
    let identity = identity_text(state.snapshot.as_ref());
    let footer = footer_text(&state);
    let credit = credit_strings(state.credits.as_ref());
    let reset_credit = credit.as_ref().map(|(value, expiry)| RenderCredit {
        value,
        expiry,
    });

    let model = RenderModel {
        status,
        status_is_error: state.last_error.is_some(),
        identity: &identity,
        primary: RenderRow {
            label: "5 小时",
            value: &primary.0,
            reset: &primary.1,
            percent: primary.2,
        },
        secondary: RenderRow {
            label: "每周",
            value: &secondary.0,
            reset: &secondary.1,
            percent: secondary.2,
        },
        reset_credit,
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
            if let Ok(replacement) = FluentRenderer::new(
                hwnd as isize,
                PANEL_WIDTH as u32,
                PANEL_HEIGHT_CREDITS as u32,
            ) {
                *slot = Some(replacement);
                if let Some(renderer) = slot.as_ref() {
                    let _ = renderer.draw(&model, theme, width, height);
                }
            }
        }
    });

    EndPaint(hwnd, &paint);
}

unsafe fn paint_menu(hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut paint);
    if hdc.is_null() {
        return;
    }

    let hovered = MENU_HOVER.with(|hover| *hover.borrow());
    let model = MenuModel {
        hovered,
        display_remaining: DISPLAY_MODE.load(Ordering::Acquire) == DISPLAY_REMAINING,
        refresh_minutes: REFRESH_MINUTES.load(Ordering::Acquire),
    };
    let theme = fluent_renderer::system_theme(apps_use_dark_theme(), system_highlight_rgb());

    MENU_RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let result = slot
            .as_ref()
            .ok_or_else(|| "Menu renderer unavailable".to_string())
            .and_then(|renderer| renderer.draw(&model, theme));
        if result.is_err() {
            if let Ok(replacement) = MenuRenderer::new(hwnd as isize) {
                *slot = Some(replacement);
                if let Some(renderer) = slot.as_ref() {
                    let _ = renderer.draw(&model, theme);
                }
            }
        }
    });

    EndPaint(hwnd, &paint);
}

fn row_strings(window: Option<&UsageWindow>, remaining: bool) -> (String, String, Option<f64>) {
    let shown_percent = window.map(|window| {
        if remaining {
            100.0 - window.used_percent
        } else {
            window.used_percent
        }
    });
    let value = shown_percent
        .map(|percent| {
            format!(
                "{:.0}% {}",
                percent,
                if remaining { "剩余" } else { "已用" }
            )
        })
        .unwrap_or_else(|| "--".to_string());
    let reset = window
        .and_then(|window| window.reset_at)
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|time| format!("重置 {}", time.format("%m-%d %H:%M")))
        .unwrap_or_else(|| "重置时间 --".to_string());
    (value, reset, shown_percent)
}

fn identity_text(snapshot: Option<&UsageSnapshot>) -> String {
    let email = snapshot.and_then(|snapshot| snapshot.email.as_deref());
    let plan = snapshot
        .and_then(|snapshot| snapshot.plan_type.as_deref())
        .map(format_plan);

    let text = match (email, plan.as_deref()) {
        (Some(email), Some(plan)) => format!("{email} · {plan}"),
        (Some(email), None) => email.to_string(),
        (None, Some(plan)) => plan.to_string(),
        (None, None) => "Codex 账户".to_string(),
    };
    compact_middle(&text, 46)
}

fn credit_strings(credits: Option<&ResetCreditsSnapshot>) -> Option<(String, String)> {
    let credits = credits.filter(|credits| credits.available_count > 0)?;
    let value = format!("{} 张可用", credits.available_count);
    let expiry = credits
        .earliest_expires_at
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|time| format!("最近 {} 到期", time.format("%m-%d %H:%M")))
        .unwrap_or_else(|| "到期时间暂不可用".to_string());
    Some((value, expiry))
}

fn footer_text(state: &DisplayState) -> String {
    if let Some(error) = state.last_error.as_deref() {
        return compact_error(error);
    }

    if let Some(snapshot) = state.snapshot.as_ref() {
        return Local
            .timestamp_opt(snapshot.fetched_at, 0)
            .single()
            .map(|time| time.format("更新 %H:%M").to_string())
            .unwrap_or_else(|| "已更新".to_string());
    }

    "读取 Codex 登录信息后显示用量".to_string()
}

fn current_panel_height() -> i32 {
    let has_credits = STATE
        .get()
        .and_then(|state| state.lock().ok())
        .and_then(|state| state.credits.as_ref().map(|credits| credits.available_count > 0))
        .unwrap_or(false);
    if has_credits {
        PANEL_HEIGHT_CREDITS
    } else {
        PANEL_HEIGHT_BASE
    }
}

unsafe fn resize_popup_for_content(hwnd: HWND) {
    if IsWindowVisible(hwnd) == 0 {
        return;
    }

    let height = current_panel_height();
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return;
    }

    let bottom = rect.bottom;
    let monitor = MonitorFromPoint(
        POINT {
            x: rect.left,
            y: bottom,
        },
        MONITOR_DEFAULTTONEAREST,
    );
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    GetMonitorInfoW(monitor, &mut monitor_info);
    let y = (bottom - height).max(monitor_info.rcWork.top + 8);

    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        rect.left,
        y,
        PANEL_WIDTH,
        height,
        SWP_NOACTIVATE,
    );
}

fn set_refresh_minutes(hwnd: HWND, minutes: u32) {
    let minutes = match minutes {
        5 | 30 | 60 => minutes,
        _ => 5,
    };
    REFRESH_MINUTES.store(minutes, Ordering::Release);
    unsafe {
        KillTimer(hwnd, TIMER_REFRESH);
        set_refresh_timer(hwnd);
    }
}

unsafe fn set_refresh_timer(hwnd: HWND) {
    let minutes = REFRESH_MINUTES.load(Ordering::Acquire).max(1);
    SetTimer(hwnd, TIMER_REFRESH, minutes * 60 * 1000, None);
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

fn main_hwnd() -> Option<HWND> {
    MAIN_HWND.get().copied().map(|value| value as HWND)
}

fn menu_hwnd() -> Option<HWND> {
    MENU_HWND.get().copied().map(|value| value as HWND)
}

fn point_from_lparam(lparam: LPARAM) -> (i32, i32) {
    let raw = lparam as u32;
    let x = (raw & 0xffff) as u16 as i16 as i32;
    let y = ((raw >> 16) & 0xffff) as u16 as i16 as i32;
    (x, y)
}

fn format_plan(plan: &str) -> String {
    let plan = plan.trim();
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn compact_middle(value: &str, max_chars: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let left = max_chars / 2;
    let right = max_chars.saturating_sub(left + 1);
    format!(
        "{}…{}",
        chars[..left].iter().collect::<String>(),
        chars[chars.len() - right..].iter().collect::<String>()
    )
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
    for (index, unit) in value
        .encode_utf16()
        .take(N.saturating_sub(1))
        .enumerate()
    {
        target[index] = unit;
    }
}
