[CmdletBinding()]
param(
    [ValidateSet('', 'x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu', 'x86_64-pc-windows-gnullvm')]
    [string]$Target = '',
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).Path

function Get-BytesHash {
    param([byte[]]$Bytes)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($hasher.ComputeHash($Bytes)).Replace('-', '').ToLowerInvariant() }
    finally { $hasher.Dispose() }
}

$manifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw -Encoding UTF8
$packageSection = [regex]::Match($manifest, '(?ms)^\[package\]\s*\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
$version = [regex]::Match($packageSection, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?)"\s*$').Groups[1].Value
if (-not $version) { throw 'Could not read a safe package version from Cargo.toml.' }

$licensePath = Join-Path $projectRoot 'LICENSE'
$readmePath = Join-Path $projectRoot 'docs\portable-readme.txt'
foreach ($source in @($licensePath, $readmePath)) {
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Required package file is missing: $source" }
}

$effectiveTarget = $Target
if (-not $effectiveTarget -and
    (Test-Path -LiteralPath (Join-Path $projectRoot '.tools\cargo\bin\cargo.exe')) -and
    (Test-Path -LiteralPath (Join-Path $projectRoot '.tools\llvm-mingw-20260826-ucrt-x86_64\bin\x86_64-w64-mingw32-clang.exe'))) {
    $effectiveTarget = 'x86_64-pc-windows-gnullvm'
}
& (Join-Path $PSScriptRoot 'build.ps1') -Task Verify -Configuration Release -Target $effectiveTarget

$releasePath = Join-Path $projectRoot 'target'
if ($effectiveTarget) { $releasePath = Join-Path $releasePath $effectiveTarget }
$exePath = Join-Path $releasePath 'release\tokwatch.exe'
if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) { throw "Release executable was not produced at $exePath" }

$artifacts = [ordered]@{
    'TokWatch.exe' = [IO.File]::ReadAllBytes($exePath)
    'LICENSE' = [IO.File]::ReadAllBytes($licensePath)
    'README.txt' = [IO.File]::ReadAllBytes($readmePath)
}
$exeBytes = $artifacts['TokWatch.exe']
if ($exeBytes.Length -lt 64 -or [BitConverter]::ToUInt16($exeBytes, 0) -ne 0x5A4D) {
    throw 'The release executable is not a Windows PE file.'
}
$peOffset = [BitConverter]::ToInt32($exeBytes, 0x3C)
if ($peOffset -lt 0 -or $peOffset -gt $exeBytes.Length - 6 -or
    [BitConverter]::ToUInt32($exeBytes, $peOffset) -ne 0x00004550 -or
    [BitConverter]::ToUInt16($exeBytes, $peOffset + 4) -ne 0x8664) {
    throw 'Only a Windows x64 executable can be packaged.'
}

# Fixed entry timestamps make identical package contents produce identical ZIPs.
Add-Type -AssemblyName System.IO.Compression
$zipName = "TokWatch-v$version-windows-x64.zip"
$buffer = [IO.MemoryStream]::new()
try {
    $archive = [IO.Compression.ZipArchive]::new($buffer, [IO.Compression.ZipArchiveMode]::Create, $true)
    try {
        foreach ($name in @('TokWatch.exe', 'LICENSE', 'README.txt')) {
            $entry = $archive.CreateEntry($name, [IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $stream = $entry.Open()
            try { $stream.Write($artifacts[$name], 0, $artifacts[$name].Length) }
            finally { $stream.Dispose() }
        }
    }
    finally { $archive.Dispose() }
    $artifacts[$zipName] = $buffer.ToArray()
}
finally { $buffer.Dispose() }
$checksums = "$(Get-BytesHash $exeBytes)  TokWatch.exe`r`n$(Get-BytesHash $artifacts[$zipName])  $zipName`r`n"
$artifacts['SHA256SUMS.txt'] = [Text.UTF8Encoding]::new($false).GetBytes($checksums)

$distPath = [IO.Path]::GetFullPath((Join-Path $projectRoot 'dist'))
$outputPath = [IO.Path]::GetFullPath((Join-Path $distPath "v$version"))
if (-not $outputPath.StartsWith($projectRoot.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Package destination must stay within the project.'
}
foreach ($directory in @($distPath, $outputPath)) {
    if (Test-Path -LiteralPath $directory) {
        $item = Get-Item -LiteralPath $directory -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Package destination must be an ordinary directory: $directory"
        }
    }
}

# Check every destination before writing, so a conflict leaves existing files intact.
$pending = @()
foreach ($name in $artifacts.Keys) {
    $destination = Join-Path $outputPath $name
    if (Test-Path -LiteralPath $destination) {
        $item = Get-Item -LiteralPath $destination -Force
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Package artifact must be an ordinary file: $destination"
        }
        if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -eq (Get-BytesHash $artifacts[$name])) {
            continue
        }
        if (-not $Force) { throw "A different artifact already exists: $destination. Use -Force to replace it intentionally." }
    }
    $pending += $name
}
[IO.Directory]::CreateDirectory($outputPath) | Out-Null
foreach ($name in $pending) {
    [IO.File]::WriteAllBytes((Join-Path $outputPath $name), $artifacts[$name])
}
Write-Output "Local release package: $outputPath"
Write-Output $checksums.TrimEnd()
