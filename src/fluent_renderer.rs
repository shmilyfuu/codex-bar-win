use windows::{
    core::PCWSTR,
    UI::ViewManagement::{UIColorType, UISettings},
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct2D::{
                Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U},
                D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES,
                D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
                D2D1_ROUNDED_RECT, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
            },
            DirectWrite::{
                DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_WORD_WRAPPING_NO_WRAP, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        },
    },
};

pub struct RenderRow<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub reset: &'a str,
    pub used_percent: Option<f64>,
}

pub struct RenderModel<'a> {
    pub status: &'a str,
    pub status_is_error: bool,
    pub primary: RenderRow<'a>,
    pub secondary: RenderRow<'a>,
    pub footer: &'a str,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub accent_rgb: (u8, u8, u8),
}

pub fn system_theme(fallback_dark: bool, fallback_accent: (u8, u8, u8)) -> Theme {
    let mut dark = fallback_dark;
    let mut accent_rgb = fallback_accent;

    if let Ok(settings) = UISettings::new() {
        if let Ok(background) = settings.GetColorValue(UIColorType::Background) {
            dark = u16::from(background.R) + u16::from(background.G) + u16::from(background.B) < 384;
        }

        // WinUI's AccentFillColorDefaultBrush resolves to SystemAccentColorLight2 in
        // dark mode and SystemAccentColorDark1 in light mode. Query those exact system
        // palette entries instead of approximating an accent shade ourselves.
        let accent_type = if dark {
            UIColorType::AccentLight2
        } else {
            UIColorType::AccentDark1
        };
        if let Ok(accent) = settings.GetColorValue(accent_type) {
            accent_rgb = (accent.R, accent.G, accent.B);
        }
    }

    Theme { dark, accent_rgb }
}

pub struct FluentRenderer {
    target: ID2D1HwndRenderTarget,
    body: IDWriteTextFormat,
    body_strong: IDWriteTextFormat,
    body_right: IDWriteTextFormat,
    caption: IDWriteTextFormat,
    caption_right: IDWriteTextFormat,
}

impl FluentRenderer {
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(|error| format!("Direct2D factory: {error}"))?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|error| format!("DirectWrite factory: {error}"))?;

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
                pixelSize: D2D_SIZE_U { width, height },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = d2d
                .CreateHwndRenderTarget(&properties, &hwnd_properties)
                .map_err(|error| format!("Direct2D render target: {error}"))?;

            // Windows 11 typography: Segoe UI Variable; body 14 Regular, body-strong
            // 14 Semibold, caption 12 Regular.
            let body = create_format(&dwrite, 14.0, false, false)?;
            let body_strong = create_format(&dwrite, 14.0, true, false)?;
            let body_right = create_format(&dwrite, 14.0, false, true)?;
            let caption = create_format(&dwrite, 12.0, false, false)?;
            let caption_right = create_format(&dwrite, 12.0, false, true)?;

            Ok(Self {
                target,
                body,
                body_strong,
                body_right,
                caption,
                caption_right,
            })
        }
    }

    pub fn draw(&self, model: &RenderModel<'_>, theme: Theme, width: f32, height: f32) -> Result<(), String> {
        let palette = Palette::for_theme(theme);

        unsafe {
            self.target.BeginDraw();
            // Solid WinUI surface: no Acrylic/Mica/backdrop blending.
            self.target.Clear(Some(&palette.surface_fill));

            let surface_stroke = self.brush(palette.surface_stroke)?;
            let primary = self.brush(palette.text_primary)?;
            let secondary = self.brush(palette.text_secondary)?;
            let tertiary = self.brush(palette.text_tertiary)?;
            let track = self.brush(palette.progress_track)?;
            let accent = self.brush(palette.accent)?;
            let danger = self.brush(palette.critical)?;

            // WinUI OverlayCornerRadius = 8 epx for transient flyout/overlay surfaces.
            let surface = D2D1_ROUNDED_RECT {
                rect: rect(0.5, 0.5, width - 0.5, height - 0.5),
                radiusX: 8.0,
                radiusY: 8.0,
            };
            self.target.DrawRoundedRectangle(&surface, &surface_stroke, 1.0, None);

            // Windows content gutter = 16 epx.
            draw_text(&self.target, "Codex Usage", &self.body_strong, &primary, rect(16.0, 12.0, 188.0, 34.0));
            draw_text(
                &self.target,
                model.status,
                &self.caption_right,
                if model.status_is_error { &danger } else { &secondary },
                rect(188.0, 12.0, width - 16.0, 34.0),
            );

            draw_row(
                &self.target,
                &self.body,
                &self.body_right,
                &self.caption,
                &primary,
                &secondary,
                &track,
                &accent,
                &model.primary,
                47.0,
                width,
            );
            draw_row(
                &self.target,
                &self.body,
                &self.body_right,
                &self.caption,
                &primary,
                &secondary,
                &track,
                &accent,
                &model.secondary,
                111.0,
                width,
            );

            draw_text(
                &self.target,
                model.footer,
                &self.caption,
                &tertiary,
                rect(16.0, height - 29.0, width - 16.0, height - 9.0),
            );

            self.target
                .EndDraw(None, None)
                .map_err(|error| format!("Direct2D draw: {error}"))?;
        }
        Ok(())
    }

    unsafe fn brush(&self, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush, String> {
        self.target
            .CreateSolidColorBrush(&color, None)
            .map_err(|error| format!("Direct2D brush: {error}"))
    }
}

