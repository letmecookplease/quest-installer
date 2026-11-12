use crate::{
    adb::{self, quote, SHORT, TRANSFER},
    require_absolute,
    state::{AppState, History},
    AppError, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    name: String,
    pub package: String,
    version: String,
    version_code: String,
    expansions: Vec<String>,
    estimated_bytes: u64,
}

pub struct Build {
    pub info: BuildInfo,
    apk: PathBuf,
    obbs: Vec<PathBuf>,
    source: PathBuf,
    _staged: tempfile::TempDir,
}

pub fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn stage_path(dir: &Path, source: &Path) -> Result<PathBuf> {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.contains(['/', '\\', ':']) && !name.chars().any(char::is_control))
        .ok_or_else(|| AppError::new("input", "The build contains an unsupported filename."))?;
    Ok(dir.join(name))
}

fn collect_local(path: &Path, level: usize, files: &mut Vec<PathBuf>) -> Result<()> {
    for item in fs::read_dir(path)? {
        let item = item?;
        let kind = item.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let path = item.path();
        if kind.is_file() && matches!(extension(&path).as_str(), "apk" | "obb") {
            files.push(path);
            if files.len() > 64 {
                return Err(AppError::new(
                    "input",
                    "Choose a folder containing one build.",
                ));
            }
        } else if kind.is_dir() && level < 3 {
            collect_local(&path, level + 1, files)?;
        }
    }
    Ok(())
}

fn stage_build(source: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
    require_absolute(source)?;
    let mut files = Vec::new();
    if extension(source) == "zip" && source.is_file() {
        let mut archive = zip::ZipArchive::new(File::open(source)?)?;
        let mut names = HashSet::new();
        let mut bytes = 0_u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            if !entry.is_file() || entry.is_symlink() {
                continue;
            }
            let path = entry
                .enclosed_name()
                .ok_or_else(|| AppError::new("archive", "The ZIP contains an unsafe path."))?;
            if !matches!(extension(&path).as_str(), "apk" | "obb") {
                continue;
            }
            let destination = stage_path(dir, &path)?;
            if !names.insert(destination.to_string_lossy().to_lowercase()) || files.len() >= 64 {
                return Err(AppError::new(
                    "archive",
                    "The ZIP contains duplicate or too many build files.",
                ));
            }
            bytes = bytes.saturating_add(entry.size());
            if bytes > 24 * 1024 * 1024 * 1024 {
                return Err(AppError::new(
                    "archive",
                    "The extracted build exceeds 24 GB.",
                ));
            }
            let expected = entry.size();
            let mut output = File::create(&destination)?;
            if io::copy(&mut io::Read::take(&mut entry, expected + 1), &mut output)? != expected {
                return Err(AppError::new(
                    "archive",
                    "The ZIP entry size does not match its contents.",
                ));
            }
            files.push(destination);
        }
    } else {
        let mut sources = Vec::new();
        if source.is_dir() {
            collect_local(source, 0, &mut sources)?;
        } else if source.is_file() && extension(source) == "apk" {
            sources.push(source.to_owned());
            if let Some(parent) = source.parent() {
                for item in fs::read_dir(parent)? {
                    let item = item?;
                    if item.file_type()?.is_file() && extension(&item.path()) == "obb" {
                        sources.push(item.path());
                    }
                }
            }
        } else {
            return Err(AppError::new(
                "input",
                "Choose an APK, a ZIP with an APK, or a build folder.",
            ));
        }
        let mut names = HashSet::new();
        for path in sources {
            let destination = stage_path(dir, &path)?;
            if !names.insert(destination.to_string_lossy().to_lowercase()) {
                return Err(AppError::new(
                    "input",
                    "The build contains duplicate filenames.",
                ));
            }
            fs::copy(path, &destination)?;
            files.push(destination);
        }
    }
    Ok(files)
}

