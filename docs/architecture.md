# Architecture

TokWatch is a Windows notification-area application with a native Win32 panel. Rust owns its state and worker coordination; Microsoft Windows bindings provide the tray, controls, drawing, registry, and process APIs. `serde` and `serde_json` handle data conversion. There is no webview, managed runtime, embedded browser, or database.

## Modules

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Entry point and Windows-only UI dispatch |
| `src/ui.rs` | Tray icon, native panel controls, menu, timers, worker commands, and optional startup entry |
| `src/ui_style.rs` | Light/dark palettes, rounded card surfaces, progress meter, icons, and owner-drawn buttons |
| `src/codex.rs` | Executable discovery, app-server process lifetime, protocol framing, and account access |
| `src/model.rs` | Bounded usage model, optional-field handling, remaining percentages, and countdowns |
| `src/settings.rs` | Settings and minimized usage-cache persistence |
| `src/ui_tests.rs` | Native layout, input validation, and callback regression checks |
| `build.rs` / `app.manifest` | Build-only manifest embedding for native controls, DPI awareness, and normal user privileges |

The main thread owns Windows controls and processes the Windows message queue. A worker waits for refresh or sign-in commands. Each active Codex client has a bounded reply queue and a dedicated pipe reader, so account requests do not block the UI thread.

## Native panel design

The panel is 420 by 560 logical pixels, scaled using the monitor DPI. Its scale is capped to fit the available monitor work area, and placement is clamped at screen edges. This fitting also applies when the window moves between display scales. The tray icon keeps the actual monitor DPI. A large usage card emphasizes the remaining percentage and meter. Two smaller cards separate the scheduled reset countdown from the available full-reset count. The header holds the app identity and plan badge; compact detail and status rows lead to the primary action and Settings button.

Windows provides the top-level rounded window through DWM attributes. The app follows the system light/dark theme and updates the background, card surfaces, borders, text, and mint accent together when the theme changes. Segoe UI fonts use ClearType rendering. GDI draws the card shapes, progress meter, small icons, and button surfaces; there is no rendering loop or additional UI framework.

The text labels and buttons remain native child controls, preserving their accessible text and standard keyboard behavior. Buttons are owner-drawn to provide matching colors and rounded shapes, with distinct hover, pressed, disabled, and keyboard-focus states. The primary action is Refresh for a connected account and Connect to Codex when authentication is needed. A cached paint theme allows native controls to request colors and button drawing safely during nested Windows callbacks.

Server-provided labels and secondary metadata use bounded controls and ellipsis where needed. Unknown data retains explicit unavailable wording instead of a fabricated zero. The UI separates these states from current usage and continues to display local countdowns without additional account requests.

## Account connection

```text
TokWatch native UI
    |
    | worker command
    v
Hidden native Codex helper
    codex.exe app-server --listen stdio://
    |
    | Codex-managed authentication and account requests
    v
OpenAI account service
```

The local protocol is newline-delimited JSON over redirected standard input and output. TokWatch completes `initialize` / `initialized`, then calls `account/read` and `account/rateLimits/read`. It does not create threads, send model prompts, or invoke reset redemption. The only sign-in action, `account/login/start` with `type: "chatgpt"`, follows a user selecting **Connect**.

A normal refresh owns a new helper client for the duration of the account read. The client is then dropped and the process released. Browser sign-in retains its helper until an explicit account/login/completed event with the matching login ID arrives, with a five-minute sign-in limit. Each individual protocol request has a 30-second timeout.

On Windows, the helper is launched without a console window and attached to a job configured to kill its processes when the job closes. Client shutdown closes input, releases the job, terminates and waits for the helper, and joins the pipe reader. The helper's working directory is the user profile instead of the directory from which TokWatch happened to launch.

Authentication belongs to Codex. TokWatch does not read Codex authentication files or retain credentials. Protocol diagnostics from the helper are discarded; user-facing failures are generated locally. Replies are bounded to 1 MiB and the reader queue holds at most eight messages. Unexpected server requests are not approved.

