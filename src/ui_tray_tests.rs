use super::*;

const TEST_ACCENT: COLORREF = COLORREF(0x00d47800);

#[test]
fn number_icons_use_native_dimensions_with_smooth_transparent_edges() {
    unsafe {
        for dpi in [96, 120, 144, 168, 192] {
            for value in [
                "0%", "1%", "6%", "11%", "25%", "50%", "81%", "88%", "93%", "99%", "100%", "?",
            ] {
                for dark in [false, true] {
                    let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi).clamp(16, 64);
                    let pixels = render_pixels(value, dark, size, TEST_ACCENT, false).unwrap();
                    assert_eq!(pixels.len(), (size * size * 4) as usize);
                    assert!(pixels.chunks_exact(4).any(|p| p[3] >= 128));
                    assert!(pixels.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .all(|p| p[..3].iter().all(|c| *c <= p[3]))
                    );
                    let padding = icon_padding(size);
                    for y in 0..size {
                        for x in 0..size {
                            if x < padding
                                || x >= size - padding
                                || y < padding
                                || y >= size - padding
                            {
                                assert_eq!(
                                    pixels[((y * size + x) * 4 + 3) as usize],
                                    0,
                                    "{value} must retain its small transparent margin at {size}px"
                                );
                            }
                        }
                    }
                    let icon = make_icon(value, dark, dpi, TEST_ACCENT, false);
                    assert!(!icon.is_invalid());
                    let mut info = ICONINFO::default();
                    GetIconInfo(icon, &mut info).unwrap();
                    let mut dimensions = BITMAP::default();
                    assert_ne!(
                        GetObjectW(
                            info.hbmColor.into(),
                            std::mem::size_of::<BITMAP>() as i32,
                            Some((&mut dimensions as *mut BITMAP).cast())
                        ),
                        0
                    );
                    assert_eq!((dimensions.bmWidth, dimensions.bmHeight), (size, size));
                    let _ = DeleteObject(info.hbmColor.into());
                    let _ = DeleteObject(info.hbmMask.into());
                    let _ = DestroyIcon(icon);
                    capture(&pixels, size, value, dark);
                }
            }
        }
    }
}

#[test]
fn all_allowance_values_remain_distinct_at_native_size() {
    for size in [16, 20, 24, 32] {
        let mut readings = std::collections::HashSet::new();
        for amount in 0..=100 {
            assert!(
                readings.insert(
                    render_pixels(&format!("{amount}%"), true, size, TEST_ACCENT, false).unwrap()
                ),
                "{amount}% must retain its own readable number at {size}px"
            );
        }
    }
}

#[test]
fn unavailable_marker_never_looks_like_zero_or_full_allowance() {
    for dark in [false, true] {
        for size in [16, 20, 24, 32] {
            let unknown = render_pixels("?", dark, size, TEST_ACCENT, false).unwrap();
            for value in ["0%", "100%"] {
                assert_ne!(
                    unknown,
                    render_pixels(value, dark, size, TEST_ACCENT, false).unwrap()
                );
            }
            for invalid in ["", "101%", "-1%", "invalid"] {
                assert_eq!(
                    render_pixels(invalid, dark, size, TEST_ACCENT, false).unwrap(),
                    unknown
                );
            }
        }
    }
}

#[test]
fn text_uses_the_theme_foreground_without_accent_colored_decoration() {
    for high_contrast in [false, true] {
        for dark in [false, true] {
            let foreground = ui_style::palette_for(dark, TEST_ACCENT, high_contrast).text;
            for value in ["0%", "93%", "100%", "?"] {
                let pixels = render_pixels(value, dark, 16, TEST_ACCENT, high_contrast).unwrap();
                assert_eq!(
                    Some(pixels.clone()),
                    render_pixels(value, dark, 16, COLORREF(0x000000e0), high_contrast)
                );
                for pixel in pixels.chunks_exact(4) {
                    for channel in 0..3 {
                        let color = (foreground.0 >> (channel * 8)) & 255;
                        assert_eq!(
                            pixel[2 - channel],
                            ((color * pixel[3] as u32 + 127) / 255) as u8
                        );
                    }
                }
            }
        }
    }
    assert_ne!(
        render_pixels("93%", false, 16, TEST_ACCENT, false),
        render_pixels("93%", true, 16, TEST_ACCENT, false)
    );
    for size in [0, 15, 65, i32::MAX] {
        assert!(render_pixels("93%", true, size, TEST_ACCENT, false).is_none());
    }
}

#[test]
fn centered_digits_fill_the_padded_rectangle() {
    for size in [16, 20, 24, 28, 32, 48, 64] {
        let available = size - icon_padding(size) * 2;
        for value in [
            "0", "1", "6", "8", "10", "11", "25", "81", "88", "93", "99", "100", "?",
        ] {
            let mask = text_mask(value, size).unwrap();
            assert!(
                mask.iter().any(|a| *a > 0 && *a < 255),
                "small digits must retain grayscale antialiasing"
            );
            let points: Vec<_> = mask
                .iter()
                .enumerate()
                .filter(|(_, a)| **a > 0)
                .map(|(i, _)| (i as i32 % size, i as i32 / size))
                .collect();
            let left = points.iter().map(|p| p.0).min().unwrap();
            let right = points.iter().map(|p| p.0).max().unwrap() + 1;
            let top = points.iter().map(|p| p.1).min().unwrap();
            let bottom = points.iter().map(|p| p.1).max().unwrap() + 1;
            assert!(
                (left + right - size).abs() <= 1 && (top + bottom - size).abs() <= 1,
                "{value} must be centered at {size}px"
            );
            assert!(
                left >= icon_padding(size)
                    && top >= icon_padding(size)
                    && right <= size - icon_padding(size)
                    && bottom <= size - icon_padding(size)
            );
            assert!(
                (right - left).max(bottom - top) >= available - 1,
                "{value} must fill at least one dimension of the available rectangle at {size}px"
            );
            let minimum_height = match value.len() {
                1 => available - 1,
                2 => available * 2 / 3,
                _ => (available * 3 / 7).max(7),
            };
            assert!(
                bottom - top >= minimum_height,
                "{value} is unnecessarily small at {size}px: {}",
                bottom - top
            );
        }
    }
}

fn capture(pixels: &[u8], size: i32, value: &str, dark: bool) {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".verification/number-only/icons");
    std::fs::create_dir_all(&directory).unwrap();
    let name = value.replace('%', "pct").replace('?', "unknown");
    let theme = if dark { "dark" } else { "light" };
    std::fs::write(
        directory.join(format!("{name}-{theme}-{size}.bgra")),
        pixels,
    )
    .unwrap();
}
