// Native UI regression checks create only their own test windows. No account calls.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Fixture {
    Normal,
    Missing,
    Long,
}

impl Fixture {
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Missing => "missing",
            Self::Long => "long",
        }
    }

    fn snapshot(self) -> UsageSnapshot {
        match self {
            Self::Normal => demo_snapshot(),
            Self::Missing => UsageSnapshot::from_response(serde_json::json!({
                "rateLimits": {"primary": {"windowDurationMins": 10080}}
            }))
            .unwrap(),
            Self::Long => {
                let mut snapshot = demo_snapshot();
                for window in &mut snapshot.pools[0].windows {
                    window.used_percent = Some(0.0);
                }
                snapshot.pools[0].name =
                    "Codex usage with a very long descriptive account allowance name ".repeat(2);
                snapshot.pools[0].plan =
                    Some("A long enterprise subscription plan description".into());
                snapshot.pools[0].credits.as_mut().unwrap().balance =
                    Some("1234567890123456789012345678901234567890.12345678901234567890".into());
                snapshot
            }
        }
    }
}

#[test]
fn native_controls_fit_in_both_themes_at_common_dpi_scales() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).unwrap();
        let name = w!("TokWatch.Native.LayoutTest");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        });
        for dpi in [96u32, 120, 144, 168, 192] {
            for dark in [false, true] {
                for fixture in [Fixture::Normal, Fixture::Missing, Fixture::Long] {
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        name,
                        w!("TokWatch layout test"),
                        WS_POPUP | WS_CLIPCHILDREN,
                        160,
                        120,
                        PANEL_WIDTH * dpi as i32 / 96,
                        PANEL_HEIGHT * dpi as i32 / 96,
                        None,
                        None,
                        Some(instance.into()),
                        None,
                    )
                    .unwrap();
                    let host = new_host(
                        hwnd,
                        Settings::default(),
                        Some(fixture.snapshot()),
                        true,
                        None,
                    );
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
                    {
                        let mut borrowed = host.app.borrow_mut();
                        let app = &mut *borrowed;
                        app.dpi = dpi;
                        app.panel_dpi = dpi;
                        app.dark = dark;
                        let _ = DeleteObject(app.brush.into());
                        let _ = DeleteObject(app.surface_brush.into());
                        app.brush = CreateSolidBrush(app.bg());
                        app.surface_brush = CreateSolidBrush(ui_style::palette(dark).surface);
                        if matches!(fixture, Fixture::Long) {
                            app.error = Some(
                                "The local Codex connection is unavailable. Check the configured application path and sign in again through Codex before refreshing your account usage. Additional connection details should remain available without expanding or clipping this compact panel."
                                    .into(),
                            );
                        }
                        app.create_controls();
                        host.theme.set(PaintTheme::from_app(app));
                        app.render();
                        assert_eq!(app.labels.len(), 16);
                        assert_eq!(app.buttons.len(), 2);
                        verify_control_bounds(app, dpi, dark, fixture);
                        verify_text_layout(app, dpi, dark, fixture);
                        verify_control_semantics(app);
                        match fixture {
                            Fixture::Normal => {
                                assert_eq!(label_text(app.labels[4]), "26%");
                                assert_eq!(label_text(app.labels[11]), "2");
                            }
                            Fixture::Missing => {
                                assert_eq!(label_text(app.labels[4]), "—");
                                assert_eq!(label_text(app.labels[11]), "—");
                            }
                            Fixture::Long => {
                                assert_eq!(label_text(app.labels[4]), "100%");
                            }
                        }
                    }
                    host.flush(hwnd);
                    // Capture both native and fractional-scale renderings. All 30
                    // configurations use the renderer's actual DirectWrite metrics.
                    if matches!(dpi, 96 | 144) {
                        capture_test_panel(hwnd, &host, dark, fixture);
                    }
                    destroy_test_panel(hwnd, &host);
                }
            }
        }
        let _ = UnregisterClassW(name, Some(instance.into()));
    }
}