The implementation follows the official [Codex app-server interface](https://learn.chatgpt.com/docs/app-server). Compatibility is checked through the real handshake and account calls rather than a hard-coded executable version requirement. Installed-version differences can therefore surface as an unsupported-interface error.

## Discovery and account choice

An explicit path chosen in settings takes precedence. Automatic discovery checks:

1. Native Codex executables in absolute PATH directories.
2. Supported npm package layouts, including nested and hoisted Windows platform packages.
3. Versioned helpers under `%LOCALAPPDATA%\OpenAI\Codex\bin`, selecting the most recently modified executable.

The client launches the executable directly, without a command shell. A chosen path must be absolute, point to an existing file, and have an `.exe` extension on Windows.

The account is whichever ChatGPT sign-in the selected helper exposes through its configured Codex home. This version does not maintain an independent account list or support switching among several accounts inside TokWatch. An API-key account is reported separately because it does not supply ChatGPT subscription limits.

## Usage semantics

The model consumes the available per-pool rate-limit map and falls back to the legacy single-pool response. Each returned usage window keeps its actual duration and reset timestamp. Window labels are derived from duration rather than assuming every primary window is five hours.

Remaining allowance is `floor(100 - clamp(usedPercent, 0, 100))`. Missing and non-finite values remain unknown. Automatic selection prefers the main Codex pool and its known window with the lowest remaining percentage; a saved window selection is retained while that window exists.

The available reset count comes from the service's explicit count. It is not inferred from the length of a potentially truncated reset-credit list. Expiry text uses the earliest known available-credit expiry. Extra-credit balances retain the reported string in the model without inventing a currency or unit. The panel formats ordinary numeric balances to two decimal places and uses ellipsis for oversized labels.

Countdowns use the service timestamp and the local clock. At or after a reset time, the panel indicates that the reset is due and awaits confirmation from a new reading. The app does not manufacture a 100% reading when a countdown expires.

## Refresh and stale data

The default refresh interval is 120 seconds. The menu offers 60, 120, 300, and 600 seconds; settings normalization bounds manually edited values to 60–1800 seconds. A five-second UI timer schedules due work, and overlapping requests are suppressed. Resume notifications request a refresh. Connection failures increase the delay exponentially, capped at 30 minutes; manual refresh remains available.

The worker sleeps between commands. The tray icon is regenerated when its displayed text, theme, or scale changes. The visible panel has a short timer for hover dismissal and local countdown rendering; it does not request account information on each visual update.

Cached readings begin unverified after launch. A failed refresh or an old reading makes the tray show `?`, while the panel can retain the last known percentage with cached wording. The age threshold is twice the polling interval, with a three-minute minimum. A confirmed signed-out or API-key account clears the prior usage cache.

## Storage and startup

Settings and cache files live under `%LOCALAPPDATA%\TokWatch`:

- `settings.json`: polling interval, optional native Codex path, and optional selected-window key.
- `usage-cache.json`: normalized usage data and its fetch timestamp.

The cache excludes account identifiers, credentials, raw replies, and reset-credit IDs. File reads have size limits. Writes use a unique sibling temporary file, flush it, then replace the destination, preserving the previous file if replacement fails.

Startup is opt-in through a quoted executable path in the current user's `Run` registry key. The app uses a local named mutex to prevent a second instance in the same Windows session.

## Resource tradeoff and validation

This implementation prioritizes a small resident tray process by closing the Codex helper after each normal reading. That saves helper memory between refreshes at the cost of another helper startup for the next read. One local account-read observation placed the helper around 99 MB while active; this is an observation for that installation, not a memory guarantee or an estimate of the tray application's own footprint. Startup cost and long-term resource behavior require further measurement across systems.

The local tooling uses Rust 1.98.1 targeting `x86_64-pc-windows-gnullvm` with LLVM-MinGW. The Cargo host remains the local Rust GNU installation; the script selects the LLVM target and its matching linker for the application. Only a narrow import-tool directory is added to PATH, preserving the host linker. Output is under `target/x86_64-pc-windows-gnullvm/release`. A convenience delivery copy can live at `dist/TokWatch.exe`.

The source uses Windows APIs through Microsoft's Rust bindings. MSVC is the documented standard Windows developer setup, but it has not been exercised in this local validation run. `.cargo/config.toml` requests static C/unwind runtime linking for x64 MSVC and GNU-LLVM, so the private compiler DLLs are build-time tools rather than application dependencies. `build.rs` uses the build-only `embed-manifest` crate to embed `app.manifest`; no manifest helper runs with the application.

Unit tests cover response normalization, incomplete fields, countdown boundaries, settings defaults and persistence, account classification, sign-in URL validation, protocol limits, and diagnostic redaction. Native UI checks cover control bounds and text layout at 100%, 150%, and 200% scaling in both light and dark themes, with normal, missing, and long account metadata. They also check native accessible button names, invalid reset dates and authentication URLs, and nested callback handling. An ignored live test performs the real handshake and read-only account queries and checks helper shutdown. Browser login remains implemented but unverified end to end; demo mode supplies simulated data for UI inspection without account requests.


