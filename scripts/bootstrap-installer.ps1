[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).Path
$toolsRoot = Join-Path $projectRoot '.tools'
$version = '6.7.3'
$expectedHash = '9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732'
$download = Join-Path $toolsRoot "innosetup-$version.exe"
$compilerRoot = Join-Path $toolsRoot "innosetup-$version"
$compiler = Join-Path $compilerRoot 'ISCC.exe'
$compilerHash = '0a8757031b33777e4c9cbffee40f11a5062b36d25cbe144c1db73b6102b80ad7'

foreach ($directory in @($toolsRoot, $compilerRoot)) {
    if (Test-Path -LiteralPath $directory) {
        $item = Get-Item -LiteralPath $directory -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Compiler destination must be an ordinary project directory: $directory"
        }
    }
}
if (Test-Path -LiteralPath $compiler -PathType Leaf) {
    $item = Get-Item -LiteralPath $compiler -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Compiler must not be a reparse point.' }
    if ((Get-FileHash -LiteralPath $compiler -Algorithm SHA256).Hash.ToLowerInvariant() -ne $compilerHash) { throw "Expected Inno Setup $version in $compilerRoot." }
    Write-Output $compiler
    return
}

[IO.Directory]::CreateDirectory($toolsRoot) | Out-Null
if (-not (Test-Path -LiteralPath $download -PathType Leaf)) {
    Write-Host "Downloading official Inno Setup $version to the local .tools directory..."
    Invoke-WebRequest -Uri "https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-$version.exe" -OutFile $download
}
$item = Get-Item -LiteralPath $download -Force
if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Compiler download must not be a reparse point.' }
if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) {
    throw 'Inno Setup download failed the pinned SHA-256 verification.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $download
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|, )CN=Pyrsys B\.V\.(,|$)') {
    throw 'Inno Setup download must have a valid Pyrsys B.V. Authenticode signature.'
}

# Inno Setup's official portable mode avoids registration, shortcuts, and file associations.
$arguments = @('/PORTABLE=1', '/CURRENTUSER', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NOICONS', '/TASKS=', ('/DIR="' + $compilerRoot + '"'))
$process = Start-Process -FilePath $download -ArgumentList $arguments -WindowStyle Hidden -PassThru -Wait
if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
    throw "Portable Inno Setup extraction failed with exit code $($process.ExitCode)."
}
if ((Get-FileHash -LiteralPath $compiler -Algorithm SHA256).Hash.ToLowerInvariant() -ne $compilerHash) { throw 'Extracted compiler has an unexpected version.' }
Write-Output $compiler
