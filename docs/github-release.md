# GitHub repository and release details

These values are prepared for the owner to enter manually. The local build and packaging scripts do not commit, upload, create tags, edit GitHub, or publish releases.

## Repository About

Repository: [MeysamResan/mxs-tokwatch](https://github.com/MeysamResan/mxs-tokwatch)

Description:

> Lightweight native Windows 11 tray monitor for Codex usage, remaining allowance, and reset times. Built with Rust and Win32.

Topics:

```text
windows windows-11 rust win32 system-tray codex usage-monitor
```

Leave Website empty unless you have a separate project website. The repository README contains the user and build documentation.

## Prepare the local assets

Run from the repository root:

```powershell
.\scripts\package.ps1
```

The helper runs the required release verification through `scripts/build.ps1`, then prepares `dist\v0.1.0`. Build outputs remain ignored by Git. It does not publish anything.

## Create the release manually

1. Review and manually commit and push the complete tested source, including new source files already present in the working tree. A tag at an older commit will not reproduce a binary built from uncommitted changes.
2. In the repository's About settings, enter the description and topics above.
3. Open the repository's Releases page and create a new release.
4. Use tag **v0.1.0**, targeting the commit containing the tested source, and title **TokWatch v0.1.0 - Windows x64 preview**.
5. Paste the body from [the release notes](releases/v0.1.0.md), omitting its first heading if you use it as the title. Mark this initial preview as a **pre-release**.
6. Attach the following files from `dist\v0.1.0`: **TokWatch.exe**, **TokWatch-v0.1.0-windows-x64.zip**, **SHA256SUMS.txt**, and **LICENSE**. The ZIP already contains the license and quick-start README.
7. Review the completed release and publish it manually.

The binary is unsigned. The notes record the remaining manual validation and do not claim full Windows or sign-in compatibility.
