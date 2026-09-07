[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).Path
$verificationRoot = Join-Path $projectRoot '.verification'
$fixtureRoot = Join-Path $verificationRoot ('installer-smoke-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
$installDir = Join-Path $fixtureRoot 'installed O''Brien $test'
$dataDir = Join-Path $fixtureRoot 'user-data'
$fixtureName = 'TokWatch Installer Verification'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{938A5CA4-FA7C-40F0-9EAF-E2F89746655C}_is1'
$runKey = 'Software\Microsoft\Windows\CurrentVersion\Run'
$startupName = 'TokWatchInstallerVerification'
$programDir = Join-Path ([Environment]::GetFolderPath('Programs')) $fixtureName
$desktopLink = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) ($fixtureName + '.lnk')
$registry = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64)
$originalData = [Environment]::GetEnvironmentVariable('TOKWATCH_INSTALLER_FIXTURE_DATA', 'Process')
$originalUpdateFixture = [Environment]::GetEnvironmentVariable('TOKWATCH_UPDATE_FIXTURE', 'Process')
$normalApp = @(Get-Process -Name tokwatch -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
$startupEntry = $registry.OpenSubKey($runKey)
try { $existingStartup = if ($startupEntry) { $startupEntry.GetValue($startupName, $null) } else { $null } }
finally { if ($startupEntry) { $startupEntry.Dispose() } }
$existingInstall = $registry.OpenSubKey($uninstallKey)
if ($existingInstall -or $null -ne $existingStartup -or (Test-Path -LiteralPath $programDir) -or (Test-Path -LiteralPath $desktopLink)) {
    if ($existingInstall) { $existingInstall.Dispose() }
    throw 'An installer verification entry already exists. Inspect it before running the test again.'
}
foreach ($directory in @($verificationRoot, $fixtureRoot)) {
    $full = [IO.Path]::GetFullPath($directory)
    if (-not $full.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture must remain inside this project.' }
    if (Test-Path -LiteralPath $directory) {
        $item = Get-Item -LiteralPath $directory -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Fixture directories must be ordinary directories.' }
    }
}
[IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null
[IO.Directory]::CreateDirectory($dataDir) | Out-Null
$clang = Join-Path $projectRoot '.tools\llvm-mingw-20260826-ucrt-x86_64\bin\x86_64-w64-mingw32-clang.exe'
if (-not (Test-Path -LiteralPath $clang -PathType Leaf)) { throw 'The installer fixture uses the project-local LLVM/MinGW C compiler.' }
$compiler = & (Join-Path $PSScriptRoot 'bootstrap-installer.ps1')
$source = Join-Path $PSScriptRoot 'fixtures\installer-window.c'
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class TokWatchInstallerFixtureNative { [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, IntPtr title); [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint msg, IntPtr wp, IntPtr lp); }'
function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}
function Wait-Fixture {
    for ($attempt = 0; $attempt -lt 80; $attempt++) {
        if ([TokWatchInstallerFixtureNative]::FindWindow('TokWatch.Installer.Verification.Window', [IntPtr]::Zero) -ne [IntPtr]::Zero) { return }
        Start-Sleep -Milliseconds 100
    }
    throw 'Fixture window did not start.'
}
function Invoke-Installer {
    param([string]$Executable, [string[]]$Arguments)
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru
    Assert-True ($process.ExitCode -eq 0) "Installer returned $($process.ExitCode): $Executable"
}
function Read-Startup {
    $key = $registry.OpenSubKey($runKey)
    try { if ($key) { return $key.GetValue($startupName, $null) } }
    finally { if ($key) { $key.Dispose() } }
    return $null
}
function Write-Startup {
    param([string]$Value)
    $key = $registry.CreateSubKey($runKey)
    try { $key.SetValue($startupName, $Value) } finally { $key.Dispose() }
}
function Compile-Fixture {
    param([string]$Version, [string]$OutputDirectory)
    [IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
    $binary = Join-Path $OutputDirectory 'TokWatch.exe'
    & $clang -O2 -municode -mwindows ('-DFIXTURE_VERSION=L"' + $Version + '"') $source -o $binary -luser32
    if ($LASTEXITCODE -ne 0) { throw 'Native installer fixture compilation failed.' }
    & $compiler '/Q' '/DTestFixture=1' ("/DProjectRoot=$projectRoot") ("/DAppVersion=$Version") ("/DSourceExe=$binary") ("/O$OutputDirectory") (Join-Path $projectRoot 'installer\TokWatch.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Fixture setup compilation failed.' }
    return @{ Binary = $binary; Setup = Join-Path $OutputDirectory "TokWatch-Setup-v$Version-windows-x64.exe" }
}
function Invoke-VerifiedUpdate {
    param([string]$Setup, [Diagnostics.Process]$Running, [string]$Scenario, [switch]$Tamper, [switch]$ExpectFailure)
    $staged = Join-Path $installDir ('.TokWatch-update-' + $Scenario + '.exe')
    Copy-Item -LiteralPath $Setup -Destination $staged
    $ready = Join-Path $fixtureRoot ($Scenario + '.ready')
    $errorFile = Join-Path $fixtureRoot ($Scenario + '-error.txt')
    $helper = Join-Path $fixtureRoot ($Scenario + '-helper.ps1')
    $inputFile = Join-Path $fixtureRoot ($Scenario + '-input.json')
    $inputData = [ordered]@{target=$installedExe;staged=$staged;ready=$ready;error=$errorFile;output=$helper;parent_pid=$Running.Id;created=$Running.StartTime.ToFileTimeUtc()}
    [IO.File]::WriteAllText($inputFile, ($inputData | ConvertTo-Json))
    [Environment]::SetEnvironmentVariable('TOKWATCH_UPDATE_FIXTURE', $inputFile, 'Process')
    & (Join-Path $PSScriptRoot 'build.ps1') -Task Test -TestFilter export_installer_helper_fixture -IncludeIgnored
    if ($Tamper) { [IO.File]::AppendAllText($staged, 'tampered after verification') }
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $worker = Start-Process -FilePath $powershell -ArgumentList @('-NoProfile', '-NonInteractive', '-File', ('"' + $helper + '"')) -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        if ($worker.HasExited) { throw 'Update helper exited before its handshake.' }
        if ((Test-Path -LiteralPath $ready) -and [IO.File]::ReadAllText($ready) -eq 'READY') { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert-True ((Test-Path -LiteralPath $ready) -and [IO.File]::ReadAllText($ready) -eq 'READY') 'Update helper handshake timed out.'
    $window = [TokWatchInstallerFixtureNative]::FindWindow('TokWatch.Installer.Verification.Window', [IntPtr]::Zero)
    [TokWatchInstallerFixtureNative]::PostMessage($window, 0x111, [IntPtr]106, [IntPtr]::Zero) | Out-Null
    Assert-True ($worker.WaitForExit(45000)) 'Installer update timed out.'
    Wait-Fixture
    Assert-True ((Test-Path -LiteralPath $errorFile) -eq [bool]$ExpectFailure) 'Unexpected update result.'
    Assert-True (-not (Test-Path -LiteralPath $staged)) 'Staged installer was not cleaned up.'
    Assert-True (-not (Test-Path -LiteralPath $ready)) 'Update handshake file was not cleaned up.'
}
function Installed-FixtureProcess {
    $running = @(Get-Process -Name TokWatch -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $installedExe })
    Assert-True ($running.Count -eq 1) 'Expected exactly one running installed fixture.'
    return $running[0]
}
$report = [ordered]@{ fixture = $fixtureRoot; passed = @() }
try {
    [Environment]::SetEnvironmentVariable('TOKWATCH_INSTALLER_FIXTURE_DATA', $dataDir, 'Process')
    $sentinel = Join-Path $dataDir 'settings.json'
    [IO.File]::WriteAllText($sentinel, '{"keep":"user settings"}')
    $initial = Compile-Fixture '0.2.0' (Join-Path $fixtureRoot 'initial')
    $upgrade = Compile-Fixture '0.2.1' (Join-Path $fixtureRoot 'upgrade')
    $portable = Start-Process -FilePath $initial.Binary -WindowStyle Hidden -PassThru
    Wait-Fixture
    Write-Startup ('"' + $initial.Binary + '"')
    $setupArgs = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/DIR="' + $installDir + '"'), '/TASKS=desktopicon')
    Invoke-Installer $initial.Setup ($setupArgs + ('/LOG="' + (Join-Path $fixtureRoot 'install.log') + '"'))
    Assert-True $portable.HasExited 'Initial setup did not gracefully close the portable fixture.'
    $installedExe = Join-Path $installDir 'TokWatch.exe'
    Assert-True ((Get-FileHash -LiteralPath $installedExe).Hash -eq (Get-FileHash -LiteralPath $initial.Binary).Hash) 'Installed executable bytes differ.'
    Assert-True ((Read-Startup) -eq ('"' + $installedExe + '"')) 'Startup entry was not migrated.'
    $entry = $registry.OpenSubKey($uninstallKey)
    try {
        Assert-True ($null -ne $entry) 'Installed Apps registration is missing.'
        Assert-True ($entry.GetValue('DisplayVersion') -eq '0.2.0') 'Initial DisplayVersion is wrong.'
        Assert-True ($entry.GetValue('InstallLocation').TrimEnd('\') -eq $installDir) 'InstallLocation is wrong.'
    } finally { if ($entry) { $entry.Dispose() } }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($link in @((Join-Path $programDir ($fixtureName + '.lnk')), $desktopLink)) {
        Assert-True (Test-Path -LiteralPath $link -PathType Leaf) "Shortcut is missing: $link"
        $shortcut = $shell.CreateShortcut($link)
        Assert-True ($shortcut.TargetPath -eq $installedExe) 'Shortcut target differs from the installed program.'
        Assert-True ($shortcut.IconLocation -eq ((Join-Path $installDir 'TokWatch.ico') + ',0')) 'Shortcut icon is missing.'
    }
    $report.passed += 'install, shortcuts, Installed Apps registration, graceful portable exit, startup migration'
    $userFile = Join-Path $installDir 'user-created.txt'
    [IO.File]::WriteAllText($userFile, 'preserve this user file')
    $running = Start-Process -FilePath $installedExe -WindowStyle Hidden -PassThru
    Wait-Fixture
    Invoke-VerifiedUpdate $upgrade.Setup $running 'successful-upgrade'
    Assert-True $running.HasExited 'Upgrade did not gracefully close the installed fixture.'
    Assert-True ((Get-FileHash -LiteralPath $installedExe).Hash -eq (Get-FileHash -LiteralPath $upgrade.Binary).Hash) 'Upgrade did not replace executable bytes.'
    $entry = $registry.OpenSubKey($uninstallKey)
    try { Assert-True ($entry.GetValue('DisplayVersion') -eq '0.2.1') 'Upgrade did not update Installed Apps version.' }
    finally { if ($entry) { $entry.Dispose() } }
    $report.passed += 'verified installer update, exact-process handshake, upgrade registration, relaunch, special-character path, staged-file cleanup'
    $running = Installed-FixtureProcess
    Invoke-VerifiedUpdate $upgrade.Setup $running 'tampered-installer' -Tamper -ExpectFailure
    Assert-True ((Get-FileHash -LiteralPath $installedExe).Hash -eq (Get-FileHash -LiteralPath $upgrade.Binary).Hash) 'Tampered installer changed the app.'
    $report.passed += 'tampered installer rejected; existing app restarted intact'
    $failedSetup = Join-Path $fixtureRoot 'failed-setup.exe'
    & $clang -O2 -municode -mwindows '-DFIXTURE_EXIT_CODE=7' $source -o $failedSetup -luser32
    if ($LASTEXITCODE -ne 0) { throw 'Failed-setup fixture compilation failed.' }
    $running = Installed-FixtureProcess
    Invoke-VerifiedUpdate $failedSetup $running 'failed-setup' -ExpectFailure
    Assert-True ((Get-FileHash -LiteralPath $installedExe).Hash -eq (Get-FileHash -LiteralPath $upgrade.Binary).Hash) 'Setup failure did not restore the previous executable.'
    $report.passed += 'installer failure restored and restarted previous executable'
    $running = Installed-FixtureProcess
    $uninstaller = Join-Path $installDir 'unins000.exe'
    Invoke-Installer $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + (Join-Path $fixtureRoot 'uninstall.log') + '"'))
    Assert-True $running.HasExited 'Uninstall did not gracefully close the installed fixture.'
    Assert-True (-not (Test-Path -LiteralPath $installedExe)) 'Uninstall left the program executable.'
    Assert-True ($null -eq $registry.OpenSubKey($uninstallKey)) 'Uninstall left the Installed Apps entry.'
    Assert-True ($null -eq (Read-Startup)) 'Uninstall left its startup entry.'
    Assert-True (-not (Test-Path -LiteralPath $desktopLink)) 'Uninstall left its desktop shortcut.'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $programDir ($fixtureName + '.lnk')))) 'Uninstall left its Start menu shortcut.'
    Assert-True (([IO.File]::ReadAllText($sentinel)) -eq '{"keep":"user settings"}') 'User settings changed.'
    Assert-True (([IO.File]::ReadAllText($userFile)) -eq 'preserve this user file') 'User-created installation file was removed.'
    Assert-True (Test-Path -LiteralPath (Join-Path $dataDir 'cleanup-called.txt')) 'Uninstall did not invoke integration cleanup.'
    $report.passed += 'uninstall, graceful exit, startup removal, integration cleanup, user-file preservation'
    Invoke-Installer $initial.Setup ($setupArgs + ('/LOG="' + (Join-Path $fixtureRoot 'reinstall.log') + '"'))
    $unrelatedStartup = '"C:\An unrelated application\Example.exe"'
    Write-Startup $unrelatedStartup
    Invoke-Installer (Join-Path $installDir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
    Assert-True ((Read-Startup) -eq $unrelatedStartup) 'Uninstall changed an unrelated startup command.'
    $report.passed += 'unrelated startup command preserved'
    foreach ($appId in $normalApp) { Assert-True ($null -ne (Get-Process -Id $appId -ErrorAction SilentlyContinue)) 'Normal TokWatch was affected by installer fixtures.' }
    $report.passed += 'normal TokWatch process unaffected'
    $report | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $fixtureRoot 'result.json') -Encoding utf8
    Write-Output ($report | ConvertTo-Json -Depth 4)
}
finally {
    $window = [TokWatchInstallerFixtureNative]::FindWindow('TokWatch.Installer.Verification.Window', [IntPtr]::Zero)
    if ($window -ne [IntPtr]::Zero) { [TokWatchInstallerFixtureNative]::PostMessage($window, 0x111, [IntPtr]106, [IntPtr]::Zero) | Out-Null }
    $uninstaller = Join-Path $installDir 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        $cleanup = Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') -WindowStyle Hidden -Wait -PassThru
        if ($cleanup.ExitCode -ne 0) { Write-Warning "Fixture cleanup returned $($cleanup.ExitCode); inspect $fixtureRoot." }
    }
    $key = $registry.OpenSubKey($runKey, $true)
    try { if ($key) { $key.DeleteValue($startupName, $false) } } finally { if ($key) { $key.Dispose() } }
    [Environment]::SetEnvironmentVariable('TOKWATCH_INSTALLER_FIXTURE_DATA', $originalData, 'Process')
    [Environment]::SetEnvironmentVariable('TOKWATCH_UPDATE_FIXTURE', $originalUpdateFixture, 'Process')
    $registry.Dispose()
}
