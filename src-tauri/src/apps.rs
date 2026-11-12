use crate::{
    adb::{self, quote, Adb},
    builds::valid_package,
    state::AppState,
    AppError, Result,
};
use serde::Serialize;
use std::time::Duration;
use tauri::State;

const DATA: &str = "/sdcard/Android/data";
const OBB: &str = "/sdcard/Android/obb";

pub fn storage_paths(package: &str) -> [String; 2] {
    [format!("{DATA}/{package}"), format!("{OBB}/{package}")]
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    package: String,
    version_code: String,
}

fn parse_apps(listing: &str) -> Vec<InstalledApp> {
    let mut apps: Vec<_> = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let package = fields.next()?.strip_prefix("package:")?;
            let version = fields.find_map(|field| field.strip_prefix("versionCode:"))?;
            if !valid_package(package) || version.parse::<u64>().is_err() {
                return None;
            }
            Some(InstalledApp {
                package: package.to_owned(),
                version_code: version.to_owned(),
            })
        })
        .collect();
    apps.sort_by_cached_key(|app| app.package.to_ascii_lowercase());
    apps
}

async fn current_user(adb: &Adb, serial: &str) -> Result<u32> {
    adb.shell(serial, "am get-current-user")
        .await?
        .trim()
        .parse()
        .map_err(|_| AppError::new("apps", "Could not identify the active headset profile."))
}

async fn installed_apps(adb: &Adb, serial: &str, user: u32) -> Result<Vec<InstalledApp>> {
    let listing = adb
        .shell(
            serial,
            &format!("pm list packages -3 --show-versioncode --user {user}"),
        )
        .await?;
    Ok(parse_apps(&listing))
}

#[tauri::command]
pub async fn list_installed_apps(
    serial: String,
    state: State<'_, AppState>,
) -> Result<Vec<InstalledApp>> {
    state.require_device(&serial)?;
    let user = current_user(&state.adb, &serial).await?;
    installed_apps(&state.adb, &serial, user).await
}

#[derive(Serialize)]
pub struct Removal {
    cleanup_errors: Vec<String>,
}

#[tauri::command]
pub async fn uninstall_app(
    serial: String,
    package: String,
    version_code: String,
    state: State<'_, AppState>,
) -> Result<Removal> {
    let _operation = state
        .operation
        .try_lock()
        .map_err(|_| AppError::new("busy", "Wait for the current operation to finish."))?;
    state.require_device(&serial)?;
    if !valid_package(&package) || version_code.parse::<u64>().is_err() {
        return Err(AppError::new(
            "input",
            "Select an app from the installed apps list.",
        ));
    }
    let adb = &state.adb;
    let user = current_user(adb, &serial).await?;
    if !installed_apps(adb, &serial, user)
        .await?
        .iter()
        .any(|app| app.package == package && app.version_code == version_code)
    {
        return Err(AppError::new(
            "apps",
            "The installed app changed. Refresh the list before uninstalling.",
        ));
    }

    let command = format!(
        "pm uninstall --versionCode {version_code} {}",
        quote(&package)
    );
    let reply = adb::output(
        adb.target(&serial)?.args(["shell", "-T", &command]),
        Duration::from_secs(120),
    )
    .await?;
    if reply.trim() != "Success" {
        return Err(AppError::with_raw(
            "apps",
            "Android could not uninstall the app.",
            reply,
        ));
    }

    let mut cleanup_errors = Vec::new();
    match installed_apps(adb, &serial, user).await {
        Ok(apps) if apps.iter().any(|app| app.package == package) => {
            cleanup_errors
                .push("The package is present again. Its shared folders were kept.".into());
        }
        Ok(_) if user == 0 => {
            for (parent, path) in [DATA, OBB].into_iter().zip(storage_paths(&package)) {
                let parent = quote(parent);
                let target = quote(&path);
                let command = format!(
                    "if [ -d {parent} ]; then ls -A -- {parent} >/dev/null || exit 1; fi; \
                     rm -rf -- {target} && [ ! -e {target} ] && [ ! -L {target} ]"
                );
                if let Err(error) = adb.shell(&serial, &command).await {
                    cleanup_errors.push(format!("{path}\n{}", error.raw.unwrap_or(error.message)));
                }
            }
        }
        Ok(_) => cleanup_errors.push(
            "Shared-folder cleanup could only be verified from the headset’s owner profile.".into(),
        ),
        Err(error) => cleanup_errors.push(format!(
            "Could not verify shared-folder cleanup. {}",
            error.raw.unwrap_or(error.message)
        )),
    }
    Ok(Removal { cleanup_errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exact_package_and_preserves_large_version_code() {
        let apps = parse_apps(
            "package:com.casinovr.casinovrpoker versionCode:202605111\n\
             package:com.example.b versionCode:9007199254740993\n\
             package:com.example;broken versionCode:1\n",
        );
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].package, "com.casinovr.casinovrpoker");
        assert_eq!(apps[1].version_code, "9007199254740993");
        assert_eq!(
            storage_paths(&apps[0].package),
            [
                "/sdcard/Android/data/com.casinovr.casinovrpoker",
                "/sdcard/Android/obb/com.casinovr.casinovrpoker"
            ]
        );
    }
}
