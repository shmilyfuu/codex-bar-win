use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use chrono::{Local, TimeZone};
use gpui_kit::{
    component::{progress::Progress, ActiveTheme as _, Root, Sizable as _, Theme, ThemeColor},
    AppContext as _, AsyncApp, Bounds, Context, IntoElement, ParentElement as _, Render,
    Styled as _, WeakEntity, Window, WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;

use crate::{
    tray::{self, TrayCommand},
    usage::{self, UsageSnapshot, UsageWindow},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const AUTO_HIDE_DELAY: Duration = Duration::from_secs(8);

static REFRESHING: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct UsageView {
    snapshot: Option<UsageSnapshot>,
    error: Option<String>,
    refreshing: bool,
}

impl UsageView {
    fn apply_result(&mut self, result: Result<UsageSnapshot, String>) {
        self.refreshing = false;
        match result {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
    }
}

impl Render for UsageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let plan = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.plan_type.as_deref())
            .map(|plan| plan.to_uppercase());

        let status_text = if self.refreshing {
            "正在更新"
        } else if self.error.is_some() {
            "更新失败"
        } else if self.snapshot.is_some() {
            "已更新"
        } else {
            "等待更新"
        };

        let footer = if let Some(error) = self.error.as_deref() {
            compact_error(error)
        } else if let Some(snapshot) = self.snapshot.as_ref() {
            Local
                .timestamp_opt(snapshot.fetched_at, 0)
                .single()
                .map(|time| format!("最近更新 {}", time.format("%H:%M")))
                .unwrap_or_else(|| "已更新".to_string())
        } else {
            "读取现有 Codex 登录信息".to_string()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(colors.background)
            .text_color(colors.foreground)
            .border_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().child("Codex Usage"))
                            .when_some(plan, |this, plan| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(colors.secondary)
                                        .text_color(colors.secondary_foreground)
                                        .text_xs()
                                        .child(plan),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.error.is_some() {
                                colors.danger
                            } else {
                                colors.muted_foreground
                            })
                            .child(status_text),
                    ),
            )
            .child(usage_card(
                "five-hour-usage",
                "5 小时",
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.primary.as_ref()),
                colors,
            ))
            .child(usage_card(
                "weekly-usage",
                "每周",
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.secondary.as_ref()),
                colors,
            ))
            .child(
                div()
                    .mt_auto()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(footer),
            )
    }
}

fn usage_card(
    id: &'static str,
    label: &'static str,
    window: Option<&UsageWindow>,
    colors: ThemeColor,
) -> impl IntoElement {
    let used = window.map(|window| window.used_percent as f32).unwrap_or(0.0);
    let value = window
        .map(|window| {
            format!(
                "{:.0}% 已用  ·  {:.0}% 可用",
                window.used_percent,
                100.0 - window.used_percent
            )
        })
        .unwrap_or_else(|| "--".to_string());
    let reset = window
        .and_then(|window| window.reset_at)
        .and_then(|timestamp| Local.timestamp_opt(timestamp, 0).single())
        .map(|time| format!("重置 {}", time.format("%m-%d %H:%M")))
        .unwrap_or_else(|| "重置时间 --".to_string());

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(colors.secondary)
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_sm().child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.secondary_foreground)
                        .child(value),
                ),
        )
        .child(
            Progress::new(id)
                .value(used)
                .small()
                .color(colors.primary),
        )
        .child(
            div()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child(reset),
        )
}

pub fn run() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);

            let (sender, receiver) = async_channel::unbounded::<TrayCommand>();
            let view = cx.new(|_| UsageView::default());
            let view_for_window = view.clone();
            let bounds = Bounds::centered(
                None,
                size(px(tray::PANEL_WIDTH as f32), px(tray::PANEL_HEIGHT as f32)),
                cx,
            );

            let window_handle = match cx.open_window(
                WindowOptions {
                    show: false,
                    focus: true,
                    kind: WindowKind::PopUp,
                    titlebar: None,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| Root::new(view_for_window.clone(), window, cx)),
            ) {
                Ok(window) => window,
                Err(error) => {
                    tray::show_error(&format!("Cannot create usage popup: {error}"));
                    cx.quit();
                    return;
                }
            };

            let hwnd = match window_handle.update(cx, |_, window, _| raw_hwnd(window)) {
                Ok(Ok(hwnd)) => hwnd,
                Ok(Err(error)) => {
                    tray::show_error(&error);
                    cx.quit();
                    return;
                }
                Err(error) => {
                    tray::show_error(&format!("Cannot access usage popup: {error}"));
                    cx.quit();
                    return;
                }
            };

            if let Err(error) = tray::install(hwnd, sender.clone()) {
                tray::show_error(&error);
                cx.quit();
                return;
            }

            let hide_generation = Arc::new(AtomicU64::new(0));
            let command_view = view.downgrade();
            let command_sender = sender.clone();
            let command_hide_generation = hide_generation.clone();

            cx.spawn(async move |cx| {
                while let Ok(command) = receiver.recv().await {
                    match command {
                        TrayCommand::Show => {
                            tray::show_popup(hwnd);
                            start_refresh(command_view.clone(), cx);

                            let generation = command_hide_generation.fetch_add(1, Ordering::AcqRel) + 1;
                            let timer_generation = command_hide_generation.clone();
                            let hide_sender = command_sender.clone();
                            let executor = cx.background_executor().clone();
                            cx.spawn(async move |_| {
                                executor.timer(AUTO_HIDE_DELAY).await;
                                if timer_generation.load(Ordering::Acquire) == generation {
                                    let _ = hide_sender.send(TrayCommand::Hide).await;
                                }
                            })
                            .detach();
                        }
                        TrayCommand::Hide => {
                            command_hide_generation.fetch_add(1, Ordering::AcqRel);
                            tray::hide_popup(hwnd);
                        }
                        TrayCommand::Refresh => start_refresh(command_view.clone(), cx),
                        TrayCommand::Exit => {
                            tray::remove(hwnd);
                            let _ = cx.update(|cx| cx.quit());
                            break;
                        }
                    }
                }
            })
            .detach();

            let periodic_sender = sender.clone();
            let periodic_executor = cx.background_executor().clone();
            cx.spawn(async move |_| loop {
                periodic_executor.timer(REFRESH_INTERVAL).await;
                if periodic_sender.send(TrayCommand::Refresh).await.is_err() {
                    break;
                }
            })
            .detach();

            let _ = sender.try_send(TrayCommand::Refresh);
            cx.activate(true);
        });
}

fn start_refresh(view: WeakEntity<UsageView>, cx: &mut AsyncApp) {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }

    let _ = view.update(cx, |view, cx| {
        view.refreshing = true;
        cx.notify();
    });

    let task = cx.background_spawn(async { usage::fetch_usage() });
    cx.spawn(async move |cx| {
        let result = task.await;
        REFRESHING.store(false, Ordering::Release);
        let _ = view.update(cx, |view, cx| {
            view.apply_result(result);
            cx.notify();
        });
    })
    .detach();
}

fn raw_hwnd(window: &Window) -> Result<HWND, String> {
    let handle = window
        .window_handle()
        .map_err(|error| format!("Cannot obtain native window handle: {error}"))?;

    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
        _ => Err("The popup did not provide a Win32 HWND".to_string()),
    }
}

fn compact_error(error: &str) -> String {
    let error = error.replace(['\r', '\n'], " ");
    if error.chars().count() <= 68 {
        return error;
    }

    let mut compact = error.chars().take(65).collect::<String>();
    compact.push_str("...");
    compact
}
