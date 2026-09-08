# Architecture

TokWatch is a Windows notification-area application with a native Win32 panel. Rust owns state and worker coordination; Microsoft Windows bindings provide controls, drawing, registry, and process APIs. `serde` and `serde_json` handle data conversion. There is no webview, managed UI runtime, embedded browser, or database.

This document describes the current local TokWatch source. Release history is summarized in the changelog.

## Modules

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Entry point, helper modes, and Windows UI dispatch |
| `src/ui.rs` | Tray activation, native panel, provider menu, timers, worker coordination, and startup option |
| `src/ui_style.rs` | Windows light/dark and contrast palettes, exact system accent, and owner-drawn controls |
| `src/ui_theme.rs` | Public Windows appearance APIs and event subscriptions |
| `src/ui_motion.rs` | Bounded, reversible easing for brief panel and control transitions |
| `src/ui_render.rs` | Cached Direct2D/DirectWrite drawing, text layout, and test measurements |
| `src/ui_tray.rs` | Large number tray indicator with transparent, native-size premultiplied-alpha output |
| `src/ui_tray_tests.rs` | Native icon dimensions, rectangular text fit and margins, smooth coverage, theme colors, and unknown-state checks |
| `src/codex.rs` | Codex executable discovery, app-server lifetime, protocol framing, and account reads |
| `src/codex_lifecycle_tests.rs` | Offline helper reuse, graceful shutdown, cancellation, and recovery checks |
| `src/claude.rs` | Claude Code status-line setup, bounded feed input, local feed storage, and subscription normalization |
| `src/updater.rs` | GitHub release discovery, verified installer downloads, and update handoff |
| `src/installation.rs` | Refresh an existing matching Installed Apps version after an in-place update |
| `src/model.rs` | Bounded usage model, optional fields, remaining percentages, and countdowns |
| `src/settings.rs` | Provider and update preferences, settings, and minimized usage-cache persistence |
| `src/ui_tests.rs` | Native layout, input validation, and callback regression checks |
| `build.rs` / `app.manifest` | Build-only manifest embedding for native controls, DPI awareness, and normal privileges |

The main thread owns Windows controls and the message queue. Account and update work happens off the UI thread. Each active Codex client uses a bounded reply queue and dedicated pipe reader. Idle workers wait for commands instead of continuously polling a service.

## Native panel and tray

Clicking or keyboard-activating the tray icon opens the panel; activating it again closes it. Escape or focus moving to another window dismisses it. Hover does not open the panel. Right-clicking the tray icon opens the menu.

The compact 320-by-292-DIP panel follows the Windows system/taskbar theme and uses the exact `UISettings.GetColorValue(Accent)` RGB for its primary action and horizontal usage bar. Text over the accent chooses black or white for readable contrast. Public DWM attributes provide the system border, rounded corners, and Desktop Acrylic (`DWMSBT_TRANSIENTWINDOW`), appropriate to a transient flyout. The DWM transient API selects the brightest Acrylic variant, which is visibly lighter than the taskbar. A shared theme tint over that material reduces its brightness while retaining one-third of the backdrop contribution. Parent and native child controls reproduce the same tinted base, including empty text areas and button corners. This approximates the taskbar tone; it does not retrieve Explorer's internal material or promise identical pixels over different backgrounds. Acrylic requires Windows 11 build 22621 or later; older systems and disabled transparency use a solid light or dark surface. Contrast themes use system colors and disable glass and motion. UISettings events and Windows theme messages apply appearance changes while the app is running. The tray follows the Windows taskbar theme for legibility on light and dark backgrounds. The usage header combines the provider and available subscription, such as `Codex · Pro`, with the selected period aligned at the right. A left-aligned remaining percentage sits above a horizontal allowance bar. The app name, logo, and separate subscription badge are omitted from the flyout. Reset times, credits, freshness and connection status, and the existing refresh/connect and settings actions remain available below the usage display.

The tray percentage is a conventional notification-area icon registered with `Shell_NotifyIconW`. Windows owns the icon slot and its position among the other system-tray icons. `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` supplies the target bitmap size: 16 pixels at 100% scaling. The icon displays the remaining number from 0 to 100 without a percent sign. Visible Segoe UI glyph bounds select the largest font that fits the rectangular bitmap with a one-physical-pixel margin at 16 pixels, scaled proportionally at larger sizes. Native GDI renders the text at four times the target resolution; area averaging produces a smooth grayscale mask in the final premultiplied-alpha bitmap. The centered text uses the contrasting theme text color. Unknown or stale readings display `?`, and the native tooltip retains the full percentage wording. Rendering uses the monitor containing the actual tray icon rectangle, falling back to the primary taskbar monitor before the existing window DPI. Icon generation is cached until the displayed value, appearance, provider, or tray-monitor DPI changes; display changes refresh it through the existing event path.

