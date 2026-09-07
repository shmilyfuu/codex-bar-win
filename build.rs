use std::{env, fs::File, path::PathBuf};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let icon_path = out_dir.join("codex-bar-win.ico");

    let mut icon = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16_u32, 20, 24, 32, 48, 64, 128, 256] {
        let image = ico::IconImage::from_rgba_data(size, size, render_icon(size));
        icon.add_entry(ico::IconDirEntry::encode(&image).expect("encode application icon"));
    }

    let file = File::create(&icon_path).expect("create generated application icon");
    icon.write(file).expect("write generated application icon");

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon_path.to_string_lossy().as_ref())
        .set("ProductName", "Codex Usage")
        .set("FileDescription", "Codex usage tray viewer")
        .set("OriginalFilename", "codex-bar-win.exe");
    resource.compile().expect("compile Windows resources");
}

fn render_icon(size: u32) -> Vec<u8> {
    // Small tray icons expose aliasing very easily. Render coverage at 8x8 samples per
    // output pixel, then alpha-composite the white meter over the dark tile. The previous
    // renderer switched any partially-covered glyph pixel straight to white, which made
    // diagonal/rounded edges visibly jagged at 16-24 px.
    const SAMPLES: u32 = 8;
    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    let scale = size as f32;
    let inset = scale * 0.0625;
    let radius = scale * 0.23;

    for y in 0..size {
        for x in 0..size {
            let mut background_samples = 0_u32;
            let mut glyph_samples = 0_u32;

            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                    if rounded_rect(px, py, inset, inset, scale - inset, scale - inset, radius) {
                        background_samples += 1;
                    }
                    if meter_glyph(px, py, scale) {
                        glyph_samples += 1;
                    }
                }
            }

            let total = (SAMPLES * SAMPLES) as f32;
            let bg = background_samples as f32 / total;
            let fg = glyph_samples as f32 / total;
            let index = ((y * size + x) * 4) as usize;

            // Straight-alpha composite: foreground over the rounded tile.
            let out_a = fg + bg * (1.0 - fg);
            if out_a <= f32::EPSILON {
                continue;
            }

            let bg_rgb = [20.0_f32, 23.0, 28.0];
            let fg_rgb = [245.0_f32, 247.0, 250.0];
            for channel in 0..3 {
                let premul = fg_rgb[channel] * fg + bg_rgb[channel] * bg * (1.0 - fg);
                rgba[index + channel] = (premul / out_a).round().clamp(0.0, 255.0) as u8;
            }
            rgba[index + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    rgba
}

fn meter_glyph(x: f32, y: f32, size: f32) -> bool {
    // Geometry is intentionally chunky enough to remain legible in the 16 px tray slot.
    let width = size * 0.125;
    let bottom = size * 0.735;
    let bars = [
        (size * 0.315, size * 0.485),
        (size * 0.500, size * 0.365),
        (size * 0.685, size * 0.245),
    ];

    bars.into_iter().any(|(center_x, top)| {
        let left = center_x - width / 2.0;
        let right = center_x + width / 2.0;
        rounded_rect(x, y, left, top, right, bottom, width * 0.42)
    })
}

fn rounded_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }

    let radius = radius
        .max(0.0)
        .min((right - left).max(0.0) / 2.0)
        .min((bottom - top).max(0.0) / 2.0);
    let inner_left = left + radius;
    let inner_right = right - radius;
    let inner_top = top + radius;
    let inner_bottom = bottom - radius;

    let cx = if inner_left <= inner_right {
        x.clamp(inner_left, inner_right)
    } else {
        (left + right) * 0.5
    };
    let cy = if inner_top <= inner_bottom {
        y.clamp(inner_top, inner_bottom)
    } else {
        (top + bottom) * 0.5
    };

    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
