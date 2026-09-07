//! A colored circular allowance gauge, rebuilt only when usage or DPI changes.
//! The complete percentage keeps its natural proportions inside the ring.
use windows::{
    Win32::{
        Foundation::{COLORREF, SIZE},
        Graphics::Gdi::*,
        UI::{HiDpi::GetSystemMetricsForDpi, WindowsAndMessaging::*},
    },
    core::{BOOL, w},
};

pub(super) unsafe fn make_icon(value: &str, dark: bool, dpi: u32) -> HICON {
    unsafe {
        let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi.max(96)).clamp(16, 64);
        let Some(pixels) = render_pixels(value, dark, size) else {
            return HICON::default();
        };
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return HICON::default();
        }
        let info = bitmap_info(size, size);
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        let mask_pixels = vec![0u8; (((size + 15) / 16) * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_pixels.as_ptr().cast()));
        let icon = if mask.is_invalid() {
            HICON::default()
        } else {
            CreateIconIndirect(&ICONINFO {
                fIcon: BOOL(1),
                hbmMask: mask,
                hbmColor: bitmap,
                ..Default::default()
            })
            .unwrap_or_default()
        };
        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        icon
    }
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn parse_percent(value: &str) -> Option<u8> {
    value
        .strip_suffix('%')?
        .parse::<u8>()
        .ok()
        .filter(|v| *v <= 100)
}

// RGB colors match the allowance meter in the details panel.
fn gauge_color(percent: Option<u8>, dark: bool) -> [u8; 3] {
    match (percent, dark) {
        (None, true) => [155, 168, 185],
        (None, false) => [98, 114, 129],
        (Some(0..=20), true) => [255, 137, 148],
        (Some(0..=20), false) => [186, 55, 73],
        (Some(21..=50), true) => [246, 193, 104],
        (Some(21..=50), false) => [153, 99, 16],
        (Some(_), true) => [111, 231, 194],
        (Some(_), false) => [21, 129, 100],
    }
}

unsafe fn render_pixels(value: &str, dark: bool, size: i32) -> Option<Vec<u8>> {
    unsafe {
        let percent = parse_percent(value);
        let text = percent
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "?".into());
        let sample = 8;
        let source_height = size * sample;
        let source_width = source_height * 4;
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(
            Some(dc),
            &bitmap_info(source_width, source_height),
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        ) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous_bitmap = SelectObject(dc, bitmap.into());
        let font = CreateFontW(
            -source_height,
            0,
            0,
            0,
            700,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            NONANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        );
        let previous_font = SelectObject(dc, font.into());
        std::slice::from_raw_parts_mut(
            bits.cast::<u8>(),
            (source_width * source_height * 4) as usize,
        )
        .fill(0);
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(0x00ffffff));
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut extent = SIZE::default();
        let measured = GetTextExtentPoint32W(dc, &utf16, &mut extent).as_bool();
        let prefix_width = percent.and_then(|v| {
            let digits: Vec<u16> = v.to_string().encode_utf16().collect();
            let mut prefix = SIZE::default();
            GetTextExtentPoint32W(dc, &digits, &mut prefix)
                .as_bool()
                .then_some(prefix.cx)
        });
        let drawn = TextOutW(dc, 0, 0, &utf16).as_bool();
        let _ = GdiFlush();
        let source = std::slice::from_raw_parts(
            bits.cast::<u8>(),
            (source_width * source_height * 4) as usize,
        );
        let pixels = if measured && drawn {
            Some(compose_gauge(
                source,
                (source_width, source_height),
                extent.cx,
                prefix_width,
                percent,
                dark,
                size,
            ))
        } else {
            None
        };
        SelectObject(dc, previous_font);
        SelectObject(dc, previous_bitmap);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        pixels
    }
}

