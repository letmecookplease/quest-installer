use crate::{require_absolute, state::AppState, AppError, Result};
use serde::Serialize;
use std::{fs, path::PathBuf, process::Stdio};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Clone, Serialize)]
struct LogEvent {
    session: String,
    line: Option<String>,
}

#[tauri::command]
pub async fn start_logs(
    app: AppHandle,
    serial: String,
    package: String,
    session: String,
    state: State<'_, AppState>,
) -> Result<()> {
    state.require_device(&serial)?;
    let build = state
        .build
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::new("input", "Choose a build before streaming logs."))?;
    if package != build.info.package {
        return Err(AppError::new(
            "input",
            "Select the package from the current build.",
        ));
    }
    let reply = state
        .adb
        .shell(&serial, &format!("pidof {}", crate::adb::quote(&package)))
        .await
        .map_err(|_| {
            AppError::new(
                "logs",
                "Launch the app on the headset before streaming logs.",
            )
        })?;
    let pid = reply
        .split_whitespace()
        .find_map(|word| word.parse::<u32>().ok())
        .ok_or_else(|| {
            AppError::new(
                "logs",
                "Launch the app on the headset before streaming logs.",
            )
        })?;
    let mut child = state
        .adb
        .target(&serial)?
        .args(["logcat", "-v", "threadtime", "-T", "150"])
        .arg(format!("--pid={pid}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("logs", "Could not read the log stream."))?;
    let handle = tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = app.emit(
                "logs:entry",
                LogEvent {
                    session: session.clone(),
                    line: Some(line.chars().take(4096).collect()),
                },
            );
        }
        let _ = child.wait().await;
        let _ = app.emit(
            "logs:entry",
            LogEvent {
                session,
                line: None,
            },
        );
    });
    if let Some(previous) = state.logs.lock().unwrap().replace(handle) {
        previous.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn stop_logs(state: State<'_, AppState>) {
    if let Some(task) = state.logs.lock().unwrap().take() {
        task.abort();
    }
}

#[tauri::command]
pub fn save_logs(path: PathBuf, text: String) -> Result<()> {
    require_absolute(&path)?;
    if text.len() > 20 * 1024 * 1024 {
        return Err(AppError::new(
            "logs",
            "The log buffer is too large to save.",
        ));
    }
    fs::write(path, text)?;
    Ok(())
}
