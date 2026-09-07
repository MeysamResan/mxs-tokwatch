# Windows installer

TokWatch installs for the current user under `%LOCALAPPDATA%\Programs\TokWatch`. Setup creates a Start menu shortcut, offers a desktop shortcut, and registers the app in Windows Settings. Administrator permission and extra language runtimes are not required.

Setup closes an existing TokWatch instance normally before upgrading it. Uninstall removes program files, shortcuts, and recognized startup/Claude integration entries. Preferences and usage data in `%LOCALAPPDATA%\TokWatch` are preserved.

## Build

```powershell
.\scripts\package.ps1 -Force
```

This runs formatting, lint, tests, and a release build through `scripts/build.ps1`, then creates only `dist/v<version>/TokWatch-Setup-v<version>-windows-x64.exe`. `-Force` replaces an existing build and removes the old portable packaging outputs. Build scripts do not publish.

`scripts/bootstrap-installer.ps1` obtains the pinned Inno Setup 6.7.3 compiler in the ignored `.tools` folder, checking its SHA-256 and Authenticode signature. The TokWatch installer itself is unsigned. The stable AppId is `{A30D44CA-5373-4B32-8BFA-B7B3C2560B70}`.

## Updates

A numeric GitHub release tag such as `v0.2.1` must have one asset named `TokWatch-Setup-v0.2.1-windows-x64.exe`. GitHub supplies its `sha256:` digest in the release API. The app rejects missing digests, duplicate assets, unexpected URLs, incomplete downloads, or checksum mismatches.

After **Restart to update**, a hidden helper waits for the exact running process, rechecks both file hashes, and invokes Setup silently in the existing folder. It restarts TokWatch after setup succeeds. If setup or relaunch fails, it restores the prior executable and records an error for the next launch. Staged installers are removed after the attempt.

## Verify

```powershell
.\scripts\test-installer.ps1
```

Fixtures use a separate AppId, window class, startup entry, and install folder. Checks cover install, upgrade, the production updater helper, recovery, uninstall, shortcuts, registration, and preservation of user files. They do not use the normal TokWatch installation or provider settings.
