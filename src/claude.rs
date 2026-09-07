//! Claude subscription usage through Claude Code's documented status line.
//!
//! Claude Code owns authentication and supplies rate-limit JSON on stdin. Only
//! numeric usage fields reach disk; no credentials, prompts, transcript paths,
//! account identifiers, or context-window token counts are retained.

use crate::model::{UsagePool, UsageSnapshot, UsageWindow, unix_now};
use crate::settings::data_dir;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 16 * 1024;
const CACHE_FILE: &str = "claude-statusline.json";
const MANUAL_SETUP_FILE: &str = "claude-statusline-settings.json";
const NO_READING: &str = "Connect Claude Code, then use it with your Claude Pro or Max account. Usage appears after its first response.";
static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize, Serialize)]
struct BridgeReading {
    version: u8,
    received_at: u64,
    five_hour: Option<BridgeWindow>,
    seven_day: Option<BridgeWindow>,
}

#[derive(Deserialize, Serialize)]
struct BridgeWindow {
    used_percentage: Option<f64>,
    resets_at: Option<u64>,
}

impl BridgeReading {
    fn from_statusline(value: &Value, now: u64) -> Result<Self, String> {
        if !value.is_object() {
            return Err("Claude Code supplied an invalid status line payload.".into());
        }
        let limits = value.get("rate_limits");
        Ok(Self {
            version: 1,
            received_at: now,
            five_hour: limits.and_then(|value| parse_window(value.get("five_hour"))),
            seven_day: limits.and_then(|value| parse_window(value.get("seven_day"))),
        })
    }

    fn snapshot(&self, now: u64) -> Result<UsageSnapshot, String> {
        if self.version != 1 || self.received_at == 0 || self.received_at > now.saturating_add(60) {
            return Err(
                "Claude usage cache is invalid. Use Claude Code to send a fresh reading.".into(),
            );
        }
        let windows = [
            ("five_hour", 300, self.five_hour.as_ref()),
            ("seven_day", 10080, self.seven_day.as_ref()),
        ]
        .into_iter()
        .map(|(kind, duration, source)| {
            // A known reset is no longer a current usage reading. Do not invent
            // a refreshed allowance while Claude Code is closed or idle.
            let expired = source
                .and_then(|window| window.resets_at)
                .is_some_and(|reset| reset <= now);
            UsageWindow {
                key: format!("claude:{kind}"),
                kind: kind.into(),
                used_percent: source
                    .and_then(|window| window.used_percentage)
                    .filter(|value| !expired && valid_percentage(*value)),
                duration_mins: Some(duration),
                resets_at: source
                    .and_then(|window| window.resets_at)
                    .filter(|reset| !expired && *reset > 0),
            }
        })
        .collect();
        Ok(UsageSnapshot {
            // This is the time of a local Claude Code emission, not an
            // independent account-service request by TokWatch.
            fetched_at: self.received_at,
            pools: vec![UsagePool {
                id: "claude".into(),
                name: "Claude".into(),
                windows,
                plan: None,
                credits: None,
            }],
            reset_credits: None,
        })
    }
}

fn valid_percentage(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

fn parse_window(value: Option<&Value>) -> Option<BridgeWindow> {
    let object = value?.as_object()?;
    let used_percentage = object
        .get("used_percentage")
        .and_then(Value::as_f64)
        .filter(|value| valid_percentage(*value));
    let resets_at = object
        .get("resets_at")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0);
    (used_percentage.is_some() || resets_at.is_some()).then_some(BridgeWindow {
        used_percentage,
        resets_at,
    })
}

/// Poll only the local status line bridge. Never starts Claude or sends a prompt.
pub fn read_limits() -> Result<UsageSnapshot, String> {
    let bytes = read_bounded(&data_dir()?.join(CACHE_FILE), MAX_CACHE_BYTES)?
        .ok_or_else(|| NO_READING.to_string())?;
    let reading: BridgeReading = serde_json::from_slice(&bytes).map_err(|_| {
        "Claude usage cache is invalid. Use Claude Code to send a fresh reading.".to_string()
    })?;
    reading.snapshot(unix_now())
}