unsafe fn verify_control_bounds(app: &App, dpi: u32, dark: bool, fixture: Fixture) {
    unsafe {
        let mut client = RECT::default();
        GetClientRect(app.hwnd, &mut client).unwrap();
        let mut positions = Vec::new();
        for control in app.labels.iter().chain(&app.buttons) {
            let mut screen = RECT::default();
            GetWindowRect(*control, &mut screen).unwrap();
            let mut top_left = POINT {
                x: screen.left,
                y: screen.top,
            };
            let mut bottom_right = POINT {
                x: screen.right,
                y: screen.bottom,
            };
            assert!(ScreenToClient(app.hwnd, &mut top_left).as_bool());
            assert!(ScreenToClient(app.hwnd, &mut bottom_right).as_bool());
            assert!(
                top_left.x >= 0
                    && top_left.y >= 0
                    && bottom_right.x <= client.right
                    && bottom_right.y <= client.bottom,
                "control outside {dpi}-DPI panel (dark={dark}, {fixture:?}): {screen:?}"
            );
            positions.push((top_left, bottom_right));
        }
        // Native children should not cover one another even after DPI rounding.
        for (index, (start, end)) in positions.iter().enumerate() {
            for (other_index, (other_start, other_end)) in
                positions.iter().enumerate().skip(index + 1)
            {
                assert!(
                    end.x <= other_start.x
                        || other_end.x <= start.x
                        || end.y <= other_start.y
                        || other_end.y <= start.y,
                    "controls {index} and {other_index} overlap at {dpi} DPI"
                );
            }
        }
    }
}

unsafe fn verify_text_layout(app: &App, dpi: u32, dark: bool, fixture: Fixture) {
    unsafe {
        // Measure with the same fractional physical font size used for rendering.
        // Allow only one physical pixel for integer native window bounds.
        let scale = dpi as f32 / 96.0;
        let tolerance = 1.0;
        for (index, hwnd) in app.labels.iter().enumerate() {
            let text = label_text(*hwnd);
            let (size, weight) = FONT_SPECS[LABEL_SPECS[index].4];
            let extent = ui_render::measure_text(&text, size as f32 * scale, weight as u16)
                .expect("DirectWrite text measurement must be available");
            let mut rect = RECT::default();
            GetClientRect(*hwnd, &mut rect).unwrap();
            let width = rect.right as f32;
            let height = rect.bottom as f32;
            assert!(
                extent.height <= height + tolerance,
                "label {index} vertically clipped at {dpi} DPI (dark={dark}, {fixture:?}): '{text}' height {} vs {height}",
                extent.height
            );
            let external_metadata = matches!(index, 1 | 2 | 3 | 6 | 9 | 12 | 14 | 15);
            assert!(
                extent.width <= width + tolerance || external_metadata,
                "label {index} horizontally clipped at {dpi} DPI (dark={dark}, {fixture:?}): '{text}' width {} vs {width}",
                extent.width
            );
        }
        for hwnd in &app.buttons {
            let text = label_text(*hwnd);
            let extent = ui_render::measure_text(&text, 12.0 * scale, 400)
                .expect("DirectWrite button text measurement must be available");
            let mut rect = RECT::default();
            GetClientRect(*hwnd, &mut rect).unwrap();
            assert!(
                extent.width + 16.0 * scale <= rect.right as f32 + tolerance
                    && extent.height <= rect.bottom as f32 + tolerance,
                "button text does not fit at {dpi} DPI: '{text}'"
            );
        }
    }
}

