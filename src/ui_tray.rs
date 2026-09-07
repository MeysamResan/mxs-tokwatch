//! A colored percentage on a transparent background, rebuilt only on usage or DPI changes.
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

// Keep the red-and-charcoal theme readable on either taskbar background.
fn allowance_color(percent: Option<u8>, dark: bool) -> [u8; 3] {
    match (percent, dark) {
        (None, true) => [155, 155, 165],
        (None, false) => [98, 98, 108],
        (Some(0..=20), true) => [255, 60, 70],
        (Some(0..=20), false) => [175, 0, 15],
        (Some(21..=50), true) => [235, 120, 125],
        (Some(21..=50), false) => [125, 0, 0],
        (Some(_), true) => [235, 235, 237],
        (Some(_), false) => [42, 42, 46],
    }
}

unsafe fn render_pixels(value: &str, dark: bool, size: i32) -> Option<Vec<u8>> {
    unsafe {
        let final_size = size;
        const SAMPLE: i32 = 4;
        let size = size * SAMPLE;
        let canvas = size * 3;
        let percent = parse_percent(value);
        let text = percent
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "?".into());
        let utf16: Vec<u16> = text.encode_utf16().collect();
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
        source.fill(0);
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(0x00ffffff));
        let mut output = None;
        // Fit a normal font without changing its proportions. Supersampling the
        // complete text run smooths edges without stretching individual glyphs.
        for height in (1..=size * 2).rev() {
            let font = CreateFontW(
                -height,
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
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Arial Narrow"),
            );
            if font.is_invalid() {
                continue;
            }
            let previous_font = SelectObject(dc, font.into());
            let mut extent = SIZE::default();
            let measured = GetTextExtentPoint32W(dc, &utf16, &mut extent).as_bool();
            // Font line boxes include invisible leading and side bearings.
            // Fit the visible ink instead so that space does not shrink the label.
            let candidate = measured && extent.cx <= size + 3 * SAMPLE && extent.cy <= canvas;
            source.fill(0);
            let mut ink = (canvas, canvas, 0, 0);
            if candidate && TextOutW(dc, 0, 0, &utf16).as_bool() {
                let _ = GdiFlush();
                for y in 0..extent.cy.min(canvas) {
                    for x in 0..extent.cx.min(canvas) {
                        if source[((y * canvas + x) * 4) as usize] != 0 {
                            ink.0 = ink.0.min(x);
                            ink.1 = ink.1.min(y);
                            ink.2 = ink.2.max(x + 1);
                            ink.3 = ink.3.max(y + 1);
                        }
                    }
                }
            }
            let ink_width = ink.2 - ink.0;
            let ink_height = ink.3 - ink.1;
            if ink_width > 0
                && ink_height > 0
                && ink_width <= size
                && ink_height <= size - 2 * SAMPLE
            {
                let offset_x = ink.0 - (size - ink_width) / 2;
                let offset_y = ink.1 - (size - ink_height) / 2;
                let accent = allowance_color(percent, dark);
                let mut pixels = vec![0u8; (final_size * final_size * 4) as usize];
                for y in 0..final_size {
                    for x in 0..final_size {
                        let mut coverage = 0u32;
                        for dy in 0..SAMPLE {
                            for dx in 0..SAMPLE {
                                let source_x = x * SAMPLE + dx + offset_x;
                                let source_y = y * SAMPLE + dy + offset_y;
                                if source_x >= 0
                                    && source_y >= 0
                                    && source_x < canvas
                                    && source_y < canvas
                                {
                                    coverage += source
                                        [((source_y * canvas + source_x) * 4) as usize]
                                        as u32;
                                }
                            }
                        }
                        let alpha = ((coverage + 8) / 16) as u8;
                        let pixel = &mut pixels[((y * final_size + x) * 4) as usize..][..4];
                        for channel in 0..3 {
                            pixel[2 - channel] =
                                ((accent[channel] as u16 * alpha as u16 + 127) / 255) as u8;
                        }
                        pixel[3] = alpha;
                    }
                }
                output = Some(pixels);
            }
            SelectObject(dc, previous_font);
            let _ = DeleteObject(font.into());
            if output.is_some() {
                break;
            }
        }
        SelectObject(dc, previous_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        output
    }
}

#[cfg(test)]
#[path = "ui_tray_tests.rs"]
mod tests;
