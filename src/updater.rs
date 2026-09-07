//! Public GitHub release discovery and a verified Windows updater.
//! 0.x builds follow published numeric previews and stable releases; 1.x+ follows
//! stable releases only. GitHub/TLS and the repository owner are the trust
//! boundary; SHA-256 detects corruption, it is not a publisher signature.

use serde::Deserialize;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::core::{PCWSTR, w};

pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const RELEASES_URL: &str =
    "https://api.github.com/repos/MeysamResan/mxs-tokwatch/releases?per_page=100";
const DOWNLOAD_PREFIX: &str = "https://github.com/MeysamResan/mxs-tokwatch/releases/download/";
const MAX_EXE: usize = 32 * 1024 * 1024;
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
const MAX_ERROR: u64 = 16 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    pub version: String,
    staged: Arc<StagedUpdate>,
}

#[derive(Debug)]
struct StagedUpdate {
    target: PathBuf,
    path: PathBuf,
    sha256: String,
    target_sha256: String,
    handed_off: AtomicBool,
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        if !self.handed_off.load(Ordering::Acquire) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version(u32, u32, u32);

impl Version {
    fn parse(value: &str) -> Option<Self> {
        let value = value.strip_prefix('v').unwrap_or(value);
        let mut pieces = value.split('.');
        let mut number = || {
            let part = pieces.next()?;
            if part.is_empty()
                || part.len() > 9
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            part.parse::<u32>().ok()
        };
        let result = Self(number()?, number()?, number()?);
        pieces.next().is_none().then_some(result)
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
    #[serde(default)]
    digest: Option<String>,
}

fn asset<'a>(release: &'a Release, name: &str, max_size: usize) -> Option<&'a Asset> {
    let expected = format!("{DOWNLOAD_PREFIX}{}/{name}", release.tag_name);
    let mut matching = release.assets.iter().filter(|asset| asset.name == name);
    let first = matching.next()?;
    (matching.next().is_none()
        && first.state == "uploaded"
        && first.size > 0
        && first.size <= max_size as u64
        && first.browser_download_url == expected)
        .then_some(first)
}

fn setup_asset(release: &Release) -> Option<&Asset> {
    let name = format!("TokWatch-Setup-{}-windows-x64.exe", release.tag_name);
    let installer = asset(release, &name, MAX_EXE)?;
    parse_digest(installer.digest.as_deref()?).map(|_| installer)
}

fn parse_digest(value: &str) -> Option<String> {
    let hash = value.strip_prefix("sha256:")?;
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

fn newer_release(releases: &[Release], current: Version) -> Option<&Release> {
    releases
        .iter()
        .filter(|release| {
            !release.draft
                && release.published_at.as_ref().is_some_and(|s| !s.is_empty())
                && (current.0 == 0 || !release.prerelease)
                && release.tag_name.starts_with('v')
                && Version::parse(&release.tag_name).is_some_and(|v| v > current)
                && setup_asset(release).is_some()
        })
        .max_by_key(|release| Version::parse(&release.tag_name))
}

/// Run on a worker. Only public release metadata/assets are read, with no
/// credentials. A verified installer is staged beside the app, never launched here.
pub fn check_and_download() -> Result<Option<PreparedUpdate>, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("Automatic updates currently support Windows x64 only.".into());
    }
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or("This build does not use a supported numeric release version.")?;
    let manifest = get_https(RELEASES_URL, MAX_MANIFEST)?;
    let releases: Vec<Release> = serde_json::from_slice(&manifest)
        .map_err(|_| "GitHub returned an unreadable release list.".to_string())?;
    let Some(release) = newer_release(&releases, current) else {
        return Ok(None);
    };
    let installer = setup_asset(release).ok_or("The release installer is missing.")?;
    let expected = installer
        .digest
        .as_deref()
        .and_then(parse_digest)
        .ok_or("GitHub has not supplied a valid SHA-256 digest for this installer.")?;
    let contents = get_https(&installer.browser_download_url, MAX_EXE)?;
    if contents.len() as u64 != installer.size || !is_windows_installer(&contents) {
        return Err("The downloaded installer is not a complete Windows executable.".into());
    }
    if sha256(&contents)? != expected {
        return Err("Installer checksum mismatch. The downloaded file was discarded.".into());
    }
    let target = current_executable()?;
    let target_sha256 = sha256_file(&target)?;
    let directory = target
        .parent()
        .ok_or("The application folder is unavailable.")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Windows reported an invalid clock.")?
        .as_nanos();
    let path = directory.join(format!(
        ".TokWatch-update-{}-{}-{stamp}.exe",
        release.tag_name,
        std::process::id()
    ));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|e| format!("Cannot save an update beside TokWatch ({e}). Move TokWatch to a folder you can write to, then check again."))?;
    // Cleanup ownership starts only once create_new succeeds.
    let staged = Arc::new(StagedUpdate {
        target,
        path,
        sha256: expected,
        target_sha256,
        handed_off: AtomicBool::new(false),
    });
    file.write_all(&contents)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("Cannot save the update: {e}"))?;
    Ok(Some(PreparedUpdate {
        version: release.tag_name.clone(),
        staged,
    }))
}