/// Call before creating any window, acquiring the tray mutex, or starting workers.
pub fn run_statusline() -> Result<(), String> {
    let reading = receive_statusline(std::io::stdin().lock(), unix_now())?;
    let directory = data_dir()?;
    fs::create_dir_all(&directory)
        .map_err(|_| "Could not create TokWatch's local Claude usage folder.".to_string())?;
    let bytes = serde_json::to_vec(&reading)
        .map_err(|_| "Could not prepare the Claude usage reading.".to_string())?;
    atomic_write(&directory.join(CACHE_FILE), &bytes)?;
    let snapshot = reading.snapshot(unix_now())?;
    let segments: Vec<String> = snapshot.pools[0]
        .windows
        .iter()
        .filter_map(|window| {
            let remaining = window.remaining_percent()?;
            let label = if window.kind == "five_hour" {
                "5h"
            } else {
                "7d"
            };
            Some(format!("{label}: {remaining}% left"))
        })
        .collect();
    let text = if segments.is_empty() {
        "Claude usage: waiting for a subscription reading".into()
    } else {
        format!("Claude | {}", segments.join(" | "))
    };
    // Claude Code captures stdout. The main tray process remains a GUI app.
    let _ = writeln!(std::io::stdout().lock(), "{text}");
    Ok(())
}

fn receive_statusline(reader: impl Read, now: u64) -> Result<BridgeReading, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read Claude Code's status line payload.".to_string())?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("Claude Code supplied an oversized status line payload.".into());
    }
    // Windows PowerShell may prepend a UTF-8 BOM when piping a native program.
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Claude Code supplied an invalid status line payload.".to_string())?;
    BridgeReading::from_statusline(&value, now)
}

/// Adds the documented global statusLine setting. Existing unrelated settings
/// remain intact; reconnecting may relocate our exact generated bridge, but a
/// customized or unrelated status line is never replaced automatically.
pub fn connect() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| {
        "Could not locate TokWatch. Keep the EXE in a permanent folder and retry.".to_string()
    })?;
    let config = config_dir()?;
    let target = config.join("settings.json");
    let original = read_bounded(&target, MAX_INPUT_BYTES)?;
    let mut settings: Value = match original.as_deref() {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| {
            "Claude Code settings contain invalid JSON. Repair them in Claude Code, then retry."
                .to_string()
        })?,
        None => json!({}),
    };
    if !settings.is_object() {
        return Err(
            "Claude Code settings must be a JSON object. Repair them in Claude Code, then retry."
                .into(),
        );
    }
    let previous_settings = settings.clone();
    let command = bridge_command(&executable)?;
    if merge_statusline(&mut settings, &command).is_err() {
        let directory = data_dir()?;
        fs::create_dir_all(&directory)
            .map_err(|_| "Could not create TokWatch's Claude setup instructions.".to_string())?;
        let snippet = json!({"statusLine": {"type": "command", "command": command}});
        let bytes = serde_json::to_vec_pretty(&snippet)
            .map_err(|_| "Could not prepare Claude setup instructions.".to_string())?;
        atomic_write(&directory.join(MANUAL_SETUP_FILE), &bytes)?;
        return Err(format!(
            "Claude Code already has a status line. It was preserved. To use TokWatch's status line, merge {} into your Claude Code user settings; save your existing statusLine first.",
            directory.join(MANUAL_SETUP_FILE).display()
        ));
    }
    if settings == previous_settings {
        return Ok(());
    }
    save_configuration(&config, &target, original.as_deref(), &settings)
}

/// Uninstall removes only this executable's exact bridge. Other status lines,
/// provider authentication, and cached TokWatch settings remain untouched.
pub fn uninstall_integration() -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|_| "Could not locate TokWatch while removing its Claude bridge.".to_string())?;
    let command = bridge_command(&executable)?;
    let config = config_dir()?;
    let target = config.join("settings.json");
    let Some(original) = read_bounded(&target, MAX_INPUT_BYTES)? else {
        return Ok(());
    };
    let mut settings: Value = serde_json::from_slice(&original)
        .map_err(|_| "Claude Code settings contain invalid JSON and were preserved.".to_string())?;
    if remove_statusline(&mut settings, &command) {
        save_configuration(&config, &target, Some(&original), &settings)?;
    }
    Ok(())
}

fn remove_statusline(settings: &mut Value, command: &str) -> bool {
    if settings["statusLine"]["type"].as_str() == Some("command")
        && settings["statusLine"]["command"].as_str() == Some(command)
    {
        settings.as_object_mut().unwrap().remove("statusLine");
        true
    } else {
        false
    }
}

