#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod apps;
mod builds;
mod files;
mod logs;
mod state;
mod storage;

use adb::{Adb, Device, SHORT};
use serde::Serialize;
use state::{AppState, Settings};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Serialize)]
pub struct AppError {
    code: &'static str,
    message: String,
    raw: Option<String>,
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            raw: None,
        }
    }

    pub fn with_raw(code: &'static str, message: impl Into<String>, raw: String) -> Self {
        Self {
            code,
            message: message.into(),
            raw: Some(raw),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new("io", error.to_string())
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::new("archive", error.to_string())
    }
}

#[tauri::command]
fn devices(state: State<'_, AppState>) -> state::DeviceSnapshot {
    state.devices()
}

#[tauri::command]
fn settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
fn set_theme(theme: String, state: State<'_, AppState>) -> Result<Settings> {
    if !matches!(theme.as_str(), "dark" | "light" | "sage-lime") {
        return Err(AppError::new("input", "Unknown theme."));
    }
    state.update_settings(|settings| settings.theme = theme)
}

#[tauri::command]
fn set_watched_folder(folder: Option<PathBuf>, state: State<'_, AppState>) -> Result<Settings> {
    if folder.as_ref().is_some_and(|path| !path.is_dir()) {
        return Err(AppError::new("input", "Choose an existing folder."));
    }
    state.update_settings(|settings| settings.watched_folder = folder)
}

#[tauri::command]
fn set_nickname(serial: String, nickname: String, state: State<'_, AppState>) -> Result<Settings> {
    state.adb.target(&serial)?;
    let nickname = nickname.trim().to_owned();
    if nickname.chars().count() > 32 || nickname.chars().any(char::is_control) {
        return Err(AppError::new(
            "input",
            "Use up to 32 characters for the headset name.",
        ));
    }
    state.update_settings(|settings| {
        if nickname.is_empty() {
            settings.nicknames.remove(&serial);
        } else {
            settings.nicknames.insert(serial, nickname);
        }
    })
}

#[tauri::command]
async fn wireless(
    action: String,
    endpoint: Option<String>,
    code: Option<String>,
    serial: Option<String>,
    state: State<'_, AppState>,
) -> Result<String> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    if action == "usb" {
        let serial = serial.ok_or_else(|| AppError::new("input", "Select a USB headset first."))?;
        state.require_device(&serial)?;
        return adb::output(state.adb.target(&serial)?.args(["tcpip", "5555"]), SHORT).await;
    }
    let endpoint = adb::local_endpoint(endpoint.as_deref().unwrap_or_default())?.to_string();
    if action == "pair" {
        let code = code
            .filter(|code| code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| AppError::new("input", "Enter the six-digit pairing code."))?;
        let reply = state
            .adb
            .run(&["pair", &endpoint, &code], SHORT)
            .await
            .map_err(|_| {
                AppError::new(
                    "pair",
                    "Pairing failed. Check the pairing port and code on the headset.",
                )
            })?;
        if !reply.contains("Successfully paired") {
            return Err(AppError::new(
                "pair",
                "Pairing failed. Check the pairing port and code on the headset.",
            ));
        }
        return Ok(reply);
    }
    if action != "connect" {
        return Err(AppError::new("input", "Unknown wireless action."));
    }
    let reply = state.adb.run(&["connect", &endpoint], SHORT).await?;
    if reply.starts_with("connected to ") || reply.starts_with("already connected to ") {
        Ok(reply)
    } else {
        Err(AppError::with_raw(
            "connect",
            "Could not connect to the headset.",
            reply,
        ))
    }
}

fn executable(app: &AppHandle, name: &str) -> PathBuf {
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let next_to_app = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(&filename)));
    if let Some(path) = next_to_app.filter(|path| path.is_file()) {
        return path;
    }
    app.path().resource_dir().unwrap_or_default().join(filename)
}

async fn track_devices(app: AppHandle, adb: Adb) {
    loop {
        match adb.track().await {
            Ok(mut socket) => {
                while let Ok(devices) = adb::next_devices(&mut socket).await {
                    publish_devices(&app, devices, None);
                }
                publish_devices(
                    &app,
                    vec![],
                    Some("Lost the ADB connection. Reconnecting…".into()),
                );
            }
            Err(error) => {
                publish_devices(
                    &app,
                    vec![],
                    Some(format!("ADB unavailable. {}", error.message)),
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn publish_devices(app: &AppHandle, devices: Vec<Device>, error: Option<String>) {
    let state = app.state::<AppState>();
    let snapshot = state::DeviceSnapshot {
        devices,
        error,
        loading: false,
    };
    if state.set_devices(snapshot.clone()) {
        let _ = app.emit("devices:changed", snapshot);
    }
}

pub fn require_absolute(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(AppError::new("input", "Choose an absolute local path."));
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            let adb_home = data.join("adb");
            std::fs::create_dir_all(&adb_home)?;
            let adb = Adb::new(executable(app.handle(), "adb"), adb_home);
            let state = AppState::new(adb.clone(), executable(app.handle(), "aapt2"), data);
            app.manage(state);
            tauri::async_runtime::spawn(track_devices(app.handle().clone(), adb));
            tauri::async_runtime::spawn(state::watch_builds(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            devices,
            settings,
            set_theme,
            set_watched_folder,
            set_nickname,
            wireless,
            apps::list_installed_apps,
            apps::uninstall_app,
            builds::inspect_build,
            storage::available_space,
            builds::run_install,
            builds::app_control,
            files::list_files,
            files::file_action,
            files::preview_image,
            logs::start_logs,
            logs::stop_logs,
            logs::save_logs
        ])
        .run(tauri::generate_context!())
        .expect("Quest Installer failed to start");
}
