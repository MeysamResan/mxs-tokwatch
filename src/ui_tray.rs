//! A large native-sized allowance number, rebuilt only when its state changes.
use super::ui_style;
use windows::{
    Win32::{
        Foundation::{COLORREF, SIZE},
        Graphics::Gdi::*,
        UI::{HiDpi::GetSystemMetricsForDpi, WindowsAndMessaging::*},
    },
    core::{BOOL, w},
};

pub(super) unsafe fn make_icon(
    value: &str,
    dark: bool,
    dpi: u32,
    accent: COLORREF,
    high_contrast: bool,
) -> HICON {
    unsafe {
        let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi.max(96)).clamp(16, 64);
        let Some(pixels) = render_pixels(value, dark, size, accent, high_contrast) else {
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

// Draw only the reading, using the full rectangular icon area. The grayscale
// mask keeps smooth text edges without a background, ring, or percent sign.
fn render_pixels(
    value: &str,
    dark: bool,
    size: i32,
    accent: COLORREF,
    high_contrast: bool,
) -> Option<Vec<u8>> {
    if !(16..=64).contains(&size) {
        return None;
    }
    let text = parse_percent(value).map_or_else(|| "?".into(), |percent| percent.to_string());
    let foreground = ui_style::palette_for(dark, accent, high_contrast).text;
    let mask = text_mask(&text, size)?;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for (pixel, coverage) in pixels.chunks_exact_mut(4).zip(mask) {
        // HICON requires premultiplied BGRA, including fully transparent pixels.
        for channel in 0..3 {
            let color = (foreground.0 >> (channel * 8)) & 255;
            pixel[2 - channel] = ((color * coverage as u32 + 127) / 255) as u8;
        }
        pixel[3] = coverage;
    }
    Some(pixels)
}

fn icon_padding(size: i32) -> i32 {
    // One physical pixel at 100% scaling, increasing with the native icon size.
    ((size + 8) / 16).max(1)
}

fn text_mask(text: &str, size: i32) -> Option<Vec<u8>> {
    // GDI drops antialiasing for some tiny hinted fonts. Rasterizing the font at
    // four times the target resolution also lets fitting advance in quarter
    // pixels, instead of losing an entire row whenever the next font is too big.
    const SCALE: i32 = 4;
    let source_size = size * SCALE;
    let source = text_mask_at_scale(text, source_size, icon_padding(size) * SCALE)?;
    let mut mask = vec![0u8; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let mut coverage = 0u32;
            for sy in 0..SCALE {
                for sx in 0..SCALE {
                    coverage +=
                        source[((y * SCALE + sy) * source_size + x * SCALE + sx) as usize] as u32;
                }
            }
            mask[(y * size + x) as usize] = ((coverage + 8) / 16) as u8;
        }
    }
    Some(mask)
}

fn text_mask_at_scale(text: &str, size: i32, padding: i32) -> Option<Vec<u8>> {
    unsafe {
        let text: Vec<u16> = text.encode_utf16().collect();
        let canvas = size * 2;
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(
            Some(dc),
            &bitmap_info(canvas, canvas),
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        ) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous_bitmap = SelectObject(dc, bitmap.into());
        let source =
            std::slice::from_raw_parts_mut(bits.cast::<u8>(), (canvas * canvas * 4) as usize);
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(0x00ffffff));
        // Keep the font's natural spacing so adjacent rounded digits (88/100)
        // retain a visible gap after downsampling to tiny tray dimensions.
        let mut result = None;
        // Fit the visible glyphs to the padded rectangle, including 100 and ?.
        // Font line boxes have extra leading that should not shrink the digits.
        // Segoe UI digits are at least half an em wide. Start with a bound
        // derived from the string width instead of probing oversized fonts.
        let max_height = (size * 2 / text.len() as i32 + padding * 2).min(size * 2);
        for height in (4..=max_height).rev() {
            let font = CreateFontW(
                -height,
                0,
                0,
                0,
                600,
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
            if font.is_invalid() {
                continue;
            }
            let previous_font = SelectObject(dc, font.into());
            let mut extent = SIZE::default();
            let measured = GetTextExtentPoint32W(dc, &text, &mut extent).as_bool();
            source.fill(0);
            let mut ink = (canvas, canvas, 0, 0);
            if measured
                && extent.cx <= canvas
                && extent.cy <= canvas
                && TextOutW(dc, 0, 0, &text).as_bool()
            {
                let _ = GdiFlush();
                for y in 0..extent.cy {
                    for x in 0..extent.cx {
                        if source[((y * canvas + x) * 4) as usize] != 0 {
                            ink.0 = ink.0.min(x);
                            ink.1 = ink.1.min(y);
                            ink.2 = ink.2.max(x + 1);
                            ink.3 = ink.3.max(y + 1);
                        }
                    }
                }
            }
            let (width, height) = (ink.2 - ink.0, ink.3 - ink.1);
            let available = size - padding * 2;
            if width > 0 && height > 0 && width <= available && height <= available {
                let left = (size - width) / 2;
                let top = (size - height) / 2;
                let mut mask = vec![0u8; (size * size) as usize];
                for y in 0..height {
                    for x in 0..width {
                        mask[((top + y) * size + left + x) as usize] =
                            source[(((y + ink.1) * canvas + x + ink.0) * 4) as usize];
                    }
                }
                result = Some(mask);
            }
            SelectObject(dc, previous_font);
            let _ = DeleteObject(font.into());
            if result.is_some() {
                break;
            }
        }
        SelectObject(dc, previous_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        result
    }
}

#[cfg(test)]
#[path = "ui_tray_tests.rs"]
mod tests;