fn save_configuration(
    config: &Path,
    target: &Path,
    original: Option<&[u8]>,
    settings: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|_| "Could not prepare Claude Code settings.".to_string())?;
    fs::create_dir_all(config)
        .map_err(|_| "Could not create the Claude Code settings folder.".to_string())?;
    if let Some(original) = original {
        // A unique full backup allows recovery without replacing an older one.
        write_unique(config, "settings.tokwatch-backup", original)?;
    }
    if read_bounded(target, MAX_INPUT_BYTES)?.as_deref() != original {
        return Err("Claude Code settings changed during setup. Retry the operation.".into());
    }
    atomic_write(target, &bytes)
}

/// Indicates only that this executable's bridge is installed in user settings.
/// Claude Code project or managed settings can still override the user setting.
pub fn is_configured() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let Ok(command) = bridge_command(&executable) else {
        return false;
    };
    let Ok(config) = config_dir() else {
        return false;
    };
    let Ok(Some(bytes)) = read_bounded(&config.join("settings.json"), MAX_INPUT_BYTES) else {
        return false;
    };
    let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    settings["statusLine"]["type"].as_str() == Some("command")
        && settings["statusLine"]["command"].as_str() == Some(command.as_str())
}

fn config_dir() -> Result<PathBuf, String> {
    if let Some(custom) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(custom);
        return path.is_absolute().then_some(path).ok_or_else(|| {
            "CLAUDE_CONFIG_DIR must be an absolute path before connecting Claude Code.".into()
        });
    }
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".claude"))
        .ok_or_else(|| "Windows did not provide a home folder for Claude Code settings.".into())
}

fn merge_statusline(settings: &mut Value, command: &str) -> Result<(), String> {
    let object = settings
        .as_object_mut()
        .ok_or_else(|| "Claude Code settings must be a JSON object.".to_string())?;
    if let Some(existing) = object
        .get_mut("statusLine")
        .filter(|value| !value.is_null())
    {
        if existing.get("type").and_then(Value::as_str) == Some("command") {
            let previous = existing.get("command").and_then(Value::as_str);
            if previous == Some(command) {
                return Ok(());
            }
            if previous.is_some_and(is_generated_bridge) {
                // Moving the portable app into its installed location only
                // changes our own command. Keep padding and other user fields.
                existing["command"] = json!(command);
                return Ok(());
            }
        }
        return Err("An existing Claude Code status line was preserved.".into());
    }
    object.insert(
        "statusLine".into(),
        json!({"type": "command", "command": command}),
    );
    Ok(())
}

fn bridge_command(executable: &Path) -> Result<String, String> {
    let path = executable
        .to_str()
        .filter(|value| !value.chars().any(char::is_control))
        .ok_or_else(|| {
            "TokWatch's folder name cannot be used in Claude Code settings.".to_string()
        })?;
    if !executable.is_absolute() {
        return Err("TokWatch must be installed at an absolute path.".into());
    }
    // An encoded PowerShell command works under either documented Windows
    // status-line shell (Git Bash or PowerShell), including paths with spaces,
    // quotes, dollars, ampersands, and backticks. No raw path enters that shell.
    let literal_path = path.replace('\\', "/").replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; [Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::In.ReadToEnd() | & '{literal_path}' --claude-statusline"
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    Ok(format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
        base64(&utf16)
    ))
}

