[CmdletBinding()]
param(
    [ValidateSet('Build', 'Check', 'Test', 'Lint', 'Format', 'Verify')]
    [string]$Task = 'Verify',
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [string]$Target = '',
    [string]$TestFilter = '',
    [switch]$IncludeIgnored
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (($TestFilter -or $IncludeIgnored) -and $Task -ne 'Test') {
    throw 'TestFilter and IncludeIgnored are only supported with -Task Test.'
}
$projectRoot = Split-Path -Parent $PSScriptRoot
$localCargo = Join-Path $projectRoot '.tools\cargo\bin\cargo.exe'
$savedEnvironment = @{}
foreach ($variableName in @('PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER')) {
    $savedEnvironment[$variableName] = [Environment]::GetEnvironmentVariable($variableName, 'Process')
}

function Invoke-Cargo {
    param([string[]]$Arguments)
    & $script:cargoExecutable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo failed with exit code $LASTEXITCODE."
    }
}

Push-Location $projectRoot
try {
    if (Test-Path -LiteralPath $localCargo) {
        $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
        $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
        $env:PATH = (Split-Path -Parent $localCargo) + [IO.Path]::PathSeparator + $env:PATH
        # Keep only dlltool and its runtime DLLs on PATH. Adding LLVM's full bin
        # directory would shadow Rust's GNU linker with an incompatible gcc shim.
        $localToolShim = Join-Path $projectRoot '.tools\toolshim'
        if (Test-Path -LiteralPath (Join-Path $localToolShim 'dlltool.exe')) {
            $env:PATH = $localToolShim + [IO.Path]::PathSeparator + $env:PATH
        }
        $llvmLinker = Join-Path $projectRoot '.tools\llvm-mingw-20260826-ucrt-x86_64\bin\x86_64-w64-mingw32-clang.exe'
        if (Test-Path -LiteralPath $llvmLinker) {
            if (-not $Target) { $Target = 'x86_64-pc-windows-gnullvm' }
            $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER = $llvmLinker
        }
        $script:cargoExecutable = $localCargo
    }
    else {
        $cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
        if ($null -eq $cargoCommand) {
            throw 'Rust is not installed. Install stable Rust and the Visual Studio C++ build tools, then run this script again.'
        }
        $script:cargoExecutable = $cargoCommand.Source
    }

    $targetArguments = @()
    if ($Target) {
        $targetArguments = @('--target', $Target)
    }
    $buildArguments = @('build') + $targetArguments
    if ($Configuration -eq 'Release') {
        $buildArguments += '--release'
    }

    $testArguments = @('test', '--all-targets') + $targetArguments
    if ($TestFilter) { $testArguments += $TestFilter }
    if ($IncludeIgnored) { $testArguments += @('--', '--ignored', '--nocapture') }

    switch ($Task) {
        'Build' { Invoke-Cargo -Arguments $buildArguments }
        'Check' { Invoke-Cargo -Arguments (@('check', '--all-targets') + $targetArguments) }
        'Test' { Invoke-Cargo -Arguments $testArguments }
        'Lint' { Invoke-Cargo -Arguments (@('clippy', '--all-targets') + $targetArguments + @('--', '-D', 'warnings')) }
        'Format' { Invoke-Cargo -Arguments @('fmt', '--all') }
        'Verify' {
            Invoke-Cargo -Arguments @('fmt', '--all', '--', '--check')
            Invoke-Cargo -Arguments (@('clippy', '--all-targets') + $targetArguments + @('--', '-D', 'warnings'))
            Invoke-Cargo -Arguments (@('test', '--all-targets') + $targetArguments)
            Invoke-Cargo -Arguments $buildArguments
        }
    }
}
finally {
    Pop-Location
    foreach ($variableName in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($variableName, $savedEnvironment[$variableName], 'Process')
    }
}