fn compose_gauge(
    source: &[u8],
    source_size: (i32, i32),
    text_width: i32,
    prefix_width: Option<i32>,
    percent: Option<u8>,
    dark: bool,
    size: i32,
) -> Vec<u8> {
    let (width, height) = source_size;
    let bounds = |begin: i32, end: i32| {
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for y in 0..height {
            for x in begin..end.min(width) {
                if source[((y * width + x) * 4) as usize] != 0 {
                    left = left.min(x);
                    right = right.max(x + 1);
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        (left, top, right, bottom)
    };
    let digits = bounds(0, prefix_width.unwrap_or(text_width));
    let suffix = prefix_width.map(|split| bounds(split, text_width));
    let mut output = vec![0u8; (size * size * 4) as usize];
    let size_f = size as f64;
    let center = size_f / 2.0;
    let outer_radius = center - 0.25;
    let stroke = (size_f * 0.085).max(1.25);
    let ring_radius = outer_radius - stroke / 2.0;
    let inner_radius = outer_radius - stroke;
    let foreground = if dark {
        [245u8, 249, 252]
    } else {
        [22u8, 33, 43]
    };
    let background = if dark {
        [22u8, 27, 34]
    } else {
        [246u8, 248, 251]
    };
    let accent = gauge_color(percent, dark);
    let track = std::array::from_fn::<_, 3, _>(|c| {
        (background[c] as f64 * 0.55 + accent[c] as f64 * 0.45).round() as u8
    });
    let sweep = std::f64::consts::TAU * percent.unwrap_or(0) as f64 / 100.0;
    let end = (
        center + ring_radius * sweep.sin(),
        center - ring_radius * sweep.cos(),
    );
    // The smaller percent unit leaves room for readable digits. Each glyph
    // group keeps its original aspect ratio; both align on the same baseline.
    let available_width = (inner_radius * 2.0 - size_f * 0.07).max(1.0);
    let nominal_height = (inner_radius * 1.12).max(1.0);
    let ratio = |b: (i32, i32, i32, i32)| (b.2 - b.0).max(1) as f64 / (b.3 - b.1).max(1) as f64;
    let digits_width = nominal_height * ratio(digits);
    let suffix_height = nominal_height * 0.62;
    let suffix_width = suffix.map(|b| suffix_height * ratio(b)).unwrap_or(0.0);
    let gap = if suffix.is_some() {
        size_f * 0.022
    } else {
        0.0
    };
    let fit = (available_width / (digits_width + gap + suffix_width)).min(1.0);
    let label_left = center - (digits_width + gap + suffix_width) * fit / 2.0;
    let label_top = center - nominal_height * fit / 2.0;
    let mut glyphs = vec![(
        digits,
        label_left,
        label_top,
        nominal_height * fit / (digits.3 - digits.1).max(1) as f64,
    )];
    if let Some(b) = suffix {
        glyphs.push((
            b,
            label_left + (digits_width + gap) * fit,
            label_top + (nominal_height - suffix_height) * fit,
            suffix_height * fit / (b.3 - b.1).max(1) as f64,
        ));
    }
    const SAMPLE: i32 = 8;
    for y in 0..size {
        for x in 0..size {
            let mut coverage = 0u32;
            let mut channels = [0u32; 3];
            let mut glyph_coverage = 0u32;
            for sy in 0..SAMPLE {
                for sx in 0..SAMPLE {
                    let xx = x as f64 + (sx as f64 + 0.5) / SAMPLE as f64;
                    let yy = y as f64 + (sy as f64 + 0.5) / SAMPLE as f64;
                    let dx = xx - center;
                    let dy = yy - center;
                    let radius = dx.hypot(dy);
                    if radius > outer_radius {
                        continue;
                    }
                    coverage += 1;
                    let angle = dx.atan2(-dy).rem_euclid(std::f64::consts::TAU);
                    let cap = stroke / 2.0;
                    let start_cap = dx.hypot(yy - (center - ring_radius)) <= cap;
                    let end_cap = (xx - end.0).hypot(yy - end.1) <= cap;
                    let active = percent.is_some_and(|p| p > 0)
                        && ((radius >= inner_radius && angle <= sweep) || start_cap || end_cap);
                    let ink = if active {
                        accent
                    } else if radius >= inner_radius {
                        track
                    } else {
                        background
                    };
                    for c in 0..3 {
                        channels[c] += ink[c] as u32;
                    }
                    for &((left, top, right, bottom), glyph_left, glyph_top, glyph_scale) in &glyphs
                    {
                        let label_width = (right - left) as f64 * glyph_scale;
                        let label_height = (bottom - top) as f64 * glyph_scale;
                        if left < right
                            && top < bottom
                            && xx >= glyph_left
                            && yy >= glyph_top
                            && xx < glyph_left + label_width
                            && yy < glyph_top + label_height
                        {
                            let source_x = left + ((xx - glyph_left) / glyph_scale) as i32;
                            let source_y = top + ((yy - glyph_top) / glyph_scale) as i32;
                            if source[((source_y * width + source_x) * 4) as usize] != 0 {
                                glyph_coverage += 1;
                            }
                        }
                    }
                }
            }
            let samples = (SAMPLE * SAMPLE) as f64;
            let opacity = coverage as f64 / samples;
            let text_alpha = (glyph_coverage as f64 / samples).powf(0.82).min(opacity);
            let pixel = &mut output[((y * size + x) * 4) as usize..][..4];
            for c in 0..3 {
                let base = channels[c] as f64 / samples;
                let rgb = base * (1.0 - text_alpha) + foreground[c] as f64 * text_alpha;
                pixel[2 - c] = rgb.round().min(opacity * 255.0) as u8;
            }
            pixel[3] = (opacity * 255.0).round() as u8;
        }
    }
    output
}

#[cfg(test)]
#[path = "ui_tray_tests.rs"]
mod tests;