// Recognize only an exact round trip of the launcher we generated. Merely
// mentioning TokWatch or --claude-statusline must not claim a custom command.
fn is_generated_bridge(command: &str) -> bool {
    let Some(encoded) =
        command.strip_prefix("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
    else {
        return false;
    };
    let Some(bytes) = decode_base64(encoded) else {
        return false;
    };
    if bytes.len() % 2 != 0 {
        return false;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let Ok(script) = String::from_utf16(&units) else {
        return false;
    };
    let Some(literal) = script
        .strip_prefix("$ErrorActionPreference='Stop'; [Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::In.ReadToEnd() | & '")
        .and_then(|text| text.strip_suffix("' --claude-statusline"))
    else {
        return false;
    };
    let path = literal.replace("''", "'");
    let path = Path::new(&path);
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("TokWatch.exe"))
        && bridge_command(path).is_ok_and(|expected| expected == command)
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if encoded.is_empty() || encoded.len() > 32 * 1024 || encoded.len() % 4 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 4 * 3);
    for group in encoded.as_bytes().chunks_exact(4) {
        let mut value = 0u32;
        for character in group {
            let digit = if *character == b'=' {
                0
            } else {
                ALPHABET.iter().position(|entry| entry == character)? as u32
            };
            value = (value << 6) | digit;
        }
        bytes.extend_from_slice(&value.to_be_bytes()[1..]);
    }
    bytes.truncate(
        bytes
            .len()
            .checked_sub(encoded.len() - encoded.trim_end_matches('=').len())?,
    );
    // Reject misplaced padding, noncanonical trailing bits, and extra padding.
    (base64(&bytes) == encoded).then_some(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[((first & 3) << 4 | second >> 4) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((second & 15) << 2 | third >> 6) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn read_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err("Could not read local Claude Code configuration or usage data.".into());
        }
    };
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read local Claude Code configuration or usage data.".to_string())?;
    if bytes.len() as u64 > limit {
        return Err(
            "Local Claude Code configuration or usage data is larger than expected.".into(),
        );
    }
    Ok(Some(bytes))
}

