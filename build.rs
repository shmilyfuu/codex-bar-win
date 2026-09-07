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
    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    let scale = size as f32;
    let inset = scale * 0.055;
    let radius = scale * 0.245;

    for y in 0..size {
        for x in 0..size {
            let mut background_samples = 0_u32;
            let mut glyph_samples = 0_u32;

            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    if rounded_rect(px, py, inset, inset, scale - inset, scale - inset, radius) {
                        background_samples += 1;
                    }
                    if meter_glyph(px, py, scale) {
                        glyph_samples += 1;
                    }
                }
            }

            let index = ((y * size + x) * 4) as usize;
            let bg_alpha = (background_samples * 255 / 16) as u8;
            let glyph_alpha = (glyph_samples * 255 / 16) as u8;

            if glyph_alpha > 0 {
                rgba[index] = 245;
                rgba[index + 1] = 247;
                rgba[index + 2] = 250;
                rgba[index + 3] = glyph_alpha.max(bg_alpha);
            } else if bg_alpha > 0 {
                rgba[index] = 20;
                rgba[index + 1] = 23;
                rgba[index + 2] = 28;
                rgba[index + 3] = bg_alpha;
            }
        }
    }

    rgba
}

fn meter_glyph(x: f32, y: f32, size: f32) -> bool {
    let width = size * 0.105;
    let bottom = size * 0.72;
    let bars = [
        (size * 0.315, size * 0.455),
        (size * 0.500, size * 0.350),
        (size * 0.685, size * 0.245),
    ];

    bars.into_iter().any(|(center_x, top)| {
        let left = center_x - width / 2.0;
        let right = center_x + width / 2.0;
        rounded_rect(x, y, left, top, right, bottom, width / 2.0)
    })
}

fn rounded_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }

    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
