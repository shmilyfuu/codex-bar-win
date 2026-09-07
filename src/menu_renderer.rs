use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
                    D2D_RECT_F, D2D_SIZE_U,
                },
                D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS_NONE,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
                D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
                D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, ID2D1Factory,
                ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
            },
            DirectWrite::{
                DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteFontCollection,
                IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        },
    },
};

use crate::fluent_renderer::Theme;

pub const MENU_WIDTH: i32 = 220;
pub const MENU_HEIGHT: i32 = 318;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Refresh,
    Remaining,
    Used,
    Refresh5,
    Refresh30,
    Refresh60,
    Exit,
}

pub struct MenuModel {
    pub hovered: Option<MenuItem>,
    pub display_remaining: bool,
    pub refresh_minutes: u32,
}

pub fn hit_test(x: i32, y: i32) -> Option<MenuItem> {
    if !(0..MENU_WIDTH).contains(&x) || !(0..MENU_HEIGHT).contains(&y) {
        return None;
    }

    match y {
        8..40 => Some(MenuItem::Refresh),
        70..102 => Some(MenuItem::Remaining),
        102..134 => Some(MenuItem::Used),
        166..198 => Some(MenuItem::Refresh5),
        198..230 => Some(MenuItem::Refresh30),
        230..262 => Some(MenuItem::Refresh60),
        278..310 => Some(MenuItem::Exit),
        _ => None,
    }
}

pub struct MenuRenderer {
    target: ID2D1HwndRenderTarget,
    body: IDWriteTextFormat,
    body_strong: IDWriteTextFormat,
    caption: IDWriteTextFormat,
}

impl MenuRenderer {
    pub fn new(hwnd: isize) -> Result<Self, String> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(|error| format!("Menu Direct2D factory: {error}"))?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|error| format!("Menu DirectWrite factory: {error}"))?;

            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: HWND(hwnd as *mut core::ffi::c_void),
                pixelSize: D2D_SIZE_U {
                    width: MENU_WIDTH as u32,
                    height: MENU_HEIGHT as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = d2d
                .CreateHwndRenderTarget(&properties, &hwnd_properties)
                .map_err(|error| format!("Menu Direct2D render target: {error}"))?;

            Ok(Self {
                target,
                body: create_format(&dwrite, 14.0, false)?,
                body_strong: create_format(&dwrite, 14.0, true)?,
                caption: create_format(&dwrite, 12.0, false)?,
            })
        }
    }

    pub fn draw(&self, model: &MenuModel, theme: Theme) -> Result<(), String> {
        let palette = Palette::for_theme(theme);
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&palette.surface));

            let primary = self.brush(palette.primary)?;
            let secondary = self.brush(palette.secondary)?;
            let tertiary = self.brush(palette.tertiary)?;
            let accent = self.brush(palette.accent)?;
            let hover = self.brush(palette.hover)?;
            let stroke = self.brush(palette.stroke)?;

            let outline = D2D1_ROUNDED_RECT {
                rect: rect(0.5, 0.5, MENU_WIDTH as f32 - 0.5, MENU_HEIGHT as f32 - 0.5),
                radiusX: 8.0,
                radiusY: 8.0,
            };
            self.target.DrawRoundedRectangle(&outline, &stroke, 1.0, None);

            if let Some(item) = model.hovered {
                if let Some((top, bottom)) = row_bounds(item) {
                    let highlight = D2D1_ROUNDED_RECT {
                        rect: rect(4.0, top as f32, MENU_WIDTH as f32 - 4.0, bottom as f32),
                        radiusX: 4.0,
                        radiusY: 4.0,
                    };
                    self.target.FillRoundedRectangle(&highlight, &hover);
                }
            }

            draw_text(&self.target, "刷新", &self.body, &primary, rect(14.0, 8.0, 206.0, 40.0));
            fill_rect(&self.target, &stroke, 10.0, 45.0, 210.0, 46.0);

            draw_text(
                &self.target,
                "显示方式",
                &self.caption,
                &tertiary,
                rect(12.0, 48.0, 208.0, 68.0),
            );
            draw_radio_row(
                &self.target,
                &self.body,
                &primary,
                &secondary,
                &accent,
                "剩余额度",
                70.0,
                model.display_remaining,
            );
            draw_radio_row(
                &self.target,
                &self.body,
                &primary,
                &secondary,
                &accent,
                "已用额度",
                102.0,
                !model.display_remaining,
            );

            draw_text(
                &self.target,
                "自动刷新",
                &self.caption,
                &tertiary,
                rect(12.0, 144.0, 208.0, 164.0),
            );
            draw_radio_row(
                &self.target,
                &self.body,
                &primary,
                &secondary,
                &accent,
                "5 分钟",
                166.0,
                model.refresh_minutes == 5,
            );
            draw_radio_row(
                &self.target,
                &self.body,
                &primary,
                &secondary,
                &accent,
                "30 分钟",
                198.0,
                model.refresh_minutes == 30,
            );
            draw_radio_row(
                &self.target,
                &self.body,
                &primary,
                &secondary,
                &accent,
                "1 小时",
                230.0,
                model.refresh_minutes == 60,
            );

            fill_rect(&self.target, &stroke, 10.0, 270.0, 210.0, 271.0);
            draw_text(
                &self.target,
                "退出",
                &self.body_strong,
                &primary,
                rect(14.0, 278.0, 206.0, 310.0),
            );

            self.target
                .EndDraw(None, None)
                .map_err(|error| format!("Menu Direct2D draw: {error}"))?;
        }
        Ok(())
    }

    unsafe fn brush(&self, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush, String> {
        self.target
            .CreateSolidColorBrush(&color, None)
            .map_err(|error| format!("Menu Direct2D brush: {error}"))
    }
}

