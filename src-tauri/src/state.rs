use crate::{
    adb::{Adb, Device},
    builds::Build,
    AppError, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeviceSnapshot {
    pub devices: Vec<Device>,
    pub error: Option<String>,
    pub loading: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: String,
    pub watched_folder: Option<PathBuf>,
    pub nicknames: HashMap<String, String>,
    pub history: Vec<History>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            watched_folder: None,
            nicknames: HashMap::new(),
            history: vec![],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub source: PathBuf,
    pub name: String,
    pub version: String,
}

pub struct AppState {
    pub adb: Adb,
    pub aapt: PathBuf,
    pub data: PathBuf,
    pub operation: tokio::sync::Mutex<()>,
    pub build: Mutex<Option<Arc<Build>>>,
    pub logs: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    snapshot: Mutex<DeviceSnapshot>,
    settings: Mutex<Settings>,
}

impl AppState {
    pub fn new(adb: Adb, aapt: PathBuf, data: PathBuf) -> Self {
        let settings = fs::read(data.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            adb,
            aapt,
            data,
            operation: tokio::sync::Mutex::new(()),
            build: Mutex::new(None),
            logs: Mutex::new(None),
            settings: Mutex::new(settings),
            snapshot: Mutex::new(DeviceSnapshot {
                devices: vec![],
                error: None,
                loading: true,
            }),
        }
    }

    pub fn devices(&self) -> DeviceSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn set_devices(&self, snapshot: DeviceSnapshot) -> bool {
        let mut current = self.snapshot.lock().unwrap();
        if *current == snapshot {
            return false;
        }
        *current = snapshot;
        true
    }

    pub fn require_device(&self, serial: &str) -> Result<Device> {
        self.snapshot
            .lock()
            .unwrap()
            .devices
            .iter()
            .find(|device| device.serial == serial && device.status == "device")
            .cloned()
            .ok_or_else(|| AppError::new("device", "Select a connected, authorized headset."))
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn update_settings(&self, update: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut current = self.settings.lock().unwrap();
        let mut updated = current.clone();
        update(&mut updated);
        let mut file = tempfile::NamedTempFile::new_in(&self.data)?;
        serde_json::to_writer_pretty(&mut file, &updated)
            .map_err(|error| AppError::new("settings", error.to_string()))?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.data.join("settings.json"))
            .map_err(|error| error.error)?;
        *current = updated.clone();
        Ok(updated)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Stamp(u64, SystemTime);

fn watch_snapshot(folder: &Path) -> std::io::Result<HashMap<PathBuf, Stamp>> {
    let mut snapshot = HashMap::new();
    for item in fs::read_dir(folder)?.flatten() {
        let path = item.path();
        if !matches!(crate::builds::extension(&path).as_str(), "apk" | "zip") {
            continue;
        }
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };
        if metadata.is_file() && metadata.len() > 0 {
            if let Ok(modified) = metadata.modified() {
                snapshot.insert(path, Stamp(metadata.len(), modified));
            }
        }
    }
    Ok(snapshot)
}

pub async fn watch_builds(app: AppHandle) {
    let mut folder = None;
    let mut previous = HashMap::new();
    let mut announced = HashSet::new();
    loop {
        let configured = app.state::<AppState>().settings().watched_folder;
        if let Some(path) = &configured {
            if let Ok(current) = watch_snapshot(path) {
                if folder != configured {
                    announced = current
                        .iter()
                        .map(|(path, stamp)| (path.clone(), *stamp))
                        .collect();
                    folder = configured.clone();
                } else {
                    announced.retain(|(path, stamp)| current.get(path) == Some(stamp));
                    for (path, stamp) in &current {
                        if previous.get(path) == Some(stamp)
                            && announced.insert((path.clone(), *stamp))
                        {
                            let _ = app.emit("build:watched", path);
                        }
                    }
                }
                previous = current;
            }
        } else {
            folder = None;
            previous.clear();
            announced.clear();
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_theme_without_losing_nicknames() {
        let directory = tempfile::tempdir().unwrap();
        let dummy = directory.path().join("dummy");
        let adb = Adb::new(dummy.clone(), directory.path().to_owned());
        let state = AppState::new(adb.clone(), dummy.clone(), directory.path().to_owned());
        assert_eq!(state.settings().theme, "dark");
        state
            .update_settings(|settings| {
                settings.theme = "light".into();
                settings
                    .nicknames
                    .insert("usb-test".into(), "QA headset".into());
            })
            .unwrap();
        state
            .update_settings(|settings| settings.theme = "sage-lime".into())
            .unwrap();
        let reopened = AppState::new(adb, dummy, directory.path().to_owned());
        assert_eq!(reopened.settings().theme, "sage-lime");
        assert_eq!(reopened.settings().nicknames["usb-test"], "QA headset");
    }
}