pub fn valid_package(package: &str) -> bool {
    package.len() <= 255
        && package.contains('.')
        && package.split('.').all(|part| {
            part.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

fn attribute<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_once(&format!(" {key}='"))?
        .1
        .split_once('\'')
        .map(|(value, _)| value)
}

fn obb_for(path: &Path, package: &str) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let suffix = format!(".{package}.obb");
    name.strip_suffix(&suffix)
        .and_then(|prefix| prefix.split_once('.'))
        .is_some_and(|(kind, version)| {
            matches!(kind, "main" | "patch")
                && !version.is_empty()
                && version.bytes().all(|b| b.is_ascii_digit())
        })
}

#[tauri::command]
pub async fn inspect_build(source: PathBuf, state: State<'_, AppState>) -> Result<BuildInfo> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    let _ = state.build.lock().unwrap().take();
    let stage_source = source.clone();
    let (staged, files) = tauri::async_runtime::spawn_blocking(move || {
        let staged = tempfile::tempdir()?;
        let files = stage_build(&stage_source, staged.path())?;
        Ok::<_, AppError>((staged, files))
    })
    .await
    .map_err(|error| AppError::new("build", error.to_string()))??;
    let mut apks = files.iter().filter(|path| extension(path) == "apk");
    let apk = apks
        .next()
        .cloned()
        .ok_or_else(|| AppError::new("input", "The build does not contain an APK."))?;
    if apks.next().is_some() {
        return Err(AppError::new(
            "input",
            "Choose a build with exactly one APK.",
        ));
    }
    let metadata = adb::output(
        adb::command(&state.aapt)
            .args(["dump", "badging"])
            .arg(&apk),
        SHORT,
    )
    .await
    .map_err(|error| {
        AppError::with_raw(
            "apk",
            "Could not read the APK manifest.",
            error.raw.unwrap_or(error.message),
        )
    })?;
    let line = metadata
        .lines()
        .find(|line| line.starts_with("package: "))
        .ok_or_else(|| AppError::new("apk", "The APK did not provide package metadata."))?;
    let package = attribute(line, "name")
        .filter(|value| valid_package(value))
        .ok_or_else(|| AppError::new("apk", "The APK package name is invalid."))?
        .to_owned();
    let mut obbs: Vec<_> = files
        .into_iter()
        .filter(|path| obb_for(path, &package))
        .collect();
    obbs.sort();
    let estimated_bytes = crate::storage::estimate_build_bytes(&state.aapt, &apk, &obbs).await?;
    let info = BuildInfo {
        name: apk.file_name().unwrap().to_string_lossy().into_owned(),
        package,
        version: attribute(line, "versionName")
            .unwrap_or_default()
            .to_owned(),
        version_code: attribute(line, "versionCode")
            .unwrap_or_default()
            .to_owned(),
        expansions: obbs
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect(),
        estimated_bytes,
    };
    *state.build.lock().unwrap() = Some(Arc::new(Build {
        info: info.clone(),
        apk,
        obbs,
        source,
        _staged: staged,
    }));
    Ok(info)
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InstallMode {
    Normal,
    Clean,
    Wipe,
}

#[derive(Clone, Serialize)]
pub struct Progress {
    serial: String,
    stage: String,
    step: usize,
    total: usize,
    transfer: Option<TransferProgress>,
}

#[derive(Clone, Serialize)]
struct TransferProgress {
    sent: u64,
    total: u64,
}

#[derive(Serialize)]
pub struct InstallResult {
    history: Vec<History>,
    notice: Option<String>,
}

#[tauri::command]
pub async fn run_install(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    mode: InstallMode,
    backup: bool,
    launch: bool,
) -> Result<InstallResult> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    state.require_device(&serial)?;
    let build = state
        .build
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::new("input", "Choose a build first."))?;
    let adb = &state.adb;
    let package = &build.info.package;
    let total = match mode {
        InstallMode::Wipe => 1,
        InstallMode::Normal => 1 + usize::from(!build.obbs.is_empty()) + usize::from(launch),
        InstallMode::Clean => {
            4 + usize::from(backup) + usize::from(!build.obbs.is_empty()) + usize::from(launch)
        }
    };
    let report = |step: usize, stage: &str, transfer: Option<TransferProgress>| {
        let _ = app.emit(
            "install:progress",
            Progress {
                serial: serial.clone(),
                stage: stage.to_owned(),
                step,
                total,
                transfer,
            },
        );
    };
    let mut step = 0;
    let mut progress = |stage: &str| {
        step += 1;
        report(step, stage, None);
        step
    };
    if mode == InstallMode::Wipe {
        progress("Clearing app data");
        let reply = adb
            .shell(&serial, &format!("pm clear {}", quote(package)))
            .await?;
        if reply.trim() != "Success" {
            return Err(AppError::with_raw(
                "adb",
                "Could not clear app data.",
                reply,
            ));
        }
        return Ok(InstallResult {
            history: state.settings().history,
            notice: None,
        });
    }
    let [data, obb] = crate::apps::storage_paths(package);
    if mode == InstallMode::Clean {
        progress("Stopping the app");
        adb.shell(&serial, &format!("am force-stop {}", quote(package)))
            .await?;
        if backup {
            progress("Backing up accessible saves");
            let exists = adb.shell(&serial, &format!(
                "ls -a /sdcard/Android/data >/dev/null && if [ -d {} ]; then echo present; fi", quote(&data)
            )).await?;
            if exists.trim() == "present" {
                let folder = state.data.join("backups").join(package);
                fs::create_dir_all(&folder)?;
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let backup = tempfile::Builder::new()
                    .prefix(&format!("backup-{timestamp}-"))
                    .tempdir_in(folder)?;
                adb::output(
                    adb.target(&serial)?
                        .arg("pull")
                        .arg(&data)
                        .arg(backup.path()),
                    TRANSFER,
                )
                .await?;
                let _ = backup.keep();
            }
        }
        progress("Removing the previous build");
        let installed = adb
            .shell(&serial, &format!("pm list packages {}", quote(package)))
            .await?;
        if installed
            .lines()
            .any(|line| line.trim() == format!("package:{package}"))
        {
            let reply =
                adb::output(adb.target(&serial)?.args(["uninstall", package]), SHORT).await?;
            if reply.trim() != "Success" {
                return Err(AppError::with_raw(
                    "adb",
                    "Could not uninstall the previous build.",
                    reply,
                ));
            }
        }
        progress("Removing leftover app files");
        adb.shell(
            &serial,
            &format!("rm -rf -- {} {}", quote(&obb), quote(&data)),
        )
        .await?;
    }
    let apk_step = progress("Preparing APK transfer");
    let reply = adb
        .install_apk(&serial, &build.apk, |sent, total| {
            if sent < total {
                report(
                    apk_step,
                    "Sending APK to headset",
                    Some(TransferProgress { sent, total }),
                );
            } else {
                report(apk_step, "Finishing install on headset", None);
            }
        })
        .await?;
    if !reply.lines().any(|line| line.trim() == "Success") {
        return Err(AppError::with_raw(
            "adb",
            "The APK install did not complete.",
            reply,
        ));
    }
    if !build.obbs.is_empty() {
        let obb_step = progress("Preparing OBB transfer");
        adb.shell(&serial, &format!("mkdir -p -- {}", quote(&obb)))
            .await?;
        for (index, path) in build.obbs.iter().enumerate() {
            report(
                obb_step,
                &format!("Sending OBB file {} of {}", index + 1, build.obbs.len()),
                None,
            );
            let name = path.file_name().unwrap().to_string_lossy();
            let temporary = format!("{obb}/.quest-installer-{index}.part");
            let destination = format!("{obb}/{name}");
            let transfer = async {
                adb::output(
                    adb.target(&serial)?.arg("push").arg(path).arg(&temporary),
                    TRANSFER,
                )
                .await?;
                adb.shell(
                    &serial,
                    &format!("mv -f -- {} {}", quote(&temporary), quote(&destination)),
                )
                .await?;
                Ok::<_, AppError>(())
            }
            .await;
            if let Err(error) = transfer {
                let _ = adb
                    .shell(&serial, &format!("rm -f -- {}", quote(&temporary)))
                    .await;
                return Err(AppError {
                    message: format!(
                        "APK installed, but an OBB transfer failed. {}",
                        error.message
                    ),
                    ..error
                });
            }
        }
    }
    let mut notice = None;
    if launch {
        progress("Opening the installed app");
        if let Err(error) = launch_app(adb, &serial, package).await {
            notice = Some(format!(
                "The build installed, but it could not launch. {}",
                error.message
            ));
        }
    }
    let history = match state.update_settings(|settings| {
        settings.history.retain(|item| item.source != build.source);
        settings.history.insert(
            0,
            History {
                source: build.source.clone(),
                name: build.info.name.clone(),
                version: build.info.version.clone(),
            },
        );
        settings.history.truncate(20);
    }) {
        Ok(settings) => settings.history,
        Err(_) => {
            let message = "The build installed, but the history could not be saved.";
            notice = Some(notice.map_or(message.to_owned(), |previous| {
                format!("{previous} {message}")
            }));
            state.settings().history
        }
    };
    Ok(InstallResult { history, notice })
}