fn is_windows_installer(contents: &[u8]) -> bool {
    if contents.len() < 64 || !contents.starts_with(b"MZ") {
        return false;
    }
    let offset = u32::from_le_bytes(contents[60..64].try_into().unwrap()) as usize;
    let Some(header_end) = offset.checked_add(24) else {
        return false;
    };
    if header_end > contents.len() || &contents[offset..offset + 4] != b"PE\0\0" {
        return false;
    }
    let machine = u16::from_le_bytes(contents[offset + 4..offset + 6].try_into().unwrap());
    let optional_size =
        u16::from_le_bytes(contents[offset + 20..offset + 22].try_into().unwrap()) as usize;
    let characteristics =
        u16::from_le_bytes(contents[offset + 22..offset + 24].try_into().unwrap());
    // Inno Setup can use a 32-bit bootstrapper for an x64-only application.
    // Packaging validates the payload architecture; the versioned asset name
    // and GitHub SHA-256 digest identify the intended Windows x64 installer.
    let (minimum, magic) = match machine {
        0x014c => (96, [0x0b, 0x01]),
        0x8664 => (112, [0x0b, 0x02]),
        _ => return false,
    };
    optional_size >= minimum
        && header_end
            .checked_add(optional_size)
            .is_some_and(|end| end <= contents.len())
        && contents.get(header_end..header_end + 2) == Some(&magic)
        && characteristics & 0x0002 != 0
        && characteristics & 0x2000 == 0
}

fn current_executable() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| format!("Cannot identify the running TokWatch executable: {e}"))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut contents = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_EXE as u64 + 1).read_to_end(&mut contents))
        .map_err(|e| format!("Cannot fingerprint the application executable: {e}"))?;
    if contents.len() > MAX_EXE {
        return Err("The application executable exceeded the update size limit.".into());
    }
    sha256(&contents)
}

fn sha256(contents: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    // Windows 11 supports the CNG SHA-256 pseudo-handle and one-shot hashing.
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, contents, &mut digest) }
        .ok()
        .map_err(|e| format!("Windows could not verify the update checksum: {e}"))?;
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn parse_https_url(url: &str) -> Result<(&str, &str), String> {
    if url.len() > 16 * 1024
        || !url.is_ascii()
        || url
            .bytes()
            .any(|b| b <= 0x20 || b == 0x7f || b == b'\\' || b == b'#')
    {
        return Err("GitHub returned an invalid update URL.".into());
    }
    let tail = url
        .strip_prefix("https://")
        .ok_or("Update URLs must use HTTPS.")?;
    let (host, suffix) = tail.split_once('/').ok_or("Invalid update URL.")?;
    if !matches!(
        host,
        "api.github.com"
            | "github.com"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
    ) {
        return Err("The update download redirected outside GitHub's approved hosts.".into());
    }
    Ok((host, &url[url.len() - suffix.len() - 1..]))
}

struct HttpHandle(*mut c_void);

