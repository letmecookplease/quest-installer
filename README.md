# Quest Installer

An offline Tauri desktop app for installing and testing Unity builds on Meta Quest. Connect a developer-mode headset, allow USB debugging, then select an APK, ZIP, or build folder. Wireless debugging uses a direct local connection. Device commands are bound to the selected headset.

Settings stores the theme, headset names, watched folder, and recent builds locally. Dark is the default. All themes use bundled Fira Code at 450 for body text, 500 for controls and card titles, and 700 for page headings and emphasis. To add a theme, extend the theme definitions in `src/model.ts`, the color tokens in `src/styles.css`, and the allowed IDs in the `set_theme` command.

The header uses the active theme’s accent color for the bundled [Mega Particle symbol](https://www.megaparticle.com/).

## Build

Install Node.js, the Rust stable toolchain, the [Tauri host prerequisites](https://v2.tauri.app/start/prerequisites/), and Android SDK Platform-Tools and Build-Tools 35.0.0. Set `ANDROID_SDK_ROOT` when the SDK is outside the usual location. `QUEST_AAPT2` can point to another compatible AAPT2 binary.

```sh
npm ci
npm run stage-tools
npm run tauri -- build
```

Build on Windows x64 for the NSIS installer and on each supported macOS architecture for its app and DMG. Stage Android tools for the architecture you are packaging. For local development, run `npm run tauri -- dev` after staging.

The GitHub release workflow packages Windows x64, Apple Silicon, and Intel Mac installers. A manual run produces build artifacts for inspection. Pushing a matching version tag publishes all three installers and their checksums after the builds pass. The Mac packages use ad-hoc signing for internal distribution.

The Windows installer uses the system WebView2 Runtime and performs no WebView2 download. Install WebView2 separately if the target PC lacks it. The app bundles ADB and AAPT2 and keeps its ADB server on port 5038.

## Headset behavior

The selected build shows estimated install size and the headset’s current free space. The estimate includes the APK, matching OBB files, and native libraries expected to be extracted. Android setup and app data can use additional space.

Use **Apps** to search installed third-party packages on the selected headset. **Uninstall** shows the exact package, build, and destination before removing it from all profiles. The app verifies cleanup of the package’s Data and OBB folders when the owner profile is active. Quest capture folders are outside the cleanup.

The Files tab shows permission failures from the device, including restricted access to `Android/data`. Clean install can back up readable external app data to the app data directory under `backups`. A requested backup that fails stops the operation before uninstall. Android app-private data is outside the scope of that backup.

Files accepts drag-and-drop uploads. Use a row’s Download action to choose a destination folder. The Logs tab streams the main process of the selected build. Start it again after relaunching the app to attach to its new process.