There is no taskbar child window, Explorer layout hook, floating label, or saved desktop position. Click opens the details panel anchored to `Shell_NotifyIconGetRect`; right-click opens settings. Hover notifications are ignored. Taskbar recreation re-registers the icon, and failed registration is retried by the existing five-second timer.

Panel coordinates scale with the monitor DPI and are capped to fit the available work area. Placement is clamped at screen edges. Direct2D draws geometry, and DirectWrite renders Segoe UI with grayscale antialiasing and fractional font sizes at the actual display resolution. The panel is never rasterized at one scale and stretched to another.

The Direct2D render target uses software rendering explicitly; this small panel does not need a hardware graphics device. The render target, brush, DirectWrite factory, and text formats are cached on the UI thread while the panel is open and released when it closes. A premultiplied-alpha buffered DC preserves the translucent theme base for DWM, including pixels painted by native child controls. Graphics failure permits a GDI fallback; an invalid target is recreated on the next paint. Opening and closing use short, reversible slide transitions; hover and usage-meter changes also ease to their targets. A 16-ms timer exists only while a transition is active, then stops. Windows animation preferences apply immediately, including finishing an in-progress transition when animations are disabled. There is no continuous rendering loop.

Labels and buttons remain native child controls with accessible names. Custom drawing retains keyboard navigation and visible focus, pressed, hover, and disabled states. A cached paint theme permits drawing during nested Windows callbacks. Bounded labels use ellipsis where necessary. Missing values use unavailable wording rather than a fabricated zero.

## Provider selection

The menu selects Codex or Claude Code and saves that choice. The UI routes refresh and connection actions to the selected provider. A provider switch must not display another provider's cached usage as current. Account selection remains owned by the selected provider's installation; TokWatch does not manage an independent list of sign-ins.

Both providers normalize into usage windows with used percentages, durations, reset timestamps, and the source reading's time. Optional fields stay optional. Codex-only full resets and credits are not invented for Claude Code.

## Codex account connection

```text
TokWatch UI -> account worker -> hidden native Codex helper
                                   |
                                   | Codex-managed authentication and requests
                                   v
                              OpenAI account service
```

The worker starts `codex.exe app-server --listen stdio://`. Newline-delimited JSON flows over redirected standard input/output. TokWatch completes `initialize` / `initialized`, then calls `account/read` and `account/rateLimits/read`. It does not create conversations, send model prompts, or redeem resets. `account/login/start` with `type: "chatgpt"` follows the user's Connect action.

Normal refreshes reuse one helper client. The worker reconnects after a failure, a helper exit, or an executable-path change; explicit Connect starts a fresh login connection. Switching to Claude releases the Codex helper. Browser sign-in waits for a matching `account/login/completed` event, with a five-minute limit, then reuses the authenticated helper for subsequent reads. Individual requests time out after 30 seconds. An idle helper remains resident between refreshes to avoid repeated startup and forced termination.

The Windows helper starts without a console and belongs to a job configured to kill its processes when the job closes. Shutdown closes input first and allows up to two seconds for the helper to exit before releasing the job; forced termination is a fallback for an unresponsive helper or remaining descendants. The worker waits for the helper and joins the reader. App exit cancels pending protocol waits (checked at most every 100 milliseconds while awaiting replies) and joins the worker so process exit does not bypass graceful cleanup. The helper uses the user profile as its working directory.

Authentication belongs to Codex. TokWatch does not read authentication files or store credentials. It discards helper diagnostics and produces local user-facing errors. Replies are limited to 1 MiB and the reader queue holds at most eight messages. Unexpected server requests are not approved.