unsafe fn create_format(factory: &IDWriteFactory, size: f32, strong: bool, right: bool) -> Result<IDWriteTextFormat, String> {
    let family = wide("Segoe UI Variable Text");
    let locale = wide("zh-CN");
    let format = factory
        .CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None::<&IDWriteFontCollection>,
            if strong { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(locale.as_ptr()),
        )
        .map_err(|error| format!("DirectWrite text format: {error}"))?;
    format
        .SetTextAlignment(if right { DWRITE_TEXT_ALIGNMENT_TRAILING } else { DWRITE_TEXT_ALIGNMENT_LEADING })
        .map_err(|error| format!("DirectWrite text alignment: {error}"))?;
    format
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .map_err(|error| format!("DirectWrite paragraph alignment: {error}"))?;
    format
        .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)
        .map_err(|error| format!("DirectWrite word wrapping: {error}"))?;
    Ok(format)
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_row(
    target: &ID2D1HwndRenderTarget,
    body: &IDWriteTextFormat,
    body_right: &IDWriteTextFormat,
    caption: &IDWriteTextFormat,
    primary: &ID2D1SolidColorBrush,
    secondary: &ID2D1SolidColorBrush,
    track: &ID2D1SolidColorBrush,
    accent: &ID2D1SolidColorBrush,
    row: &RenderRow<'_>,
    y: f32,
    width: f32,
) {
    draw_text(target, row.label, body, primary, rect(16.0, y, 150.0, y + 20.0));
    draw_text(target, row.value, body_right, primary, rect(150.0, y, width - 16.0, y + 20.0));

    // Current Microsoft.UI.Xaml ProgressBar resources:
    // MinHeight=3, TrackHeight=1, indicator radius=1.5, track radius=0.5.
    let left = 16.0;
    let right = width - 16.0;
    let progress_y = y + 27.0;
    let track_rect = D2D1_ROUNDED_RECT {
        rect: rect(left, progress_y + 1.0, right, progress_y + 2.0),
        radiusX: 0.5,
        radiusY: 0.5,
    };
    target.FillRoundedRectangle(&track_rect, track);

    if let Some(used) = row.used_percent {
        let used = used.clamp(0.0, 100.0) as f32;
        let fill_width = (right - left) * used / 100.0;
        if fill_width > 0.0 {
            let indicator = D2D1_ROUNDED_RECT {
                rect: rect(left, progress_y, left + fill_width.max(3.0).min(right - left), progress_y + 3.0),
                radiusX: 1.5,
                radiusY: 1.5,
            };
            target.FillRoundedRectangle(&indicator, accent);
        }
    }

    draw_text(target, row.reset, caption, secondary, rect(16.0, y + 36.0, width - 16.0, y + 54.0));
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

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
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
    text_primary: D2D1_COLOR_F,
    text_secondary: D2D1_COLOR_F,
    text_tertiary: D2D1_COLOR_F,
    surface_fill: D2D1_COLOR_F,
    surface_stroke: D2D1_COLOR_F,
    progress_track: D2D1_COLOR_F,
    accent: D2D1_COLOR_F,
    critical: D2D1_COLOR_F,
}

impl Palette {
    fn for_theme(theme: Theme) -> Self {
        let (r, g, b) = theme.accent_rgb;
        if theme.dark {
            Self {
                // WinUI 3 dark SolidBackgroundFillColorBase / application surface.
                text_primary: rgba(255, 255, 255, 255),
                text_secondary: rgba(255, 255, 255, 0xC5),
                text_tertiary: rgba(255, 255, 255, 0x87),
                surface_fill: rgba(32, 32, 32, 255),
                surface_stroke: rgba(0, 0, 0, 0x33),
                progress_track: rgba(255, 255, 255, 0x8B),
                accent: rgba(r, g, b, 255),
                critical: rgba(255, 153, 164, 255),
            }
        } else {
            Self {
                // WinUI 3 light SolidBackgroundFillColorBase / application surface.
                text_primary: rgba(0, 0, 0, 0xE4),
                text_secondary: rgba(0, 0, 0, 0x9E),
                text_tertiary: rgba(0, 0, 0, 0x72),
                surface_fill: rgba(243, 243, 243, 255),
                surface_stroke: rgba(0, 0, 0, 0x0F),
                progress_track: rgba(0, 0, 0, 0x72),
                accent: rgba(r, g, b, 255),
                critical: rgba(196, 43, 28, 255),
            }
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}