use crate::{
    adb::{self, SHORT},
    state::AppState,
    AppError, Result,
};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};
use tauri::State;

const QUEST_ABIS: [&str; 3] = ["arm64-v8a", "armeabi-v7a", "armeabi"];

fn manifest_flag(manifest: &str, name: &str) -> Option<bool> {
    let attribute = format!("A: http://schemas.android.com/apk/res/android:{name}(");
    let value = manifest.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(&attribute)?
            .split_once(")=")
            .map(|(_, value)| value.trim())
    })?;
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => {
            let hex = value.strip_prefix("(type 0x12)0x")?;
            u32::from_str_radix(hex, 16).ok().map(|value| value != 0)
        }
    }
}

fn native_library_bytes(apk: &Path, manifest: &str) -> Result<u64> {
    if manifest_flag(manifest, "extractNativeLibs") == Some(false) {
        return Ok(0);
    }
    let mut archive = zip::ZipArchive::new(File::open(apk)?)?;
    let mut sizes = [0_u64; QUEST_ABIS.len()];
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let Some((abi, name)) = entry
            .name()
            .strip_prefix("lib/")
            .and_then(|path| path.split_once('/'))
        else {
            continue;
        };
        if !name.starts_with("lib") || !name.ends_with(".so") || name.contains('/') {
            continue;
        }
        if let Some(index) = QUEST_ABIS.iter().position(|supported| *supported == abi) {
            sizes[index] = sizes[index].saturating_add(entry.size());
        }
    }
    let arm32 = sizes[1..]
        .iter()
        .copied()
        .find(|size| *size > 0)
        .unwrap_or(0);
    Ok(if manifest_flag(manifest, "multiArch") == Some(true) {
        sizes[0].saturating_add(arm32)
    } else if sizes[0] > 0 {
        sizes[0]
    } else {
        arm32
    })
}

pub async fn estimate_build_bytes(aapt: &Path, apk: &Path, obbs: &[PathBuf]) -> Result<u64> {
    let payload = obbs
        .iter()
        .try_fold(fs::metadata(apk)?.len(), |total, path| {
            fs::metadata(path).map(|metadata| total.saturating_add(metadata.len()))
        })?;
    let manifest = adb::output(
        adb::command(aapt)
            .args(["dump", "xmltree", "--file", "AndroidManifest.xml"])
            .arg(apk),
        SHORT,
    )
    .await;
    let unpacked = manifest
        .ok()
        .and_then(|manifest| native_library_bytes(apk, &manifest).ok())
        .unwrap_or(0);
    Ok(payload.saturating_add(unpacked))
}

fn parse_available_bytes(output: &str) -> Option<u64> {
    output.lines().skip(1).find_map(|line| {
        line.split_whitespace()
            .nth(3)?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}

#[tauri::command]
pub async fn available_space(serial: String, state: State<'_, AppState>) -> Result<u64> {
    state.require_device(&serial)?;
    let output = state.adb.shell(&serial, "df -kP /data").await?;
    parse_available_bytes(&output)
        .ok_or_else(|| AppError::new("storage", "Could not read the headset’s free space."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_available_kibibytes_from_quest_output() {
        let output = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/block/dm-49 117719436 16838192 100750172 15% /data/user/0";
        assert_eq!(parse_available_bytes(output), Some(103168176128));
        assert_eq!(parse_available_bytes("invalid"), None);
    }

    #[test]
    fn counts_only_the_expected_extracted_native_libraries() {
        let folder = tempfile::tempdir().unwrap();
        let apk = folder.path().join("build.apk");
        let mut archive = zip::ZipWriter::new(File::create(&apk).unwrap());
        for (name, bytes) in [
            ("lib/arm64-v8a/libgame.so", b"12345".as_slice()),
            ("lib/armeabi-v7a/libgame.so", b"123".as_slice()),
            ("lib/x86_64/libgame.so", b"1234567".as_slice()),
            ("assets/libgame.so", b"123456789".as_slice()),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        let attr = "A: http://schemas.android.com/apk/res/android:extractNativeLibs(0x010104ea)";
        assert_eq!(
            native_library_bytes(&apk, &format!("{attr}=true")).unwrap(),
            5
        );
        assert_eq!(
            native_library_bytes(&apk, &format!("{attr}=false")).unwrap(),
            0
        );
        assert_eq!(native_library_bytes(&apk, &format!("{attr}=true\nA: http://schemas.android.com/apk/res/android:multiArch(0x0101048e)=true")).unwrap(), 8);
    }
}