The implementation follows the official [Codex app-server interface](https://learn.chatgpt.com/docs/app-server). Compatibility is tested through the handshake and account calls rather than a hard-coded executable version requirement. API-key accounts are reported separately because they do not supply ChatGPT subscription limits.

An explicit executable path takes precedence. Automatic discovery checks native executables in absolute PATH directories, supported npm package layouts, and versioned helpers under `%LOCALAPPDATA%\OpenAI\Codex\bin`. Executables launch directly without a command shell. A chosen path must be absolute, existing, and end in `.exe` on Windows.

## Claude Code status-line connection

```text
Claude Code session -> status-line JSON -> TokWatch.exe --claude-statusline
                                                |
                                                | normalized local feed
                                                v
TokWatch UI <- account worker <- claude-statusline.json
```

Claude Code's official [status-line JSON](https://code.claude.com/docs/en/statusline) exposes subscription `rate_limits` for Pro and Max accounts after a response. The feed is driven by Claude Code session events. Reading it does not send model prompts, and TokWatch never calls an undocumented subscription endpoint or reads Claude credentials.

**Connect Claude Code** installs a status-line command in `%USERPROFILE%\.claude\settings.json`, respecting an absolute `CLAUDE_CONFIG_DIR` override. It preserves unrelated settings and backs up the original configuration to a sibling `.settings.tokwatch-backup.PID.SEQ.json` file. An existing unrelated `statusLine` is not overwritten: TokWatch writes `%LOCALAPPDATA%\TokWatch\claude-statusline-settings.json` as a setup snippet and returns an actionable message for manual integration.

The `--claude-statusline` mode bounds its standard input and persists only normalized numeric `five_hour` and `seven_day` allowance windows, resets, and timestamps in `%LOCALAPPDATA%\TokWatch\claude-statusline.json`. It does not save the original session JSON, which can contain unrelated session data. An input without subscription limits replaces earlier limits with an unknown state.

TokWatch checks the local feed every 15 seconds. A read preserves the feed's source timestamp; repeatedly reading an old file never makes it fresh. The UI shows its age, marks stale readings unknown, and treats expired windows as unknown until a new source reading arrives. Refresh only rereads this local file. It cannot request an independent Claude service refresh.

Project or managed status-line settings, or `disableAllHooks`, can prevent the global command from running. Existing Claude sessions may need to reload settings. Keep the TokWatch executable in its permanent folder because the command points to that path. After moving it or installing a portable copy, explicit Connect can relocate an exactly recognized generated TokWatch bridge while preserving custom commands. Removing the integration means removing TokWatch's `statusLine` entry or carefully restoring the saved prior configuration without losing later edits.

Claude web-only usage, model-specific weekly meters, extra credits, and full-reset credits are not supplied by this integration. Context-window percentages and API spending are not used as substitutes for subscription allowance.

## Usage and freshness semantics

Remaining allowance is `floor(100 - clamp(usedPercent, 0, 100))`. Missing or non-finite values stay unknown. Automatic selection chooses the known window with the least remaining allowance, preferring the main Codex pool when available. A saved window selection remains effective while the window exists. Labels use the reported duration instead of assuming all primary windows last five hours.

Codex reads the available per-pool map, falling back to the legacy single-pool response. Reset counts use the explicit service count, not the length of a possibly truncated credit list. Expiry text uses the earliest known available-credit expiry. Extra-credit balances retain the reported string without inventing a currency; the panel formats ordinary numeric values to two decimal places.

Countdowns use the service reset time and local clock. Reaching zero does not establish replenishment. The app waits for a new reading instead of manufacturing 100% allowance.

The default Codex interval is 120 seconds; the menu offers 60, 120, 300, and 600 seconds. Settings normalization bounds manually edited intervals to 60-1800 seconds. Overlapping requests are suppressed, resume requests a refresh, and connection failures back off up to 30 minutes. Manual refresh remains available. Claude feed reads use their own 15-second interval.

Cached readings start unverified after launch. Failure or age makes the tray show `?`; the panel can retain the last known percentage with cached wording. The ordinary age threshold is twice the polling interval, with a three-minute minimum. A confirmed signed-out or API-key Codex account clears its prior usage cache. Local countdown repainting does not trigger account network requests.

## GitHub application updates

Automatic checks are enabled by default, occur at startup and every six hours, and can be disabled in the menu. Manual checking remains available. Discovery and downloading use the fixed `MeysamResan/mxs-tokwatch` repository, not a configurable feed supplied by a remote response.

The updater accepts a newer numeric `vMAJOR.MINOR.PATCH` published release with one versioned **TokWatch-Setup-vMAJOR.MINOR.PATCH-windows-x64.exe** asset and its GitHub-provided SHA-256 digest. A source commit or tag by itself is not an update. Preview `0.x` builds consider published previews and stable releases; stable `1.x` and later builds consider stable releases only.

Downloads use bounded HTTPS requests to permitted GitHub/CDN hosts. Windows CNG verifies the SHA-256 digest from GitHub release-asset metadata. A missing or malformed digest makes a release ineligible. The staged file, `.TokWatch-update-{version}-{pid}-{timestamp}.exe`, sits beside the installed executable and is verified before installation is offered. **Restart to update** runs the verified installer silently in the existing app folder after the running app exits. Setup upgrades all installed files and registration. The helper waits for the original process, identified by PID and creation time, to exit. The previous executable is retained through replacement and relaunch checks, and restored if either fails. The original executable fingerprint is rechecked before replacement. A manually changed target is preserved and is not automatically launched. Recovery only relaunches an executable whose checksum matches the original. The application directory must be writable by the current user; the installer does not request elevation. Failures are recorded in `%LOCALAPPDATA%\TokWatch\update-error.txt` for the next startup.

GitHub, HTTPS, and control of the fixed repository are the update trust boundary. Checksums detect damaged or mismatched files; they do not authenticate the publisher independently of GitHub, and the preview binary remains unsigned. GitHub update traffic is separate from provider account data.

## Storage and startup

Files under `%LOCALAPPDATA%\TokWatch` include:

- `settings.json`: provider, automatic-update preference, Codex interval, optional executable path, and selected usage window.
- `usage-cache.json`: minimized normalized usage data and its source timestamp.
- `claude-statusline.json`: normalized Claude Code subscription windows and timestamps.
- `claude-statusline-settings.json`: setup snippet when a custom status line requires manual integration.
- `update-error.txt`: an updater failure report consumed at the next startup. Staged downloads and replacement backups reside beside the installed EXE.

Usage data excludes credentials, account email addresses, raw protocol/status-line replies, and reset-credit IDs. File reads are bounded. Settings and usage writes flush a unique sibling temporary file and replace the destination, preserving the previous file if replacement fails.

Startup is opt-in through a quoted executable path in the current user's `Run` registry key. A local named mutex prevents a second UI instance in the same Windows session. Helper modes run without opening the tray UI.

## Windows installation

The local Inno Setup package installs for the current user under `%LOCALAPPDATA%\Programs\TokWatch`, allowing the existing updater to replace the executable without elevation. A stable AppId preserves upgrade and uninstall identity. The installer adds a Start menu shortcut, optional desktop shortcut, and 64-bit HKCU Installed Apps registration. Setup gracefully closes TokWatch using its native Exit command; WM_CLOSE only hides its panel.

The installer migrates an existing startup value only when it recognizes the installed target or the running copy it closed. Uninstall removes only its own exact startup command and Claude Code bridge, preserves preferences and cached usage, and leaves provider installations alone. On a normal launch the app updates DisplayVersion only when an existing registration has a matching installation directory. Portable runs create no registration. The only public download asset is the versioned setup executable. Its SHA-256 digest is supplied by GitHub; no portable executable, ZIP, or checksum text file is needed.

See [installer details](windows-installer.md) for tooling and package verification.

## Build and validation

Use `scripts/build.ps1` for formatting, linting, tests, and local builds. The private toolchain targets `x86_64-pc-windows-gnullvm` using Rust 1.98.1 and LLVM-MinGW. The standard developer setup is MSVC Rust with the Windows SDK and C++ build tools; MSVC still needs a separate validation run.

The release statically links the C/unwind runtime for supported x64 targets and uses normal Windows system DLLs. Build-only manifest embedding has no runtime helper. `.tools`, `target`, `dist`, and `.verification` stay out of source control. Packaging does not publish anything.

Automated checks cover protocol normalization, missing fields, provider configuration, feed freshness, update parsing and verification, cache persistence, native layout at 100-200% scaling, DirectWrite text fit, accessible controls, callback handling, transparent parent/child pixel output, backdrop selection, transition cleanup, accent contrast, and number-only tray icons. Rendering tests write panel and tray captures for visual inspection. An ignored live Codex test performs the handshake and read-only account calls and checks helper shutdown. Demo mode supplies simulated data without querying an account.

The README and v0.3.0 release notes record completed local validation. Live Claude session setup, actual replacement by a future release, fresh Codex browser sign-in, and broader interactive and long-running behavior need manual validation appropriate to the change; earlier v0.1.0 test counts are not claims about this preview.
