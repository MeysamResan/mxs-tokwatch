# Changelog

## 0.3.0 — 2026-09-08

- Replace the tray circle with a large centered number without a percent sign. Fit supersampled Segoe UI text to the rectangular icon bounds with a small margin, use the theme text color, show `?` for unavailable or stale readings, and render for the actual tray monitor DPI.
- Compact the flyout to 320 by 292 DIPs with a horizontal remaining-allowance bar and a left-aligned percentage. Combine the provider and subscription in the usage header, remove the app name, logo, and separate plan badge, and retain reset times, credits, status, and actions.
- Correct the overly bright Acrylic background with a consistent theme tint beneath the panel and its native controls, bringing the dark flyout closer to the Windows taskbar tone.
- Restyle the native flyout with Windows Desktop Acrylic, system borders and corners, neutral cards, and updated spacing and typography.
- Follow the Windows system/taskbar theme and exact accent color, with live appearance updates and solid/high-contrast fallbacks.
- Add short opening, closing, hover, and usage-meter transitions that respect Windows animation settings and stop their timer when idle.
- Verify translucent parent/child rendering, accent contrast, native backdrop attributes, animation cleanup, and layout at 100-200% scaling.
- Reuse the Codex helper between refreshes instead of starting and forcibly stopping it on every poll.
- Allow helper cleanup after closing its input, with bounded termination for unresponsive helpers and cancellation during app exit.
- Render the small details panel in software to reduce exposure to graphics device and driver failures.
- Add offline process lifecycle regression tests. These stability mitigations do not establish or confirm a fix for the reported Windows LSASS crash.

## 0.2.0 — 2026-09-08

- Choose Codex or Claude Code subscription monitoring.
- Show a colored percentage in one normal system-tray icon.
- Open details on click, with a black, dark gray, and red theme.
- Install, upgrade, and uninstall through a per-user Windows installer.
- Check published GitHub releases for updates and verify installers against their SHA-256 asset digests.
- Preserve preferences, startup choices, and unrelated Claude Code settings.
- Distribute only the installer; remove portable downloads and obsolete release-preparation files.

Claude usage updates while Claude Code supplies status-line data. Available account fields depend on the provider. This is an unsigned Windows 11 x64 preview.

## 0.1.0 — 2026-09-07

- Initial Windows tray monitor for Codex allowance and reset times.