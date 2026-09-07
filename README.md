# TokWatch

A native Windows 11 system tray app for monitoring your Codex usage allowance. Built with Rust and Win32, with the remaining percentage beside the clock and a compact details panel on hover.

Codex provides allowance percentages. TokWatch displays the percentage remaining; it does not estimate an exact token balance.

## What it shows

- Remaining allowance inside a circular tray gauge, including the full percentage such as `14%`. The digits stay proportional, with a smaller percent suffix for legibility.
- Time until its scheduled reset, plus the reset date in local time.
- Available full resets and the earliest expiry reported by Codex.
- Account plan, extra-credit balance, and other usage windows when available.
- A clear unavailable or cached state when current information cannot be retrieved.

You can choose the usage window and refresh every 1, 2, 5, or 10 minutes. The default is 2 minutes. Automatic selection uses the window with the least remaining allowance in the main Codex pool when that pool is available.

## Native interface

The tray ring shows how much allowance remains. Green means more than 50%, amber means 21�50%, and red means 20% or less. Unknown or stale readings use a neutral ring and `?`. The same thresholds apply to the panel meter. The icon is drawn at the current Windows tray size with smooth edges; the ring and center readout update only when the value, theme, or scale changes.

The panel puts the remaining percentage and usage meter in one compact card. Two stat columns below it show the reset countdown and available full resets. A small plan badge, credit row, connection status, and two actions keep the details together without extra card padding.

The interface follows the Windows light or dark theme automatically. Segoe UI text, rounded surfaces, a mint accent, and a Windows 11 rounded window give it a consistent native appearance. The panel is 320 by 320 logical pixels, using about 56% less area than the previous 420 by 560 panel. It scales with the display and reduces its scale when needed to fit the available screen area.

Direct2D draws antialiased curves, and DirectWrite draws grayscale-antialiased text at the actual display resolution. Fractional font sizes preserve smooth scaling without stretching a panel bitmap. Windows graphics and font resources are cached on the UI thread while the panel is open and released when it closes; there is no extra UI runtime or continuous rendering loop.

Labels and buttons remain native Windows controls with custom drawing and accessible names. Buttons retain keyboard navigation, visible focus, and disabled states. The primary action changes from **Refresh** to **Connect to Codex** when a connection is needed.

## Run

Requirements: Windows 11 x64 and a compatible native Codex installation with a ChatGPT account sign-in. Rust is required to build TokWatch, but not to run the resulting executable.