impl HttpHandle {
    fn new(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(format!(
                "Cannot connect to GitHub: {}",
                windows::core::Error::from_thread()
            ))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for HttpHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn get_https(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let mut url = url.to_string();
    let agent = wide(concat!("TokWatch/", env!("CARGO_PKG_VERSION")));
    let headers: Vec<u16> =
        "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n"
            .encode_utf16()
            .collect();
    // Stage timeouts bound requests; redirects are validated before following.
    unsafe {
        let session = HttpHandle::new(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000).map_err(network_error)?;
        for _ in 0..5 {
            let (host, path) = parse_https_url(&url)?;
            let host = wide(host);
            let path = wide(path);
            let connection =
                HttpHandle::new(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0))?;
            let request = HttpHandle::new(WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ))?;
            WinHttpSetOption(
                Some(request.0),
                WINHTTP_OPTION_REDIRECT_POLICY,
                Some(&WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes()),
            )
            .map_err(network_error)?;
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(network_error)?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(network_error)?;
            let mut status = 0u32;
            let mut length = size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut length,
                std::ptr::null_mut(),
            )
            .map_err(network_error)?;
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let mut location = vec![0u16; 16 * 1024];
                let mut bytes = (location.len() * 2) as u32;
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_LOCATION,
                    PCWSTR::null(),
                    Some(location.as_mut_ptr().cast()),
                    &mut bytes,
                    std::ptr::null_mut(),
                )
                .map_err(network_error)?;
                let count = location
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(location.len());
                url = String::from_utf16(&location[..count])
                    .map_err(|_| "GitHub sent an invalid redirect URL.".to_string())?;
                if started.elapsed() > Duration::from_secs(120) {
                    return Err("The update request timed out. Try again later.".into());
                }
                continue;
            }
            if status != 200 {
                return Err(match status {
                    403 | 429 => {
                        "GitHub temporarily limited update checks. Try again later.".into()
                    }
                    404 => "The GitHub release is unavailable. Try again later.".into(),
                    _ => format!("GitHub update request failed (HTTP {status})."),
                });
            }
            let mut result = Vec::new();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                if started.elapsed() > Duration::from_secs(120) {
                    return Err("The update download timed out. Try again later.".into());
                }
                let mut received = 0u32;
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut received,
                )
                .map_err(network_error)?;
                if received == 0 {
                    return Ok(result);
                }
                if result.len().saturating_add(received as usize) > limit {
                    return Err("The update download exceeded its safe size limit.".into());
                }
                result.extend_from_slice(&buffer[..received as usize]);
            }
        }
    }
    Err("The update download redirected too many times.".into())
}

fn network_error(error: windows::core::Error) -> String {
    format!("Cannot download the GitHub update. Check your connection and try again ({error}).")
}

fn error_path() -> Result<PathBuf, String> {
    let directory = crate::settings::data_dir()?;
    fs::create_dir_all(&directory)
        .map_err(|e| format!("Cannot create the update status folder: {e}"))?;
    Ok(directory.join("update-error.txt"))
}

