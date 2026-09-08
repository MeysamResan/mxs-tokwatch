//! Pixel-aligned native rendering. Windows owns all graphics and font resources;
//! no bitmap scaling, permanent animation loop, or additional UI runtime is involved.

use std::{cell::RefCell, collections::HashMap};
use windows::Win32::UI::Controls::{
    BPBF_TOPDOWNDIB, BeginBufferedPaint, BufferedPaintInit, BufferedPaintUnInit, EndBufferedPaint,
};

use windows::{
    Win32::{
        Foundation::{COLORREF, RECT},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::HDC,
        },
    },
    core::w,
};

#[derive(Clone, Copy)]
pub(super) enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy)]
pub(super) struct TextStyle {
    pub size: f32,
    pub weight: u16,
    pub color: COLORREF,
    pub align: Align,
    pub center_y: bool,
    pub ellipsis: bool,
}

struct CachedFormat {
    format: IDWriteTextFormat,
    ellipsis: IDWriteInlineObject,
}

struct Renderer {
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    write: IDWriteFactory,
    formats: HashMap<(u32, u16), CachedFormat>,
}

thread_local! {
    // This is used only by the window's UI thread. Reentrant painting falls back
    // to the caller instead of borrowing a render target that is already drawing.
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

impl Renderer {
    unsafe fn new() -> windows::core::Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                // This small, event-driven panel does not need a GPU device.
                // Keep driver/device-loss failures out of its drawing path.
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                // Coordinates already account for the current window's DPI. A
                // second scale here would blur text and round fractional edges.
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            })?;
            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // Grayscale text remains clean on both themes and non-RGB displays.
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = target.CreateSolidColorBrush(&color(COLORREF(0)), None)?;
            let write = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            BufferedPaintInit()?;
            Ok(Self {
                target,
                brush,
                write,
                formats: HashMap::new(),
            })
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = BufferedPaintUnInit();
        }
    }
}

/// Draw into the supplied DC rectangle using physical pixels. Drawing coordinates
/// are local to the rectangle's top-left corner. The caller paints a complete
/// background, or leaves pixels transparent for DWM, and may use a GDI fallback.
pub(super) fn paint(dc: HDC, bounds: RECT, draw: impl FnOnce(&mut Canvas<'_>)) -> bool {
    if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
        return false;
    }
    RENDERER.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        if slot.is_none() {
            *slot = unsafe { Renderer::new().ok() };
        }
        let Some(renderer) = slot.as_mut() else {
            return false;
        };
        unsafe {
            let mut buffered_dc = HDC::default();
            let buffer = BeginBufferedPaint(dc, &bounds, BPBF_TOPDOWNDIB, None, &mut buffered_dc);
            if buffer == 0 {
                return false;
            }
            if renderer.target.BindDC(buffered_dc, &bounds).is_err() {
                let _ = EndBufferedPaint(buffer, false);
                *slot = None;
                return false;
            }
            renderer.target.BeginDraw();
            renderer.target.Clear(None);
            renderer.brush.SetOpacity(1.0);
            let mut canvas = Canvas {
                target: &renderer.target,
                brush: &renderer.brush,
                write: &renderer.write,
                formats: &mut renderer.formats,
                failed: false,
            };
            draw(&mut canvas);
            let failed = canvas.failed;
            let finished = renderer.target.EndDraw(None, None).is_ok();
            let copied = EndBufferedPaint(buffer, finished && !failed).is_ok();
            if !finished {
                // Device loss is recoverable: next paint recreates the target.
                *slot = None;
            }
            copied && finished && !failed
        }
    })
}

// The tray stays resident, but its graphics buffers need not remain allocated
// after the details panel closes. Reopening lazily creates the renderer again.
pub(super) fn release() {
    RENDERER.with(|slot| {
        if let Ok(mut renderer) = slot.try_borrow_mut() {
            *renderer = None;
        }
    });
}

pub(super) struct Canvas<'a> {
    target: &'a ID2D1DCRenderTarget,
    brush: &'a ID2D1SolidColorBrush,
    write: &'a IDWriteFactory,
    formats: &'a mut HashMap<(u32, u16), CachedFormat>,
    failed: bool,
}

