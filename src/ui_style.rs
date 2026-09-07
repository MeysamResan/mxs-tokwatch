use super::*;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED,
};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: COLORREF,
    pub(super) surface: COLORREF,
    pub(super) border: COLORREF,
    pub(super) text: COLORREF,
    pub(super) muted: COLORREF,
    pub(super) accent: COLORREF,
    pub(super) accent_bg: COLORREF,
    pub(super) track: COLORREF,
    pub(super) warning: COLORREF,
    pub(super) danger: COLORREF,
    pub(super) button_hover: COLORREF,
}

pub(super) fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: color(18, 21, 27),
            surface: color(27, 32, 40),
            border: color(44, 51, 63),
            text: color(241, 245, 249),
            muted: color(155, 168, 185),
            accent: color(111, 231, 194),
            accent_bg: color(31, 61, 54),
            track: color(48, 58, 70),
            warning: color(246, 193, 104),
            danger: color(255, 137, 148),
            button_hover: color(39, 47, 58),
        }
    } else {
        Palette {
            bg: color(245, 247, 249),
            surface: color(255, 255, 255),
            border: color(220, 227, 233),
            text: color(25, 36, 47),
            muted: color(98, 114, 129),
            accent: color(21, 129, 100),
            accent_bg: color(221, 245, 234),
            track: color(228, 235, 239),
            warning: color(153, 99, 16),
            danger: color(186, 55, 73),
            button_hover: color(233, 239, 243),
        }
    }
}

fn mix(a: COLORREF, b: COLORREF, amount: u32) -> COLORREF {
    let channel = |shift: u32| {
        ((((a.0 >> shift) & 255) * (100 - amount) + ((b.0 >> shift) & 255) * amount) / 100) as u8
    };
    color(channel(0), channel(8), channel(16))
}

// Each paint owns its small set of GDI objects and restores the caller's DC.
// No bitmap surfaces, render thread, or timer is needed while the flyout is idle.
unsafe fn rounded(dc: HDC, rect: RECT, radius: i32, fill: COLORREF, border: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, border);
        let previous_brush = SelectObject(dc, brush.into());
        let previous_pen = SelectObject(dc, pen.into());
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(dc, previous_pen);
        SelectObject(dc, previous_brush);
        let _ = DeleteObject(pen.into());
        let _ = DeleteObject(brush.into());
    }
}

unsafe fn line(dc: HDC, points: &[POINT], width: i32, ink: COLORREF) {
    unsafe {
        let pen = CreatePen(PS_SOLID, width.max(1), ink);
        let previous = SelectObject(dc, pen.into());
        let _ = Polyline(dc, points);
        SelectObject(dc, previous);
        let _ = DeleteObject(pen.into());
    }
}

unsafe fn circle(dc: HDC, rect: RECT, fill: Option<COLORREF>, ink: COLORREF, width: i32) {
    unsafe {
        let brush = fill.map(|value| CreateSolidBrush(value));
        let pen = CreatePen(PS_SOLID, width.max(1), ink);
        let previous_pen = SelectObject(dc, pen.into());
        let previous_brush = SelectObject(
            dc,
            brush
                .map(HGDIOBJ::from)
                .unwrap_or_else(|| GetStockObject(NULL_BRUSH)),
        );
        let _ = Ellipse(dc, rect.left, rect.top, rect.right, rect.bottom);
        SelectObject(dc, previous_brush);
        SelectObject(dc, previous_pen);
        let _ = DeleteObject(pen.into());
        if let Some(brush) = brush {
            let _ = DeleteObject(brush.into());
        }
    }
}

