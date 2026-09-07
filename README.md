# TokWatch

A small Windows tray app that shows your remaining **Codex or Claude Code subscription allowance** as a colored percentage.

**[Download the Windows installer](https://github.com/MeysamResan/mxs-tokwatch/releases/download/v0.2.0/TokWatch-Setup-v0.2.0-windows-x64.exe)**

Requires Windows 11 x64. This preview is unsigned.

## Use

1. Install TokWatch and open it from the Start menu.
2. Click the percentage in the system tray to see usage and reset times.
3. Right-click to choose **Codex** or **Claude Code**, connect your account, or enable **Start with Windows**.

If the icon is hidden, open the tray overflow and drag it beside the other status icons.

**Codex** uses your existing Codex sign-in and refreshes every two minutes by default. **Claude Code** reads a local status-line feed that updates while you use Claude Code. An existing custom status line is preserved; see [Claude setup](docs/claude-integration.md) for integration details.

## Updates

TokWatch checks GitHub releases at startup and every six hours. When an update is ready, choose **Restart to update**. Updates use the installer and preserve your settings.

Uninstall through **Windows Settings > Apps > Installed apps**. Your local preferences are kept.

## Build

With Rust and Windows C++ build tools installed:

```powershell
.\scripts\build.ps1
.\scripts\package.ps1
```

The installer is written to `dist/v0.2.0/`. See [installer development](docs/windows-installer.md) and [architecture](docs/architecture.md) for details.

[Changelog](CHANGELOG.md) · [MIT license](LICENSE)

An independent project, not affiliated with OpenAI or Anthropic.