/// Consume a bounded message left by a failed installer on the previous run.
pub fn read_install_error() -> Option<String> {
    let path = error_path().ok()?;
    let mut result = String::new();
    File::open(&path)
        .ok()?
        .take(MAX_ERROR)
        .read_to_string(&mut result)
        .ok()?;
    let _ = fs::remove_file(path);
    (!result.trim().is_empty()).then(|| result.trim().to_string())
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn script_path(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or("The application path cannot be represented in PowerShell.")?;
    // Windows PowerShell's .NET Framework inconsistently handles this prefix.
    let text = text.strip_prefix("\\\\?\\").unwrap_or(text);
    if text.starts_with("UNC\\")
        || text.starts_with("\\\\")
        || !path.is_absolute()
        || text.contains(['\r', '\n', '\0'])
    {
        return Err("Automatic replacement requires a local Windows application path.".into());
    }
    Ok(single_quote(text))
}

fn creation_time() -> Result<u64, String> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .map_err(|e| format!("Cannot identify this app process for the updater: {e}"))?;
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

fn installer_script(
    update: &PreparedUpdate,
    ready: &Path,
    error: &Path,
    parent_pid: u32,
    created: u64,
) -> Result<String, String> {
    let target = script_path(&update.staged.target)?;
    let staged = script_path(&update.staged.path)?;
    let backup = script_path(&update.staged.path.with_extension("backup.exe"))?;
    let ready = script_path(ready)?;
    let error = script_path(error)?;
    let hash = single_quote(&update.staged.sha256);
    let target_hash = single_quote(&update.staged.target_sha256);
    Ok(format!(
        r#"
$ErrorActionPreference = 'Stop'
# Use Windows PowerShell's own modules even when launched from PowerShell 7.
$env:PSModulePath = [IO.Path]::Combine($PSHOME, 'Modules')
$target = {target}
$staged = {staged}
$backup = {backup}
$ready = {ready}
$errorPath = {error}
$expectedHash = {hash}
$expectedTargetHash = {target_hash}
$replaced = $false
$parentClosed = $false
try {{
    $parent = Get-Process -Id {parent_pid} -ErrorAction Stop
    if ($parent.Path -ine $target -or $parent.StartTime.ToFileTimeUtc() -ne {created}) {{
        throw 'The running TokWatch process changed; installation was cancelled.'
    }}
    $null = $parent.Handle
    [IO.File]::WriteAllText($ready, 'READY')
    if (-not $parent.WaitForExit(120000)) {{ throw 'TokWatch did not close within two minutes. Try Restart to update again.' }}
    $parentClosed = $true
    if ((Get-Item -LiteralPath $staged -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {{ throw 'The staged update path changed.' }}
    if ((Get-Item -LiteralPath $target -Force).Length -gt 33554432 -or (Get-Item -LiteralPath $staged -Force).Length -gt 33554432) {{ throw 'An update executable exceeded the allowed size.' }}
    if ((Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash -ine $expectedHash) {{ throw 'The staged update checksum changed; installation was cancelled.' }}
    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ine $expectedTargetHash) {{ throw 'TokWatch was replaced while this update was pending. Your existing EXE was preserved.' }}
    [IO.File]::Copy($target, $backup, $false)
    $replaced = $true
    $folder = [IO.Path]::GetDirectoryName($target)
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ('/DIR="' + $folder + '"'))
    $setup = Start-Process -FilePath $staged -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($setup.ExitCode -ne 0) {{ throw ('Setup failed with exit code ' + $setup.ExitCode + '.') }}
    if (-not [IO.File]::Exists($target)) {{ throw 'Setup did not install TokWatch.' }}
    $app = Start-Process -FilePath $target -WorkingDirectory ([IO.Path]::GetDirectoryName($target)) -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 750
    if ($app.HasExited) {{ throw 'The updated app could not start; the previous version will be restored.' }}
    Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
}} catch {{
    $message = 'TokWatch update failed: ' + $_.Exception.Message
    if ($replaced -and [IO.File]::Exists($backup)) {{
        try {{
            [IO.File]::Replace($backup, $target, $null, $true)
            $message += ' The previous version was restored.'
        }} catch {{
            $message += ' Restore failed. Your previous EXE is preserved at ' + $backup + '. ' + $_.Exception.Message
        }}
    }}
    try {{ [IO.File]::WriteAllText($errorPath, $message) }} catch {{ }}
    if ($parentClosed -and [IO.File]::Exists($target)) {{
        try {{
            if ((Get-Item -LiteralPath $target -Force).Length -le 33554432 -and (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ieq $expectedTargetHash) {{
                Start-Process -FilePath $target -WorkingDirectory ([IO.Path]::GetDirectoryName($target)) -WindowStyle Hidden
            }}
        }} catch {{ }}
    }}
}} finally {{
    Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $ready -Force -ErrorAction SilentlyContinue
}}
"#
    ))
}

/// Start the hidden helper and wait for its process-identity handshake. The UI
/// must exit only when this returns Ok. The helper waits for this exact process
/// handle before running Setup in the existing folder. Setup updates installed
/// files and registration; a failed setup/relaunch restores the previous EXE.
pub fn launch_installer(update: &PreparedUpdate) -> Result<(), String> {
    let current = Version::parse(env!("CARGO_PKG_VERSION")).ok_or("Invalid app version.")?;
    if !Version::parse(&update.version).is_some_and(|version| version > current) {
        return Err("This update is no longer newer than the running app.".into());
    }
    if update.staged.target != current_executable()?
        || update.staged.path.parent() != update.staged.target.parent()
        || !update
            .staged
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".TokWatch-update-"))
    {
        return Err(
            "The application or staged update path changed. Check for updates again.".into(),
        );
    }
    if sha256_file(&update.staged.target)? != update.staged.target_sha256 {
        return Err("TokWatch was replaced while this update was pending. Restart the app before checking again.".into());
    }

    let info = fs::symlink_metadata(&update.staged.path)
        .map_err(|e| format!("The staged update is unavailable: {e}"))?;
    if !info.file_type().is_file() || info.file_type().is_symlink() || info.len() > MAX_EXE as u64 {
        return Err("The staged update is not a regular executable file.".into());
    }
    let mut contents = Vec::new();
    File::open(&update.staged.path)
        .and_then(|file| file.take(MAX_EXE as u64 + 1).read_to_end(&mut contents))
        .map_err(|e| format!("Cannot verify the staged update: {e}"))?;
    if contents.len() > MAX_EXE
        || !is_windows_installer(&contents)
        || sha256(&contents)? != update.staged.sha256
    {
        return Err("The staged update checksum changed. Check for updates again.".into());
    }
    let error = error_path()?;
    let ready = update.staged.path.with_extension("ready");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&ready)
        .map_err(|e| format!("Cannot prepare the update in the app folder: {e}"))?;
    let result = (|| {
        let script =
            installer_script(update, &ready, &error, std::process::id(), creation_time()?)?;
        let system_root =
            std::env::var_os("SystemRoot").ok_or("Windows system folder is unavailable.")?;
        let powershell =
            PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot start the Windows update helper: {e}"))?;
        let started = Instant::now();
        loop {
            let mut message = String::new();
            let _ = File::open(&ready).and_then(|file| file.take(16).read_to_string(&mut message));
            if message == "READY" {
                update.staged.handed_off.store(true, Ordering::Release);
                return Ok(());
            }
            if child
                .try_wait()
                .map_err(|e| format!("Cannot check the update helper: {e}"))?
                .is_some()
            {
                return Err(read_install_error().unwrap_or_else(|| "The Windows update helper did not start. Check folder permissions or try downloading the release manually.".into()));
            }
            if started.elapsed() > Duration::from_secs(8) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The Windows update helper did not become ready. TokWatch is still running; try again.".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(ready);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease,
            published_at: Some("2026-09-07T00:00:00Z".into()),
            assets: vec![Asset {
                name: format!("TokWatch-Setup-{tag}-windows-x64.exe"),
                browser_download_url: format!(
                    "{DOWNLOAD_PREFIX}{tag}/TokWatch-Setup-{tag}-windows-x64.exe"
                ),
                size: 100,
                state: "uploaded".into(),
                digest: Some(format!("sha256:{}", "a".repeat(64))),
            }],
        }
    }

    #[test]
    fn version_ordering_is_numeric_and_rejects_ambiguous_tags() {
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.9"));
        for bad in [
            "v1.2",
            "1.2.3.4",
            "v01.2.3",
            "1.2.-1",
            "1.2.3-beta",
            "1.2.3+build",
            "4294967296.0.0",
            "1.2.3 ",
            "v1.2.３",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn selection_never_downgrades_and_has_explicit_preview_policy() {
        let releases = vec![
            release("v0.1.0", true),
            release("v0.4.0", true),
            release("v0.3.0", false),
        ];
        assert_eq!(
            newer_release(&releases, Version(0, 2, 0)).unwrap().tag_name,
            "v0.4.0"
        );
        assert!(newer_release(&releases, Version(0, 4, 0)).is_none());
        let releases = vec![release("v2.0.0", true), release("v1.1.0", false)];
        assert_eq!(
            newer_release(&releases, Version(1, 0, 0)).unwrap().tag_name,
            "v1.1.0"
        );
    }

    #[test]
    fn ignores_incomplete_draft_and_foreign_assets() {
        let mut releases = vec![
            release("v0.8.0", true),
            release("v0.7.0", true),
            release("v0.6.0", true),
            release("v0.5.0", false),
        ];
        releases[0].draft = true;
        releases[1].assets.pop();
        releases[2].assets[0].browser_download_url =
            "https://github.com/other/repo/evil.exe".into();
        assert_eq!(
            newer_release(&releases, Version(0, 2, 0)).unwrap().tag_name,
            "v0.5.0"
        );
    }

    #[test]
    fn installer_requires_exact_name_and_github_sha256_digest() {
        let expected = "a".repeat(64);
        assert_eq!(
            parse_digest(&format!("sha256:{}", expected.to_uppercase())),
            Some(expected.clone())
        );
        for bad in [
            expected.clone(),
            format!("md5:{expected}"),
            "sha256:abc".into(),
            format!("sha256:{}", "z".repeat(64)),
            format!("sha256:{expected}\n"),
        ] {
            assert!(parse_digest(&bad).is_none());
        }
        let mut r = release("v0.3.0", false);
        assert!(setup_asset(&r).is_some());
        r.assets[0].digest = None;
        assert!(setup_asset(&r).is_none());
        r.assets[0].digest = Some(format!("sha256:{expected}"));
        r.assets[0].name = "TokWatch.exe".into();
        assert!(setup_asset(&r).is_none());
        let mut r = release("v0.3.0", false);
        r.assets.push(release("v0.3.0", false).assets.remove(0));
        assert!(setup_asset(&r).is_none());
    }

    #[test]
    fn windows_hash_matches_standard_sha256_vectors() {
        assert_eq!(
            sha256(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn redirects_are_https_and_exact_github_hosts_only() {
        assert!(
            parse_https_url("https://release-assets.githubusercontent.com/a?sig=abc%2F").is_ok()
        );
        for bad in [
            "http://github.com/a",
            "https://github.com.evil.test/a",
            "https://github.com@evil.test/a",
            "https://github.com:443/a",
            "https://evil.test/a",
            "https://github.com\\evil/a",
            "https://github.com/a\r\nX: bad",
        ] {
            assert!(parse_https_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn installer_quotes_paths_and_checks_exact_process_and_checksum() {
        let update = PreparedUpdate {
            version: "v99.0.0".into(),
            staged: Arc::new(StagedUpdate {
                target: PathBuf::from("C:\\Users\\O'Brien $test\\TokWatch.exe"),
                path: PathBuf::from("C:\\Users\\O'Brien $test\\.TokWatch-update-fixture.exe"),
                sha256: "a".repeat(64),
                target_sha256: "b".repeat(64),
                handed_off: AtomicBool::new(true),
            }),
        };
        let script = installer_script(
            &update,
            Path::new("C:\\fixture\\ready"),
            Path::new("C:\\fixture\\error"),
            std::process::id(),
            creation_time().unwrap(),
        )
        .unwrap();
        assert!(script.contains("O''Brien $test"));
        assert!(script.contains("$parent.StartTime.ToFileTimeUtc()"));
        assert!(script.contains("$parent.WaitForExit(120000)"));
        assert!(script.contains("Get-FileHash -LiteralPath $staged -Algorithm SHA256"));
        assert!(script.contains("Get-FileHash -LiteralPath $target -Algorithm SHA256"));
        assert!(script.contains("[IO.File]::Replace($backup, $target, $null, $true)"));
        assert!(script.contains("Start-Process -FilePath $staged -ArgumentList $arguments"));
        assert!(script.contains("'/VERYSILENT'"));
        assert!(script.contains("$setup.ExitCode -ne 0"));
        assert!(!script.contains("[IO.File]::Replace($staged, $target"));
        assert!(!script.contains("ExecutionPolicy"));
        assert!(!script.contains("-Verb RunAs"));
    }
    #[test]
    fn helper_parses_in_windows_powershell_without_executing() {
        let update = PreparedUpdate {
            version: "v99.0.0".into(),
            staged: Arc::new(StagedUpdate {
                target: PathBuf::from("C:\\Users\\O'Brien $test\\TokWatch.exe"),
                path: PathBuf::from("C:\\Users\\O'Brien $test\\.TokWatch-update-fixture.exe"),
                sha256: "a".repeat(64),
                target_sha256: "b".repeat(64),
                handed_off: AtomicBool::new(true),
            }),
        };
        let script = installer_script(
            &update,
            Path::new("C:\\fixture\\ready"),
            Path::new("C:\\fixture\\error"),
            std::process::id(),
            creation_time().unwrap(),
        )
        .unwrap();
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(powershell).args([
            "-NoLogo", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command",
            "$source = [Console]::In.ReadToEnd(); $tokens = $null; $errors = $null; $null = [Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors); if ($errors.Count) { $errors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }; exit 1 }"
        ]).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "exports the production helper for scripts/test-installer.ps1 fixtures"]
    fn export_installer_helper_fixture() {
        #[derive(Deserialize)]
        struct Fixture {
            target: PathBuf,
            staged: PathBuf,
            ready: PathBuf,
            error: PathBuf,
            output: PathBuf,
            parent_pid: u32,
            created: u64,
        }
        let path = std::env::var_os("TOKWATCH_UPDATE_FIXTURE").expect("fixture manifest");
        let fixture: Fixture = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let update = PreparedUpdate {
            version: "v99.0.0".into(),
            staged: Arc::new(StagedUpdate {
                sha256: sha256_file(&fixture.staged).unwrap(),
                target_sha256: sha256_file(&fixture.target).unwrap(),
                target: fixture.target,
                path: fixture.staged,
                handed_off: AtomicBool::new(true),
            }),
        };
        let script = installer_script(
            &update,
            &fixture.ready,
            &fixture.error,
            fixture.parent_pid,
            fixture.created,
        )
        .unwrap();
        fs::write(fixture.output, script).unwrap();
    }

    #[test]
    #[ignore = "reads public GitHub release metadata and assets; no staging or installation"]
    fn live_github_assets_pass_https_and_sha256_verification() {
        let manifest = get_https(RELEASES_URL, MAX_MANIFEST).unwrap();
        let releases: Vec<Release> = serde_json::from_slice(&manifest).unwrap();
        let release = newer_release(&releases, Version(0, 0, 0))
            .expect("At least one published numeric TokWatch release with the required assets");
        let installer = setup_asset(release).unwrap();
        let expected = parse_digest(installer.digest.as_deref().unwrap()).unwrap();
        let contents = get_https(&installer.browser_download_url, MAX_EXE).unwrap();
        assert_eq!(contents.len() as u64, installer.size);
        assert!(is_windows_installer(&contents));
        assert_eq!(sha256(&contents).unwrap(), expected);
    }
    #[test]
    fn installer_accepts_x86_or_x64_bootstrapper_and_rejects_invalid_pe() {
        let mut bytes = vec![0u8; 256];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[84..86].copy_from_slice(&112u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&0x0002u16.to_le_bytes());
        bytes[88..90].copy_from_slice(&0x020bu16.to_le_bytes());
        assert!(is_windows_installer(&bytes));
        bytes[68..70].copy_from_slice(&0x014cu16.to_le_bytes());
        bytes[88..90].copy_from_slice(&0x010bu16.to_le_bytes());
        assert!(
            is_windows_installer(&bytes),
            "Accept a 32-bit installer bootstrapper"
        );
        bytes[88..90].copy_from_slice(&0x020bu16.to_le_bytes());
        assert!(
            !is_windows_installer(&bytes),
            "PE machine and optional header must agree"
        );
        bytes[68..70].copy_from_slice(&0xaa64u16.to_le_bytes());
        assert!(!is_windows_installer(&bytes), "Reject ARM64");
        bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&0x2002u16.to_le_bytes());
        assert!(!is_windows_installer(&bytes), "Reject DLL");
        bytes[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            !is_windows_installer(&bytes),
            "Reject out-of-bounds PE header"
        );
        assert!(!is_windows_installer(b"MZ"));
    }
}