async fn launch_app(adb: &adb::Adb, serial: &str, package: &str) -> Result<()> {
    let reply = adb
        .shell(
            serial,
            &format!(
                "monkey -p {} -c android.intent.category.LAUNCHER 1",
                quote(package)
            ),
        )
        .await?;
    if !reply.contains("Events injected: 1") {
        return Err(AppError::with_raw(
            "adb",
            "Could not launch the app.",
            reply,
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn app_control(serial: String, action: String, state: State<'_, AppState>) -> Result<()> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    state.require_device(&serial)?;
    let build = state
        .build
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| AppError::new("input", "Choose a build first."))?;
    let package = &build.info.package;
    match action.as_str() {
        "launch" => launch_app(&state.adb, &serial, package).await,
        "stop" => {
            state
                .adb
                .shell(&serial, &format!("am force-stop {}", quote(package)))
                .await?;
            Ok(())
        }
        _ => Err(AppError::new("input", "Unknown app action.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn stages_nested_archive_without_extracting_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("build.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for name in [
            "build/app.apk",
            "build/data/main.42.com.example.app.obb",
            "build/readme.txt",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"sample").unwrap();
        }
        zip.finish().unwrap();
        let staged = tempfile::tempdir().unwrap();
        let paths = stage_build(&archive, staged.path()).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| obb_for(path, "com.example.app")));
        assert!(!staged.path().join("readme.txt").exists());
    }

    #[test]
    fn rejects_traversal_and_shell_metacharacters_in_package_names() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("build.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file("../escape.apk", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"sample").unwrap();
        zip.finish().unwrap();
        let staged = tempfile::tempdir_in(dir.path()).unwrap();
        assert!(stage_build(&archive, staged.path()).is_err());
        assert!(!dir.path().join("escape.apk").exists());
        assert!(valid_package("com.example.build_12"));
        assert!(!valid_package("com.example;whoami"));
    }
}