unsafe fn verify_control_semantics(app: &App) {
    unsafe {
        for (index, label) in app.labels.iter().enumerate() {
            let style = GetWindowLongPtrW(*label, GWL_STYLE) as u32;
            assert_eq!(
                style & 0x1f,
                13,
                "label {index} must use DirectWrite owner drawing"
            );
            assert!(
                !label_text(*label).is_empty(),
                "label {index} is missing its native accessible name"
            );
        }
        for (button, expected_id) in app.buttons.iter().zip([REFRESH, SETTINGS]) {
            assert_eq!(GetDlgCtrlID(*button), expected_id as i32);
            let style = GetWindowLongPtrW(*button, GWL_STYLE) as u32;
            assert_ne!(
                style & WS_TABSTOP.0,
                0,
                "button must remain keyboard reachable"
            );
            assert_eq!(style & 0xf, BS_OWNERDRAW as u32);
            assert!(
                !label_text(*button).is_empty(),
                "native accessible button name is missing"
            );
        }
    }
}

unsafe fn label_text(hwnd: HWND) -> String {
    unsafe {
        let count = GetWindowTextLengthW(hwnd);
        let mut text = vec![0u16; count as usize + 1];
        let length = GetWindowTextW(hwnd, &mut text);
        String::from_utf16_lossy(&text[..length as usize])
    }
}

