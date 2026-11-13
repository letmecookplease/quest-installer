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
    regular: bool,
    size: Option<u64>,
    modified: String,
}

const SHARED_STORAGE: &str = "/sdcard";

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn remote(path: &str) -> Result<&str> {
    if path == "/"
        || path
            .strip_prefix('/')
            .is_some_and(|suffix| suffix.split('/').all(valid_name))
    {
        Ok(path)
    } else {
        Err(AppError::new("input", "Choose a valid headset path."))
    }
}

fn child(path: &str, name: &str) -> Result<String> {
    if !valid_name(name) {
        return Err(AppError::new("input", "Enter a valid file or folder name."));
    }
    Ok(format!("{}/{name}", path.trim_end_matches('/')))
}

fn within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn listing_command(path: &str) -> String {
    format!("cd -- {} && LC_ALL=C ls -lAbnL -- .", quote(path))
}

fn field<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let (value, remainder) = rest.trim_start().split_once(char::is_whitespace)?;
    *rest = remainder;
    Some(value)
}

fn listed_name(name: &str) -> Option<String> {
    let mut result = Vec::with_capacity(name.len());
    let mut bytes = name.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            result.push(byte);
            continue;
        }
        match bytes.next()? {
            b' ' => result.push(b' '),
            // Toybox uses octal escapes for bytes that cannot be printed.
            first @ b'0'..=b'3' => {
                let second = bytes.next()?.checked_sub(b'0').filter(|digit| *digit < 8)?;
                let third = bytes.next()?.checked_sub(b'0').filter(|digit| *digit < 8)?;
                result.push((first - b'0') * 64 + second * 8 + third);
            }
            _ => return None,
        }
    }
    let result = String::from_utf8(result).ok()?;
    (valid_name(&result) && !result.contains('\u{fffd}')).then_some(result)
}

fn parse_entry(line: &str) -> Option<Entry> {
    let mut rest = line;
    let mode = field(&mut rest)?;
    let kind = mode.as_bytes().first().copied()?;
    if !b"d-lbcps?".contains(&kind) {
        return None;
    }
    for _ in 0..3 {
        field(&mut rest)?;
    }
    let size = field(&mut rest)?;
    if matches!(kind, b'b' | b'c') && size.ends_with(',') {
        field(&mut rest)?;
    }
    let date = field(&mut rest)?;
    let time = field(&mut rest)?;
    let name = if kind == b'l' {
        rest.split_once(" -> ").map_or(rest, |(name, _)| name)
    } else {
        rest
    };
    let regular = kind == b'-' && !mode.contains('?');
    Some(Entry {
        name: listed_name(name)?,
        directory: kind == b'd',
        regular,
        size: if regular { size.parse().ok() } else { None },
        modified: if date == "?" {
            "—".into()
        } else {
            format!("{date} {time}")
        },
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
        .shell(&serial, &listing_command(path))
        .await
        .map_err(|error| {
            if error.code == "permission" {
                AppError {
                    message: format!("The headset denied access to {path}."),
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
    if matches!(action.as_str(), "mkdir" | "rename" | "delete" | "push") {
        if !within(path, SHARED_STORAGE) {
            return Err(AppError::new(
                "permission",
                "Edit files from Internal storage.",
            ));
        }
        let shared = adb.shell(&serial, "readlink -f -- /sdcard").await?;
        let directory = adb
            .shell(&serial, &format!("readlink -f -- {}", quote(path)))
            .await?;
        if shared == "/" || remote(&shared).is_err() || !within(&directory, &shared) {
            return Err(AppError::new(
                "permission",
                "This folder points outside Internal storage.",
            ));
        }
    }
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
            if cfg!(windows) && !windows_filename(name) {
                return Err(AppError::new(
                    "input",
                    "Windows cannot save this file name.",
                ));
            }
            let source = quote(&from);
            let allowed = if within(path, SHARED_STORAGE) {
                format!("[ -f {source} ] || [ -d {source} ]")
            } else {
                format!("[ -f {source} ]")
            };
            adb.shell(&serial, &format!("{allowed} || {{ echo 'Only ordinary files and shared folders can be downloaded' >&2; exit 1; }}")).await?;
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

fn windows_filename(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !name.contains(['<', '>', ':', '"', '|', '?', '*'])
        && !name.ends_with([' ', '.'])
        && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
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
    fn preserves_remote_names_and_rejects_traversal() {
        let entry =
            parse_entry(r"-rw-rw---- 1 0 9997 42 2026-11-12 08:30 \ Screenshot\ 1.png").unwrap();
        assert_eq!(entry.name, " Screenshot 1.png");
        assert_eq!(entry.size, Some(42));
        assert!(remote("/sdcard/Oculus/Screenshots").is_ok());
        assert!(remote("/").is_ok());
        assert!(remote("/system/etc").is_ok());
        assert!(remote("/sdcard/../data").is_err());
        assert!(remote("/system//etc").is_err());
        assert!(child("/sdcard", "../data").is_err());
        assert_eq!(child("/", "system").unwrap(), "/system");
        assert!(!within("/sdcard2", SHARED_STORAGE));
        assert!(!windows_filename("report:other"));
        assert!(!windows_filename("COM1.txt"));
        assert!(windows_filename("Screenshot 1.png"));
    }

    #[test]
    fn lists_storage_aliases_and_system_entries() {
        let cmd = listing_command("/sdcard");
        assert_eq!(cmd, "cd -- '/sdcard' && LC_ALL=C ls -lAbnL -- .");
        let folder = parse_entry("drwxrwx--- 2 0 9997 3488 2023-04-27 03:53 Android").unwrap();
        assert!(folder.directory);
        let locked = parse_entry("d????????? ? ? ? ? ? ? data_mirror").unwrap();
        assert!(locked.directory);
        assert_eq!(locked.modified, "—");
        let link =
            parse_entry("lrw-r--r-- 1 0 0 17 2009-01-01 05:00 d -> /sys/kernel/debug").unwrap();
        assert_eq!(link.name, "d");
        assert!(!link.regular);
        let device =
            parse_entry("crw-rw-r-- 1 1000 1000 504, 0 1970-01-01 08:45 adsprpc-smd").unwrap();
        assert_eq!(device.name, "adsprpc-smd");
        assert!(!device.regular);
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
    let target = quote(&from);
    let size = state.adb.shell(&serial, &format!(
        "[ -f {target} ] || {{ echo 'This path is not an ordinary file' >&2; exit 1; }}; stat -L -c %s -- {target}"
    )).await?;
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