pub(super) unsafe fn paint_panel(app: &App, dc: HDC, region: &RECT) {
    unsafe {
        let colors = palette(app.dark);
        let px = |value| app.px(value);
        let rect = |x, y, width, height| RECT {
            left: px(x),
            top: px(y),
            right: px(x + width),
            bottom: px(y + height),
        };
        let point = |x, y| POINT { x: px(x), y: px(y) };
        FillRect(dc, region, app.brush);

        // Small signal mark, with the same silhouette as the usage meter.
        rounded(
            dc,
            rect(24, 22, 32, 32),
            px(10),
            colors.accent_bg,
            colors.accent_bg,
        );
        for (x, y, height) in [(32, 39, 8), (39, 33, 14), (46, 28, 19)] {
            rounded(
                dc,
                rect(x, y, 3, height),
                px(1),
                colors.accent,
                colors.accent,
            );
        }
        rounded(
            dc,
            rect(324, 28, 72, 25),
            px(8),
            colors.surface,
            colors.border,
        );

        // Generous gutters and one consistent radius keep native controls quiet.
        for (x, y, width, height) in [
            (24, 84, 372, 196),
            (24, 294, 180, 112),
            (216, 294, 180, 112),
        ] {
            rounded(
                dc,
                rect(x, y, width, height),
                px(16),
                colors.surface,
                colors.border,
            );
        }

        rounded(dc, rect(42, 220, 336, 8), px(4), colors.track, colors.track);
        if let Some(percent) = app
            .selected()
            .and_then(|(_, window)| window.remaining_percent())
        {
            let ink = if app.stale() {
                colors.muted
            } else if percent <= 10 {
                colors.danger
            } else if percent <= 20 {
                colors.warning
            } else {
                colors.accent
            };
            if percent > 0 {
                let mut meter = rect(42, 220, 336, 8);
                meter.right =
                    meter.left + ((meter.right - meter.left) * percent as i32 / 100).max(1);
                rounded(
                    dc,
                    meter,
                    px(4).min((meter.right - meter.left) / 2),
                    ink,
                    ink,
                );
            }
        }

        // Reset time: a light clock outline, drawn without font glyph fallback.
        circle(dc, rect(42, 311, 15, 15), None, colors.muted, px(1));
        line(
            dc,
            &[point(49, 314), point(49, 319), point(52, 321)],
            px(1),
            colors.muted,
        );

        // Reset credits: two overlapping tickets.
        rounded(
            dc,
            rect(237, 311, 12, 12),
            px(3),
            colors.surface,
            colors.muted,
        );
        rounded(
            dc,
            rect(234, 314, 12, 12),
            px(3),
            colors.surface,
            colors.muted,
        );
        line(dc, &[point(237, 320), point(243, 320)], px(1), colors.muted);

        // Detail and connection state are visually separate from the usage data.
        line(dc, &[point(24, 419), point(396, 419)], 1, colors.border);
        line(dc, &[point(24, 470), point(396, 470)], 1, colors.border);
        let status = if app.error.is_some() {
            colors.danger
        } else if app.stale() || app.busy || app.login_pending {
            colors.warning
        } else {
            colors.accent
        };
        circle(dc, rect(25, 488, 7, 7), Some(status), status, 1);
    }
}

pub(super) unsafe fn paint_button(
    dpi: u32,
    dark: bool,
    font: HFONT,
    item: &DRAWITEMSTRUCT,
    text: &str,
    primary: bool,
    hovered: bool,
) {
    unsafe {
        let colors = palette(dark);
        let px = |value: i32| (value * dpi as i32 / 96).max(1);
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused =
            item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0;
        let dc = item.hDC;
        let background = CreateSolidBrush(colors.bg);
        FillRect(dc, &item.rcItem, background);
        let _ = DeleteObject(background.into());

        let (fill, border, foreground) = if disabled {
            (colors.surface, colors.border, colors.muted)
        } else if primary {
            let base = if dark {
                colors.accent
            } else {
                color(18, 112, 87)
            };
            let fill = if pressed {
                mix(base, color(0, 0, 0), 13)
            } else if hovered {
                mix(base, color(255, 255, 255), 12)
            } else {
                base
            };
            (
                fill,
                fill,
                if dark {
                    color(13, 45, 35)
                } else {
                    color(255, 255, 255)
                },
            )
        } else {
            let fill = if pressed {
                colors.track
            } else if hovered {
                colors.button_hover
            } else {
                colors.surface
            };
            (fill, colors.border, colors.text)
        };
        rounded(dc, item.rcItem, px(9), fill, border);
        if focused && !disabled {
            let inset = px(3);
            let focus = RECT {
                left: item.rcItem.left + inset,
                top: item.rcItem.top + inset,
                right: item.rcItem.right - inset,
                bottom: item.rcItem.bottom - inset,
            };
            rounded(
                dc,
                focus,
                px(6),
                fill,
                if primary { foreground } else { colors.accent },
            );
        }

        let previous_font = SelectObject(dc, font.into());
        let previous_mode = SetBkMode(dc, TRANSPARENT);
        let previous_color = SetTextColor(dc, foreground);
        let mut bounds = item.rcItem;
        if pressed {
            bounds.top += px(1);
        }
        let mut caption: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            dc,
            &mut caption,
            &mut bounds,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SetTextColor(dc, previous_color);
        let _ = SetBkMode(dc, BACKGROUND_MODE(previous_mode as u32));
        SelectObject(dc, previous_font);
    }
}
