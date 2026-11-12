use crate::{
    adb::{self, quote, TRANSFER},
    require_absolute,
    state::AppState,
    AppError, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::State;

#[derive(Serialize)]
pub struct Entry {
    name: String,
    directory: bool,
    size: Option<u64>,
    modified: String,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn remote(path: &str) -> Result<&str> {
    if path == "/sdcard"
        || path
            .strip_prefix("/sdcard/")
            .is_some_and(|suffix| suffix.split('/').all(valid_name))
    {
        Ok(path)
    } else {
        Err(AppError::new(
            "input",
            "Choose a path within headset storage.",
        ))
    }
}

fn child(path: &str, name: &str) -> Result<String> {
    if !valid_name(name) {
        return Err(AppError::new("input", "Enter a valid file or folder name."));
    }
    Ok(format!("{path}/{name}"))
}

fn parse_entry(line: &str) -> Option<Entry> {
    let mut rest = line;
    let mut fields = Vec::with_capacity(7);
    for _ in 0..7 {
        rest = rest.trim_start();
        let (field, remainder) = rest.split_once(char::is_whitespace)?;
        fields.push(field);
        rest = remainder;
    }
    let name = rest;
    let directory = match fields[0].chars().next()? {
        'd' => true,
        '-' => false,
        _ => return None,
    };
    if !valid_name(name) {
        return None;
    }
    Some(Entry {
        name: name.to_owned(),
        directory,
        size: (!directory).then(|| fields[4].parse().ok()).flatten(),
        modified: format!("{} {}", fields[5], fields[6]),
    })
}

#[tauri::command]
pub async fn list_files(
    serial: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<Entry>> {
    state.require_device(&serial)?;
    let path = remote(&path)?;
    let listing = state
        .adb
        .shell(&serial, &format!("ls -la -- {}", quote(path)))
        .await
        .map_err(|error| {
            if path.starts_with("/sdcard/Android/data") {
                AppError {
                    message: format!(
                        "Android/data could not be opened on this headset. {}",
                        error.message
                    ),
                    ..error
                }
            } else {
                error
            }
        })?;
    let mut files: Vec<_> = listing.lines().filter_map(parse_entry).collect();
    files.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(files)
}

#[tauri::command]
pub async fn file_action(
    serial: String,
    path: String,
    action: String,
    name: Option<String>,
    new_name: Option<String>,
    local: Option<PathBuf>,
    reveal: bool,
    state: State<'_, AppState>,
) -> Result<()> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    state.require_device(&serial)?;
    let path = remote(&path)?;
    let adb = &state.adb;
    let name = name.as_deref().unwrap_or_default();
    match action.as_str() {
        "mkdir" => {
            let target = child(path, name)?;
            adb.shell(&serial, &format!("mkdir -- {}", quote(&target)))
                .await?;
        }
        "rename" => {
            let from = child(path, name)?;
            let to = child(path, new_name.as_deref().unwrap_or_default())?;
            adb.shell(&serial, &format!("if [ -e {0} ]; then echo 'Destination already exists' >&2; exit 1; fi; mv -- {1} {0}", quote(&to), quote(&from))).await?;
        }
        "delete" => {
            let target = child(path, name)?;
            adb.shell(&serial, &format!("rm -rf -- {}", quote(&target)))
                .await?;
        }
        "push" => {
            let local = local_file(local)?;
            if !local.is_file() {
                return Err(AppError::new("input", "Choose a file to upload."));
            }
            let name = local
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let target = child(path, name)?;
            let temporary = child(
                path,
                &format!(".quest-installer-{}.part", std::process::id()),
            )?;
            let transfer = async {
                adb::output(adb.target(&serial)?.arg("push").arg(&local).arg(&temporary), TRANSFER).await?;
                adb.shell(&serial, &format!("if [ -d {0} ]; then echo 'A folder already uses this name' >&2; exit 1; fi; mv -f -- {1} {0}", quote(&target), quote(&temporary))).await?;
                Ok::<_, AppError>(())
            }.await;
            if let Err(error) = transfer {
                let _ = adb
                    .shell(&serial, &format!("rm -f -- {}", quote(&temporary)))
                    .await;
                return Err(error);
            }
        }
        "pull" => {
            let local = local_file(local)?;
            let from = child(path, name)?;
            if !local.is_dir() {
                return Err(AppError::new(
                    "input",
                    "Choose an existing destination folder.",
                ));
            }
            let destination = local.join(name);
            if destination.exists() {
                return Err(AppError::new(
                    "input",
                    "A file or folder already uses that name at the destination.",
                ));
            }
            let stage = tempfile::tempdir_in(&local)?;
            let staged = stage.path().join("download");
            adb::output(
                adb.target(&serial)?.arg("pull").arg(&from).arg(&staged),
                TRANSFER,
            )
            .await?;
            if destination.exists() {
                return Err(AppError::new(
                    "input",
                    "The destination appeared during the download.",
                ));
            }
            fs::rename(staged, &destination)?;
            if reveal {
                open_folder(&local)?;
            }
        }
        _ => return Err(AppError::new("input", "Unknown file action.")),
    }
    Ok(())
}

fn local_file(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = path.ok_or_else(|| AppError::new("input", "Choose a local path."))?;
    require_absolute(&path)?;
    Ok(path)
}

fn open_folder(path: &Path) -> Result<()> {
    let mut command = std::process::Command::new(if cfg!(windows) {
        "explorer.exe"
    } else {
        "/usr/bin/open"
    });
    command.arg(path).spawn().map_err(|error| {
        AppError::new(
            "io",
            format!("Download finished. Could not open its folder. {error}"),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_remote_names_and_limits_paths_to_shared_storage() {
        let entry =
            parse_entry("-rw-rw---- 1 root everybody 42 2026-11-12 08:30  Screenshot 1.png")
                .unwrap();
        assert_eq!(entry.name, " Screenshot 1.png");
        assert_eq!(entry.size, Some(42));
        assert!(remote("/sdcard/Oculus/Screenshots").is_ok());
        assert!(remote("/sdcard/../data").is_err());
        assert!(child("/sdcard", "../data").is_err());
    }
}

#[tauri::command]
pub async fn preview_image(
    serial: String,
    path: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<String> {
    state.require_device(&serial)?;
    let from = child(remote(&path)?, &name)?;
    let mime = match crate::builds::extension(Path::new(&name)).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => {
            return Err(AppError::new(
                "input",
                "This file is not a supported image.",
            ))
        }
    };
    let size = state
        .adb
        .shell(&serial, &format!("stat -c %s -- {}", quote(&from)))
        .await?;
    let limit = 20 * 1024 * 1024;
    if size.trim().parse::<u64>().map_or(true, |size| size > limit) {
        return Err(AppError::new(
            "input",
            "Download this image to view it. The preview limit is 20 MB.",
        ));
    }
    let stage = tempfile::tempdir()?;
    let file = stage.path().join("image");
    adb::output(
        state.adb.target(&serial)?.arg("pull").arg(from).arg(&file),
        TRANSFER,
    )
    .await?;
    if file.metadata()?.len() > limit {
        return Err(AppError::new(
            "input",
            "The image exceeds the preview limit.",
        ));
    }
    Ok(format!(
        "data:{mime};base64,{}",
        STANDARD.encode(fs::read(file)?)
    ))
}
