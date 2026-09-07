use crate::model::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    #[default]
    #[serde(other)]
    Codex,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub poll_seconds: u64,
    pub provider: Provider,
    pub automatic_updates: bool,
    pub codex_path: Option<PathBuf>,
    /// A stable key such as "codex:primary"; None selects automatically.
    pub selected_window: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_seconds: 120,
            provider: Provider::Codex,
            automatic_updates: true,
            codex_path: None,
            selected_window: None,
        }
    }
}

impl Settings {
    pub fn normalized(mut self) -> Self {
        self.poll_seconds = self.poll_seconds();
        self.codex_path = self.codex_path.filter(|path| !path.as_os_str().is_empty());
        self.selected_window = self
            .selected_window
            .filter(|key| !key.is_empty() && key.len() <= 256);
        self
    }

    pub fn poll_seconds(&self) -> u64 {
        self.poll_seconds.clamp(60, 1800)
    }

    pub fn load() -> Result<Self, String> {
        let path = data_dir()?.join("settings.json");
        match read_json::<Self>(&path, 64 * 1024)? {
            Some(settings) => Ok(settings.normalized()),
            None => Ok(Self::default()),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        save_json(
            &data_dir()?.join("settings.json"),
            &self.clone().normalized(),
        )
    }
}

pub fn data_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "Windows did not provide a local application data folder.".to_string())?;
    Ok(base.join("TokWatch"))
}

pub fn load_cache() -> Result<Option<UsageSnapshot>, String> {
    read_json(&data_dir()?.join("usage-cache.json"), 512 * 1024)
}

pub fn save_cache(snapshot: &UsageSnapshot) -> Result<(), String> {
    save_json(&data_dir()?.join("usage-cache.json"), snapshot)
}

pub fn clear_cache() -> Result<(), String> {
    let path = data_dir()?.join("usage-cache.json");
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not clear {}: {error}", path.display())),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<Option<T>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    let mut contents = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut contents)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if contents.len() as u64 > limit {
        return Err(format!("{} is larger than expected.", path.display()));
    }
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|error| format!("{} contains invalid data: {error}", path.display()))
}

/// Write and flush a unique sibling before atomically replacing the destination.
/// Rust's Windows rename uses replacement semantics, so the prior good file is
/// never deleted first and survives a failed replacement.
fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid settings location.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not prepare local settings: {error}"))?;
    let name = path
        .file_name()
        .ok_or_else(|| "Invalid settings filename.".to_string())?
        .to_string_lossy();
    let (temporary, mut file) = loop {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{}.{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not save {}: {error}", path.display())),
        }
    };
    let result = (|| {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not save {}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_receive_defaults_and_polling_is_bounded() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.poll_seconds(), 120);
        assert_eq!(settings.provider, Provider::Codex);
        assert!(settings.automatic_updates);
        let low: Settings = serde_json::from_str(r#"{"poll_seconds":0}"#).unwrap();
        assert_eq!(low.normalized().poll_seconds, 60);
        let high: Settings =
            serde_json::from_str(r#"{"poll_seconds":999999,"selected_window":""}"#).unwrap();
        let high = high.normalized();
        assert_eq!(high.poll_seconds, 1800);
        assert!(high.selected_window.is_none());
    }

    #[test]
    fn replacement_preserves_valid_json_and_no_sibling_temp_remains() {
        let directory = std::env::temp_dir().join(format!(
            "tokwatch-settings-test-{}-{}",
            std::process::id(),
            SEQUENCE_FOR_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        save_json(&path, &Settings::default()).unwrap();
        let changed = Settings {
            poll_seconds: 300,
            ..Settings::default()
        };
        save_json(&path, &changed).unwrap();
        assert_eq!(
            read_json::<Settings>(&path, 65536)
                .unwrap()
                .unwrap()
                .poll_seconds,
            300
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&directory).unwrap();
    }

    #[test]
    fn oversized_cache_is_rejected() {
        let path = std::env::temp_dir().join(format!(
            "tokwatch-size-test-{}-{}.json",
            std::process::id(),
            SEQUENCE_FOR_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, b"1234567890").unwrap();
        assert!(
            read_json::<serde_json::Value>(&path, 5)
                .unwrap_err()
                .contains("larger than expected")
        );
        fs::remove_file(path).unwrap();
    }

    static SEQUENCE_FOR_TEST: AtomicU64 = AtomicU64::new(0);
}