unsafe fn destroy_test_panel(hwnd: HWND, host: &Host) {
    unsafe {
        {
            let mut borrowed = host.app.borrow_mut();
            let app = &mut *borrowed;
            let _ = app.worker.tx.send(Command::Stop);
            for child in app.labels.drain(..).chain(app.buttons.drain(..)) {
                let _ = DestroyWindow(child);
            }
            for font in app.fonts.drain(..) {
                let _ = DeleteObject(font.into());
            }
            let _ = DeleteObject(app.brush.into());
            let _ = DeleteObject(app.surface_brush.into());
            if !app.icon.is_invalid() {
                let _ = DestroyIcon(app.icon);
            }
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn capture_test_panel(hwnd: HWND, host: &Host, dark: bool, fixture: Fixture) {
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).unwrap();
        let width = rect.right;
        let height = rect.bottom;
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
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
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let previous = SelectObject(dc, bitmap.into());
        {
            let app = host.app.borrow();
            FillRect(dc, &rect, app.brush);
        }
        SendMessageW(
            hwnd,
            WM_PRINT,
            Some(WPARAM(dc.0 as usize)),
            Some(LPARAM(
                (PRF_CLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
            )),
        );
        let _ = GdiFlush();
        let data = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
        verify_smoothed_card_corner(data, width, host.app.borrow().panel_dpi, dark);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54 + data.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&(-height).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(data);
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".verification");
        std::fs::create_dir_all(&directory).unwrap();
        let theme = if dark { "dark" } else { "light" };
        std::fs::write(
            directory.join(if host.app.borrow().panel_dpi == 96 {
                format!("native-panel-{theme}-{}.bmp", fixture.name())
            } else {
                format!(
                    "native-panel-{theme}-{}-{}pct.bmp",
                    fixture.name(),
                    host.app.borrow().panel_dpi * 100 / 96
                )
            }),
            bmp,
        )
        .unwrap();
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
    }
}

fn verify_smoothed_card_corner(pixels: &[u8], width: i32, dpi: u32, dark: bool) {
    let colors = ui_style::palette(dark);
    let solid_colors = [colors.bg.0, colors.surface.0, colors.border.0];
    let scale = |value: i32| value * dpi as i32 / 96;
    let mut blended_pixels = 0;
    // This part of the hero's rounded corner contains no native child controls.
    // Aliased GDI geometry can produce only the three solid palette colors;
    // the Direct2D edge must contain intermediate coverage values.
    for y in scale(50)..scale(62) {
        for x in scale(12)..scale(24) {
            let offset = ((y * width + x) * 4) as usize;
            let color = pixels[offset + 2] as u32
                | ((pixels[offset + 1] as u32) << 8)
                | ((pixels[offset] as u32) << 16);
            if !solid_colors.contains(&color) {
                blended_pixels += 1;
            }
        }
    }
    assert!(
        blended_pixels >= 4,
        "the rounded panel edge was not antialiased at {dpi} DPI (dark={dark})"
    );
}

#[test]
fn reset_dates_and_auth_urls_reject_invalid_input() {
    assert_eq!(local_date(u64::MAX), "Reset date unavailable");
    assert!(!open_auth("https://example.com/signin"));
    assert!(!open_auth("https://auth.openai.com.evil.example/signin"));
    assert!(!open_auth("file:///C:/Windows/System32/cmd.exe"));
}

#[test]
fn nested_callbacks_are_deferred_without_borrow_panics() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();
        let name = w!("TokWatch.Native.ReentrancyTest");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name,
            w!("TokWatch callback test"),
            WS_POPUP,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap();
        let host = new_host(hwnd, Settings::default(), Some(demo_snapshot()), true, None);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
        {
            let mut app = host.app.borrow_mut();
            app.create_controls();
            for _ in 0..1000 {
                SendMessageW(hwnd, WM_TIMER, Some(WPARAM(TICK)), Some(LPARAM(0)));
                SendMessageW(hwnd, RESULT, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert!(
                host.deferred.borrow().len() <= 64,
                "nested timer queue grew without bounds"
            );
        }
        destroy_test_panel(hwnd, &host);
        let _ = UnregisterClassW(name, Some(instance.into()));
    }
}

#[test]
fn flyout_geometry_stays_inside_scaled_and_offset_work_areas() {
    let cases = [
        (
            "1080p at 200 percent",
            192,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Tray(RECT {
                left: 1820,
                top: 1040,
                right: 1844,
                bottom: 1072,
            }),
        ),
        (
            "monitor with negative origin",
            192,
            RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: -48,
            },
            PanelAnchor::Tray(RECT {
                left: -80,
                top: -40,
                right: -56,
                bottom: -8,
            }),
        ),
        (
            "top-edge tray",
            144,
            RECT {
                left: 0,
                top: 48,
                right: 1920,
                bottom: 1080,
            },
            PanelAnchor::Tray(RECT {
                left: 4,
                top: 8,
                right: 28,
                bottom: 40,
            }),
        ),
        (
            "bottom-edge tray at native scale",
            96,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Tray(RECT {
                left: 1900,
                top: 1040,
                right: 1924,
                bottom: 1072,
            }),
        ),
        (
            "small work area",
            288,
            RECT {
                left: 0,
                top: 0,
                right: 640,
                bottom: 480,
            },
            PanelAnchor::Tray(RECT {
                left: 610,
                top: 470,
                right: 634,
                bottom: 494,
            }),
        ),
        (
            "DPI suggested rectangle outside display",
            192,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Window(RECT {
                left: 1800,
                top: 900,
                right: 2640,
                bottom: 2020,
            }),
        ),
    ];
    for (name, monitor_dpi, work, anchor) in cases {
        let (panel_dpi, bounds) = panel_geometry(monitor_dpi, work, anchor);
        assert!(
            panel_dpi > 0 && panel_dpi <= monitor_dpi,
            "invalid scale: {name}"
        );
        assert!(
            bounds.left >= work.left + 8
                && bounds.top >= work.top + 8
                && bounds.right <= work.right - 8
                && bounds.bottom <= work.bottom - 8,
            "flyout is not fully accessible: {name}, {bounds:?} in {work:?}"
        );
        assert_eq!(
            bounds.right - bounds.left,
            PANEL_WIDTH * panel_dpi as i32 / 96
        );
        assert_eq!(
            bounds.bottom - bounds.top,
            PANEL_HEIGHT * panel_dpi as i32 / 96
        );
        if monitor_dpi == 96 {
            assert_eq!(
                panel_dpi, 96,
                "ordinary displays should retain requested scale"
            );
        }
        if name == "top-edge tray" {
            assert!(bounds.top >= 48, "top tray should open into the work area");
        }
    }
}
