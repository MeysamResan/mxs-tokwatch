use super::*;

#[test]
fn text_icons_are_native_sized_and_have_smooth_transparency() {
    unsafe {
        for dpi in [96, 120, 144, 192] {
            for value in ["0%", "9%", "10%", "14%", "35%", "75%", "99%", "100%", "?"] {
                for dark in [false, true] {
                    let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi).clamp(16, 64);
                    let pixels = render_pixels(value, dark, size).unwrap();
                    assert_eq!(pixels.len(), (size * size * 4) as usize);
                    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] >= 128));
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .any(|pixel| pixel[3] > 0 && pixel[3] < 255)
                    );
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .all(|pixel| { pixel[..3].iter().all(|channel| *channel <= pixel[3]) })
                    );
                    assert_text_only(&pixels, size, value);
                    assert_text_color(&pixels, parse_percent(value), dark);

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
fn remaining_allowance_uses_distinct_red_and_neutral_threshold_colors() {
    for dark in [false, true] {
        let low = allowance_color(Some(0), dark);
        let medium = allowance_color(Some(21), dark);
        let high = allowance_color(Some(51), dark);
        let unknown = allowance_color(None, dark);
        assert_eq!(allowance_color(Some(20), dark), low);
        assert_eq!(allowance_color(Some(50), dark), medium);
        assert_eq!(allowance_color(Some(100), dark), high);
        for warning in [low, medium] {
            assert!(warning[0] > warning[1] && warning[0] > warning[2]);
        }
        assert_eq!(high[0], high[1]);
        for (index, color) in [low, medium, high, unknown].iter().enumerate() {
            for other in [low, medium, high, unknown].iter().skip(index + 1) {
                assert_ne!(color, other);
            }
        }
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
                for invalid in ["", "101%", "-1%", "invalid"] {
                    assert_eq!(render_pixels(invalid, dark, size).unwrap(), unknown);
                }
            }
        }
    }
}

fn assert_text_only(pixels: &[u8], size: i32, value: &str) {
    // An empty perimeter and transparent canvas catch the previous circular
    // frame and filled disk. Normal text must remain visible without stretching its proportions.
    let mut transparent = 0;
    let (mut left, mut top, mut right, mut bottom) = (size, size, 0, 0);
    for y in 0..size {
        for x in 0..size {
            let alpha = pixels[((y * size + x) * 4 + 3) as usize];
            if y == 0 || y == size - 1 {
                assert_eq!(alpha, 0, "non-text perimeter for {value} at {size}px");
            }
            transparent += i32::from(alpha == 0);
            if alpha >= 32 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    assert!(
        transparent > size * size / 4,
        "filled background for {value}"
    );
    assert!(right - left >= size / 3, "text too narrow for {value}");
    assert!(bottom - top >= size / 4, "text too small for {value}");
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

fn assert_text_color(pixels: &[u8], percent: Option<u8>, dark: bool) {
    let expected = allowance_color(percent, dark);
    for pixel in pixels.chunks_exact(4).filter(|pixel| pixel[3] >= 64) {
        // Every visible glyph uses the allowance color; no white label or
        // opaque background survives underneath the antialiased text.
        for channel in 0..3 {
            let actual = pixel[2 - channel] as f64 * 255.0 / pixel[3] as f64;
            assert!((actual - expected[channel] as f64).abs() <= 3.0);
        }
    }
}

#[test]
fn percent_sign_stays_as_tall_as_the_number() {
    unsafe {
        for size in [16, 20, 24, 32] {
            let pixels = render_pixels("9%", true, size).unwrap();
            let points: Vec<(i32, i32)> = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .filter(|(x, y)| pixels[((y * size + x) * 4 + 3) as usize] >= 32)
                .collect();
            let width = points.iter().map(|p| p.0).max().unwrap()
                - points.iter().map(|p| p.0).min().unwrap()
                + 1;
            let height = points.iter().map(|p| p.1).max().unwrap()
                - points.iter().map(|p| p.1).min().unwrap()
                + 1;
            assert!(
                height >= (size * 2 / 3).max(12),
                "9% must stay large at {size}px"
            );
            assert!(
                width * 10 >= height * 13,
                "9% was unnaturally condensed at {size}px"
            );
            for (left, right) in [(1, size / 3), (size * 2 / 3, size - 1)] {
                let occupied: Vec<i32> = (0..size)
                    .filter(|y| {
                        (left..right).any(|x| pixels[((y * size + x) * 4 + 3) as usize] >= 32)
                    })
                    .collect();
                let height = occupied.last().unwrap() - occupied.first().unwrap() + 1;
                assert!(height >= size / 3, "number or percent shrank at {size}px");
            }
        }
    }
}
