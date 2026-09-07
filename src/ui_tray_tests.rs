use super::*;

#[test]
fn gauge_icons_are_native_sized_and_have_smooth_transparency() {
    unsafe {
        for dpi in [96, 120, 144, 192] {
            for value in ["0%", "14%", "35%", "75%", "100%", "?"] {
                for dark in [false, true] {
                    let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi).clamp(16, 64);
                    let pixels = render_pixels(value, dark, size).unwrap();
                    assert_eq!(pixels.len(), (size * size * 4) as usize);
                    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 255));
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .any(|pixel| pixel[3] > 0 && pixel[3] < 255)
                    );
                    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 0));
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .all(|pixel| { pixel[..3].iter().all(|channel| *channel <= pixel[3]) })
                    );
                    assert_circular_frame(&pixels, size, value);
                    if let Ok(percent @ 1..=100) = value.trim_end_matches('%').parse::<u8>() {
                        assert_progress_color(&pixels, size, percent, dark);
                    }

                    let icon = make_icon(value, dark, dpi);
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
fn remaining_allowance_uses_consistent_threshold_colors() {
    for dark in [false, true] {
        let red = gauge_color(Some(0), dark);
        let amber = gauge_color(Some(21), dark);
        let green = gauge_color(Some(51), dark);
        let unknown = gauge_color(None, dark);
        assert_eq!(gauge_color(Some(20), dark), red);
        assert_eq!(gauge_color(Some(50), dark), amber);
        assert_eq!(gauge_color(Some(100), dark), green);
        assert!(
            red[0] > red[1] && red[0] > red[2],
            "low allowance should be red"
        );
        assert!(
            amber[0] > amber[2] && amber[1] > amber[2],
            "medium allowance should be amber"
        );
        assert!(
            green[1] > green[0] && green[1] > green[2],
            "high allowance should be green"
        );
        assert_ne!(unknown, red);
        assert_ne!(unknown, amber);
        assert_ne!(unknown, green);
    }
}

#[test]
fn unknown_allowance_is_visually_distinct_from_zero() {
    unsafe {
        for dark in [false, true] {
            for size in [16, 20, 24, 32] {
                let unknown = render_pixels("?", dark, size).unwrap();
                let exhausted = render_pixels("0%", dark, size).unwrap();
                assert_ne!(unknown, exhausted);
            }
        }
    }
}

fn assert_circular_frame(pixels: &[u8], size: i32, value: &str) {
    // A complete frame remains visible in all quadrants even when the progress
    // arc is short. Transparent corners keep the icon circular on any taskbar.
    for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
        assert_eq!(pixels[((y * size + x) * 4 + 3) as usize], 0);
    }
    let center = size as f64 / 2.0;
    let mut quadrants = [0usize; 4];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - center;
            let dy = y as f64 + 0.5 - center;
            let radius = (dx * dx + dy * dy).sqrt();
            if radius >= size as f64 * 0.35
                && radius <= size as f64 * 0.5
                && pixels[((y * size + x) * 4 + 3) as usize] > 32
            {
                quadrants[usize::from(dx >= 0.0) + 2 * usize::from(dy >= 0.0)] += 1;
            }
        }
    }
    assert!(
        quadrants.into_iter().all(|count| count > 0),
        "missing circular frame for {value} at {size}px"
    );
}

fn capture(pixels: &[u8], size: i32, value: &str, dark: bool) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".verification/tray");
    std::fs::create_dir_all(&directory).unwrap();
    let name = value.replace('%', "pct").replace('?', "unknown");
    let theme = if dark { "dark" } else { "light" };
    std::fs::write(
        directory.join(format!("{name}-{theme}-{size}.bgra")),
        pixels,
    )
    .unwrap();
}

fn assert_progress_color(pixels: &[u8], size: i32, percent: u8, dark: bool) {
    let expected = gauge_color(Some(percent), dark);
    let center = size as f64 / 2.0;
    let mut colored_pixels = 0;
    for y in 0..size {
        for x in 0..size {
            let radius = (x as f64 + 0.5 - center).hypot(y as f64 + 0.5 - center);
            let pixel = &pixels[((y * size + x) * 4) as usize..][..4];
            // Inspect the perimeter, excluding the label. Correct premultiplied
            // BGRA must recover the expected RGB accent on the progress arc.
            if radius >= size as f64 * 0.35 && pixel[3] >= 64 {
                let matches = (0..3).all(|channel| {
                    let actual = pixel[2 - channel] as f64 * 255.0 / pixel[3] as f64;
                    (actual - expected[channel] as f64).abs() <= 3.0
                });
                colored_pixels += usize::from(matches);
            }
        }
    }
    assert!(
        colored_pixels > 0,
        "missing allowance-colored progress arc for {percent}% at {size}px"
    );
}