impl Canvas<'_> {
    pub(super) fn opacity(&mut self, alpha: f32) {
        unsafe {
            self.brush.SetOpacity(alpha.clamp(0.0, 1.0));
        }
    }

    pub(super) fn fill(&mut self, rect: RECT, fill: COLORREF) {
        unsafe {
            self.brush.SetColor(&color(fill));
            self.target.FillRectangle(&float_rect(rect), self.brush);
        }
    }

    pub(super) fn rounded(
        &mut self,
        rect: RECT,
        radius: f32,
        fill: COLORREF,
        border: Option<COLORREF>,
    ) {
        unsafe {
            let shape = D2D1_ROUNDED_RECT {
                rect: float_rect(rect),
                radiusX: radius,
                radiusY: radius,
            };
            self.brush.SetColor(&color(fill));
            self.target.FillRoundedRectangle(&shape, self.brush);
            if let Some(border) = border {
                self.brush.SetColor(&color(border));
                let outline = D2D1_ROUNDED_RECT {
                    rect: inset(shape.rect, 0.5),
                    radiusX: (radius - 0.5).max(0.0),
                    radiusY: (radius - 0.5).max(0.0),
                };
                self.target
                    .DrawRoundedRectangle(&outline, self.brush, 1.0, None);
            }
        }
    }

    pub(super) fn line(&mut self, from: (f32, f32), to: (f32, f32), width: f32, stroke: COLORREF) {
        unsafe {
            // D2D shares these vector fields with windows-numerics; using the
            // native structure avoids adding another direct dependency.
            let mut start = D2D1_ELLIPSE::default().point;
            start.X = from.0;
            start.Y = from.1;
            let mut end = start;
            end.X = to.0;
            end.Y = to.1;
            self.brush.SetColor(&color(stroke));
            self.target.DrawLine(start, end, self.brush, width, None);
        }
    }

    pub(super) fn ellipse(&mut self, rect: RECT, fill: COLORREF, border: Option<COLORREF>) {
        unsafe {
            let mut shape = D2D1_ELLIPSE::default();
            shape.point.X = (rect.left as f32 + rect.right as f32) * 0.5;
            shape.point.Y = (rect.top as f32 + rect.bottom as f32) * 0.5;
            shape.radiusX = (rect.right - rect.left) as f32 * 0.5;
            shape.radiusY = (rect.bottom - rect.top) as f32 * 0.5;
            self.brush.SetColor(&color(fill));
            self.target.FillEllipse(&shape, self.brush);
            if let Some(border) = border {
                shape.radiusX = (shape.radiusX - 0.5).max(0.0);
                shape.radiusY = (shape.radiusY - 0.5).max(0.0);
                self.brush.SetColor(&color(border));
                self.target.DrawEllipse(&shape, self.brush, 1.0, None);
            }
        }
    }

    pub(super) fn progress_bar(
        &mut self,
        rect: RECT,
        percent: Option<f32>,
        track: COLORREF,
        progress: COLORREF,
    ) {
        let bounds = float_rect(rect);
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.rounded(rect, height.min(width) * 0.5, track, None);
        let Some(percent) = percent.filter(|p| p.is_finite()) else {
            return;
        };
        let filled_width = width * percent.clamp(0.0, 100.0) / 100.0;
        if filled_width <= 0.0 {
            return;
        }
        unsafe {
            // Use physical subpixels so short fills stay accurate during the
            // brief value transition and at fractional display scales.
            let radius = height.min(filled_width) * 0.5;
            let shape = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    right: bounds.left + filled_width,
                    ..bounds
                },
                radiusX: radius,
                radiusY: radius,
            };
            self.brush.SetColor(&color(progress));
            self.target.FillRoundedRectangle(&shape, self.brush);
        }
    }

    pub(super) fn text(&mut self, text: &str, rect: RECT, style: TextStyle) {
        unsafe {
            let Some(cached) = text_format(self.write, self.formats, style.size, style.weight)
            else {
                self.failed = true;
                return;
            };
            let align = match style.align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            };
            let vertical = if style.center_y {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            };
            let trimming = DWRITE_TRIMMING {
                granularity: if style.ellipsis {
                    DWRITE_TRIMMING_GRANULARITY_CHARACTER
                } else {
                    DWRITE_TRIMMING_GRANULARITY_NONE
                },
                ..Default::default()
            };
            let sign = style.ellipsis.then_some(&cached.ellipsis);
            if cached.format.SetTextAlignment(align).is_err()
                || cached.format.SetParagraphAlignment(vertical).is_err()
                || cached.format.SetTrimming(&trimming, sign).is_err()
            {
                self.failed = true;
                return;
            }
            self.brush.SetColor(&color(style.color));
            let utf16: Vec<u16> = text.encode_utf16().collect();
            self.target.DrawText(
                &utf16,
                &cached.format,
                &float_rect(rect),
                self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

unsafe fn text_format<'a>(
    write: &IDWriteFactory,
    formats: &'a mut HashMap<(u32, u16), CachedFormat>,
    size: f32,
    weight: u16,
) -> Option<&'a CachedFormat> {
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let key = (size.to_bits(), weight);
    // A normal panel uses fewer than ten formats per DPI. Keep a bound even if
    // the app moves repeatedly across unusual display scales during a session.
    if formats.len() >= 96 && !formats.contains_key(&key) {
        formats.clear();
    }
    if let std::collections::hash_map::Entry::Vacant(entry) = formats.entry(key) {
        unsafe {
            let format = write
                .CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-US"),
                )
                .ok()?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
            let ellipsis = write.CreateEllipsisTrimmingSign(&format).ok()?;
            entry.insert(CachedFormat { format, ellipsis });
        }
    }
    formats.get(&key)
}

fn color(value: COLORREF) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (value.0 & 0xff) as f32 / 255.0,
        g: ((value.0 >> 8) & 0xff) as f32 / 255.0,
        b: ((value.0 >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn float_rect(rect: RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

fn inset(rect: D2D_RECT_F, amount: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left + amount,
        top: rect.top + amount,
        right: rect.right - amount,
        bottom: rect.bottom - amount,
    }
}

#[cfg(test)]
pub(super) struct TextSize {
    pub width: f32,
    pub height: f32,
}

/// Measure untruncated single-line text with the same fonts as the real panel.
#[cfg(test)]
pub(super) fn measure_text(text: &str, size: f32, weight: u16) -> Option<TextSize> {
    RENDERER.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        if slot.is_none() {
            *slot = unsafe { Renderer::new().ok() };
        }
        let renderer = slot.as_mut()?;
        unsafe {
            let cached = text_format(&renderer.write, &mut renderer.formats, size, weight)?;
            let utf16: Vec<u16> = text.encode_utf16().collect();
            let layout = renderer
                .write
                .CreateTextLayout(&utf16, &cached.format, 100_000.0, 100_000.0)
                .ok()?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).ok()?;
            Some(TextSize {
                width: metrics.widthIncludingTrailingWhitespace,
                height: metrics.height,
            })
        }
    })
}