Download the [Windows x64 portable ZIP](https://github.com/MeysamResan/mxs-tokwatch/releases/download/v0.1.0/TokWatch-v0.1.0-windows-x64.zip) from the [v0.1.0 preview release](https://github.com/MeysamResan/mxs-tokwatch/releases/tag/v0.1.0), extract it into a permanent folder, and run **TokWatch.exe**. The ZIP includes the executable, quick-start instructions, and MIT license. This preview is unsigned.

To build from source instead, use the instructions below, then run:

```powershell
.\target\release\tokwatch.exe
```

For the private local toolchain, the build output is `target\x86_64-pc-windows-gnullvm\release\tokwatch.exe`. A prepared portable copy in `dist\TokWatch.exe` can also be run directly.

TokWatch starts in the system tray. Windows may initially place it in the tray overflow; move its icon beside the clock to keep the percentage visible.

| Action | Behavior |
| --- | --- |
| Hover over the tray icon | Show usage and reset details |
| Click the tray icon | Keep the panel open; click again to dismiss |
| Escape or click another window | Dismiss the open panel |
| Right-click the icon, or choose Settings | Open refresh, window selection, startup, and connection options |
| Refresh | Request a new account usage reading |
| Exit | Close TokWatch |

**Start with Windows** is optional and off by default. Enable it from the tray menu after placing the executable in its intended folder. If you later move the executable, turn this option off and on again from the new location.

To open the real panel immediately:

```powershell
.\target\release\tokwatch.exe --show
```

To preview the interface with clearly labelled simulated data:

```powershell
.\target\release\tokwatch.exe --demo
```

Demo mode does not query your account. Only one TokWatch instance runs per Windows session, so exit an existing instance before switching modes.

## Connecting to Codex

TokWatch looks for a native `codex.exe` on PATH, inside supported npm Codex installations, and among the Windows Codex desktop app's installed helpers. Use **Choose Codex executable** if automatic discovery misses your installation. Select the native executable; `.cmd` and `.ps1` launchers are not supported.

It starts that executable in app-server mode, checks the existing account, reads its usage limits, and closes the helper after the reading. The Codex desktop window does not need to remain open. Authentication comes from the selected Codex installation and its configured Codex home.

If the helper already has a usable ChatGPT sign-in, no additional sign-in is needed. Otherwise, **Connect to Codex** requests Codex's browser sign-in flow. TokWatch does not ask for your password or an API key. API-key accounts do not provide the ChatGPT subscription allowance this app displays.

Existing-sign-in account reads have been checked locally. The browser sign-in path is implemented but has not yet been exercised end to end. If it fails, sign in through your Codex installation, then use **Refresh** in TokWatch.

Monitoring uses the official [Codex app-server account interface](https://learn.chatgpt.com/docs/app-server). It sends account queries, without starting an agent conversation, sending prompts, purchasing credits, or redeeming resets.

## Build and check

Use a current stable Rust toolchain. The standard Windows setup is the MSVC Rust toolchain with Visual Studio C++ build tools and the Windows SDK. The private local setup uses Rust 1.98.1 targeting `x86_64-pc-windows-gnullvm` with LLVM-MinGW. MSVC has not been exercised in this local validation run.

From the project directory:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
```

With the normal MSVC setup, the executable is `target\release\tokwatch.exe`. The release profile uses size optimization, link-time optimization, one code generation unit, and stripped symbols. Target settings in `.cargo/config.toml` statically link the C/unwind runtime for the x64 MSVC and GNU-LLVM targets; Windows system DLLs remain normal dependencies. The build-only `embed-manifest` dependency embeds `app.manifest` for native controls, per-monitor scaling, and normal user privileges.

The PowerShell helper runs the same sequence:

```powershell
.\scripts\build.ps1
```

Individual tasks are available:

```powershell
.\scripts\build.ps1 -Task Format
.\scripts\build.ps1 -Task Check
.\scripts\build.ps1 -Task Lint
.\scripts\build.ps1 -Task Test
.\scripts\build.ps1 -Task Build
```

`-Configuration Debug` selects a debug build; `-Target` accepts an explicit Rust target. If the private project-local Rust and LLVM toolchains exist under `.tools`, the script uses them with temporary process environment settings and selects `x86_64-pc-windows-gnullvm`. Its executable is `target\x86_64-pc-windows-gnullvm\release\tokwatch.exe`. Otherwise it uses Cargo from PATH. `.tools` is ignored by Git and is not required by the application or included in the source distribution.

The ordinary tests do not require a Codex account. They include 30 native layout combinations at 100%, 125%, 150%, 175%, and 200% display scaling in both light and dark themes with normal, missing, and long account metadata. Checks use actual DirectWrite text metrics, retain accessible control names and keyboard focus, and cover nested Windows callbacks. Rendering checks produce 12 panel captures and exercise 48 tray variants at native 16, 20, 24, and 32 pixel sizes. An explicitly ignored integration test can perform real, read-only account requests through an installed and signed-in Codex helper:

```powershell
cargo test live_read -- --ignored --nocapture
```

See [architecture and behavior](docs/architecture.md) for protocol handling and resource details.

## Prepare a portable release

Build, verify, and package a local Windows x64 release with:

```powershell
.\scripts\package.ps1
```

The helper runs `scripts/build.ps1 -Task Verify -Configuration Release` and writes the executable, quick-start README, MIT license, portable ZIP, and SHA-256 checksums to `dist\v0.1.0`. The ZIP is `TokWatch-v0.1.0-windows-x64.zip`; extract it into a permanent folder and run `TokWatch.exe`. No Rust toolchain is required to run it.

The package is an unsigned preview. The script checks the executable architecture and refuses to replace different artifacts unless you explicitly use `-Force`. It supports `-Target` for an x64 Windows Rust target. Output folders remain ignored by Git, and packaging does not commit, upload, or publish anything.

See the [v0.1.0 release notes](docs/releases/v0.1.0.md) and [prepared GitHub description, topics, and manual release steps](docs/github-release.md). A release tag must point to a commit containing the complete tested source, including any source files that were uncommitted when the executable was built.

## Local data and removal

TokWatch stores `settings.json` and a minimized `usage-cache.json` in `%LOCALAPPDATA%\TokWatch`. The cache contains usage readings and timestamps; it does not contain authentication tokens, account email addresses, raw protocol responses, or reset-credit IDs. Authentication remains managed by Codex. Codex makes the network requests needed to retrieve account information; TokWatch has no separate service or analytics endpoint.

The optional startup entry is the `TokWatch` value under `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.

To remove the app, turn off **Start with Windows**, exit TokWatch, and delete its executable. Delete `%LOCALAPPDATA%\TokWatch` too if you want to remove its settings and cache. Codex's own installation and sign-in are separate.

## Local validation

The Windows x64 release was built and run locally on September 7, 2026.

- Formatting and strict Clippy checks passed.
- The release executable is 653312 bytes and imports only Windows system DLLs.
- 24 automated tests passed, covering 30 native layout combinations at 100%, 125%, 150%, 175%, and 200% scaling, both themes, normal/missing/long account metadata, DirectWrite text fit, accessible controls, bounded callback deferral, protocol handling, and cache behavior.
- Rendering checks produced 12 panel captures and exercised 48 percentage-icon variants at native 16, 20, 24, and 32 pixel sizes.
- A separate live account test reused the existing ChatGPT sign-in, fetched usage and reset details, and confirmed the helper was reaped.
- The running tray app fetched two usage pools and two available resets.

A Codex helper temporarily adds memory and CPU work during a refresh; normal refreshes release it afterward. Resource measurements depend on the build, Windows configuration, and Codex installation.

Native panel rendering was inspected in light and dark themes. Full interactive tray behavior across Windows configurations and a fresh browser sign-in still need a manual validation pass.
## Current limits

- This first version targets Windows 11 x64. Other Windows versions and architectures have not been validated.
- Available fields depend on the Codex version and account. Missing values are shown as unavailable, and a reported zero remains distinct from a missing value.
- A reset countdown reaching zero does not prove that allowance has replenished. The next successful account reading determines the displayed balance.
- Codex installation paths and its account protocol can change. Update Codex or select a compatible native executable when needed.
- Browser sign-in, long-running reliability, and the full range of display scaling and accessibility configurations still need broader validation.
- This project does not currently include an installer, automatic updater, usage predictions, or alert notifications.

## License

MIT. See [LICENSE](LICENSE).