fn write_unique(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    loop {
        let path = directory.join(format!(
            ".{name}.{}.{}.json",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err("Could not save local Claude Code configuration or usage data.".into());
            }
        };
        if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err("Could not save local Claude Code configuration or usage data.".into());
        }
        return Ok(path);
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "Invalid Claude data location.".to_string())?;
    let temporary = write_unique(directory, "tokwatch-write", bytes)?;
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Could not replace local Claude Code configuration or usage data.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn subscription_windows_are_separate_from_context_and_sensitive_metadata() {
        let reading = BridgeReading::from_statusline(&json!({
            "rate_limits": {"five_hour": {"used_percentage": 23.5, "resets_at": 2000}, "seven_day": {"used_percentage": 80, "resets_at": 9000}},
            "context_window": {"used_percentage": 95},
            "session_id": "PRIVATE", "transcript_path": "PRIVATE", "accessToken": "PRIVATE"
        }), 1000).unwrap();
        let snapshot = reading.snapshot(1000).unwrap();
        assert_eq!(snapshot.pools[0].windows[0].remaining_percent(), Some(76));
        assert_eq!(
            snapshot
                .selected_window(None)
                .unwrap()
                .1
                .remaining_percent(),
            Some(20)
        );
        assert_eq!(snapshot.pools[0].windows[0].duration_mins, Some(300));
        assert_eq!(snapshot.pools[0].windows[1].duration_mins, Some(10080));
        assert!(snapshot.reset_credits.is_none());
        assert!(!serde_json::to_string(&reading).unwrap().contains("PRIVATE"));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("PRIVATE")
        );
    }

    #[test]
    fn missing_invalid_and_expired_readings_never_invent_allowance() {
        for payload in [
            json!({}),
            json!({"rate_limits":null}),
            json!({"rate_limits":{"five_hour":{"used_percentage":-1},"seven_day":{"used_percentage":101}}}),
        ] {
            let snapshot = BridgeReading::from_statusline(&payload, 1000)
                .unwrap()
                .snapshot(1000)
                .unwrap();
            assert!(
                snapshot.pools[0]
                    .windows
                    .iter()
                    .all(|window| window.used_percent.is_none())
            );
        }
        let reading = BridgeReading::from_statusline(
            &json!({"rate_limits":{
                "five_hour":{"used_percentage":100,"resets_at":1000},
                "seven_day":{"used_percentage":0,"resets_at":2000}
            }}),
            900,
        )
        .unwrap();
        let snapshot = reading.snapshot(1000).unwrap();
        assert_eq!(snapshot.pools[0].windows[0].remaining_percent(), None);
        assert_eq!(snapshot.pools[0].windows[0].resets_at, None);
        assert_eq!(snapshot.pools[0].windows[1].remaining_percent(), Some(100));
        assert_eq!(snapshot.fetched_at, 900);
        assert!(snapshot.is_stale(2000, 120));
    }

    #[test]
    fn invalid_payload_and_cache_time_fail_without_echoing_input() {
        let error = receive_statusline(Cursor::new(b"PRIVATE not JSON"), 1000)
            .err()
            .unwrap();
        assert!(!error.contains("PRIVATE"));
        assert!(
            receive_statusline(Cursor::new(vec![b' '; MAX_INPUT_BYTES as usize + 1]), 1000)
                .is_err()
        );
        assert!(BridgeReading::from_statusline(&json!([]), 1000).is_err());
        let reading = BridgeReading::from_statusline(&json!({}), 1061).unwrap();
        assert!(reading.snapshot(1000).is_err());
        let with_bom = receive_statusline(Cursor::new(b"\xef\xbb\xbf{}"), 1000).unwrap();
        assert_eq!(with_bom.received_at, 1000);
    }

    #[test]
    fn configuration_merge_preserves_user_customizations_and_existing_statusline() {
        let mut settings = json!({"permissions":{"allow":["Read"]},"theme":"dark"});
        merge_statusline(&mut settings, "new command").unwrap();
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        assert_eq!(settings["theme"], "dark");
        settings["statusLine"]["padding"] = json!(3);
        merge_statusline(&mut settings, "new command").unwrap();
        assert_eq!(settings["statusLine"]["padding"], 3);
        let preserved = settings.clone();
        assert!(merge_statusline(&mut settings, "other command").is_err());
        assert_eq!(settings, preserved);
        assert!(merge_statusline(&mut json!([]), "new command").is_err());
    }

    #[test]
    fn uninstall_removes_only_this_apps_exact_bridge_and_preserves_user_settings() {
        let command = "exact installed app bridge";
        let mut settings = json!({"statusLine":{"type":"command","command":command,"padding":3},"permissions":{"allow":["Read"]},"theme":"dark"});
        assert!(remove_statusline(&mut settings, command));
        assert!(settings.get("statusLine").is_none());
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        assert_eq!(settings["theme"], "dark");
        for mut settings in [
            json!({}),
            json!([]),
            json!({"statusLine":null}),
            json!({"statusLine":{"type":"command","command":"another app bridge"}}),
            json!({"statusLine":{"type":"custom","command":command}}),
            json!({"statusLine":{"type":"command","command":"exact installed app bridge && custom"}}),
        ] {
            let original = settings.clone();
            assert!(!remove_statusline(&mut settings, command));
            assert_eq!(settings, original);
        }
    }

    #[cfg(windows)]
    #[test]
    fn reconnect_relocates_only_our_generated_bridge_and_keeps_custom_fields() {
        let old = bridge_command(Path::new("C:/Portable/a'$(`whoami`)&b/TokWatch.exe")).unwrap();
        let new = bridge_command(Path::new(
            "C:/Users/Test/AppData/Local/Programs/TokWatch/TokWatch.exe",
        ))
        .unwrap();
        let mut settings =
            json!({"statusLine":{"type":"command","command":old,"padding":3},"theme":"dark"});
        merge_statusline(&mut settings, &new).unwrap();
        assert_eq!(settings["statusLine"]["command"], new);
        assert_eq!(settings["statusLine"]["padding"], 3);
        assert_eq!(settings["theme"], "dark");
        for custom in [
            format!("{old} extra"),
            "TokWatch.exe --claude-statusline".into(),
            bridge_command(Path::new("C:/Other/Custom.exe")).unwrap(),
            format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}", base64(&"$ErrorActionPreference='Stop'; & 'C:/TokWatch.exe' --claude-statusline; echo custom".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>())),
        ] {
            let mut settings = json!({"statusLine":{"type":"command","command":custom}});
            let original = settings.clone();
            assert!(merge_statusline(&mut settings, &new).is_err());
            assert_eq!(settings, original);
        }
    }

    #[test]
    fn launcher_encodes_paths_before_the_outer_shell_can_interpret_them() {
        #[cfg(windows)]
        let path = Path::new("C:/Users/a'$(`whoami`)&b/TokWatch.exe");
        #[cfg(not(windows))]
        let path = Path::new("/tmp/a'$(`whoami`)&b/TokWatch.exe");
        let command = bridge_command(path).unwrap();
        let encoded = command
            .strip_prefix("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
            .unwrap();
        assert!(
            encoded
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"+/=".contains(&ch))
        );
        assert!(!command.contains("whoami"));
        assert!(bridge_command(Path::new("relative.exe")).is_err());
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }
}