fn row_bounds(item: MenuItem) -> Option<(i32, i32)> {
    match item {
        MenuItem::Refresh => Some((8, 40)),
        MenuItem::Remaining => Some((70, 102)),
        MenuItem::Used => Some((102, 134)),
        MenuItem::Refresh5 => Some((166, 198)),
        MenuItem::Refresh30 => Some((198, 230)),
        MenuItem::Refresh60 => Some((230, 262)),
        MenuItem::Exit => Some((278, 310)),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_radio_row(
    target: &ID2D1HwndRenderTarget,
    body: &IDWriteTextFormat,
    primary: &ID2D1SolidColorBrush,
    secondary: &ID2D1SolidColorBrush,
    accent: &ID2D1SolidColorBrush,
    label: &str,
    top: f32,
    selected: bool,
) {
    draw_text(
        target,
        if selected { "●" } else { "○" },
        body,
        if selected { accent } else { secondary },
        rect(14.0, top, 34.0, top + 32.0),
    );
    draw_text(target, label, body, primary, rect(40.0, top, 206.0, top + 32.0));
}

unsafe fn fill_rect(
    target: &ID2D1HwndRenderTarget,
    brush: &ID2D1SolidColorBrush,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    target.FillRectangle(&rect(left, top, right, bottom), brush);
}

unsafe fn draw_text(
    target: &ID2D1HwndRenderTarget,
    text: &str,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
    layout: D2D_RECT_F,
) {
    let text = text.encode_utf16().collect::<Vec<_>>();
    target.DrawText(
        &text,
        format,
        &layout,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

unsafe fn create_format(
    factory: &IDWriteFactory,
    size: f32,
    strong: bool,
) -> Result<IDWriteTextFormat, String> {
    let family = wide("Segoe UI Variable Text");
    let locale = wide("zh-CN");
    let format = factory
        .CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None::<&IDWriteFontCollection>,
            if strong {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(locale.as_ptr()),
        )
        .map_err(|error| format!("Menu DirectWrite text format: {error}"))?;
    format
        .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)
        .map_err(|error| format!("Menu text alignment: {error}"))?;
    format
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .map_err(|error| format!("Menu paragraph alignment: {error}"))?;
    format
        .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)
        .map_err(|error| format!("Menu word wrapping: {error}"))?;
    Ok(format)
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

struct Palette {
    primary: D2D1_COLOR_F,
    secondary: D2D1_COLOR_F,
    tertiary: D2D1_COLOR_F,
    surface: D2D1_COLOR_F,
    stroke: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    accent: D2D1_COLOR_F,
}

impl Palette {
    fn for_theme(theme: Theme) -> Self {
        let (r, g, b) = theme.accent_rgb;
        if theme.dark {
            Self {
                primary: rgba(255, 255, 255, 255),
                secondary: rgba(255, 255, 255, 0xC5),
                tertiary: rgba(255, 255, 255, 0x87),
                surface: rgba(32, 32, 32, 255),
                stroke: rgba(0, 0, 0, 0x33),
                hover: rgba(255, 255, 255, 0x0F),
                accent: rgba(r, g, b, 255),
            }
        } else {
            Self {
                primary: rgba(0, 0, 0, 0xE4),
                secondary: rgba(0, 0, 0, 0x9E),
                tertiary: rgba(0, 0, 0, 0x72),
                surface: rgba(243, 243, 243, 255),
                stroke: rgba(0, 0, 0, 0x0F),
                hover: rgba(0, 0, 0, 0x09),
                accent: rgba(r, g, b, 255),
            }
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
