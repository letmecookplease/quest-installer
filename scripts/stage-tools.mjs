import { copyFile, mkdir, chmod, access } from "node:fs/promises";
import { dirname, resolve, join } from "node:path";
import { platform, arch, homedir } from "node:os";

const windows = platform() === "win32";
if (!windows && platform() !== "darwin") throw new Error("Build on Windows or macOS.");
const sdk = process.env.ANDROID_SDK_ROOT || process.env.ANDROID_HOME
  || (windows ? join(homedir(), "AppData", "Local", "Android", "Sdk") : join(homedir(), "Library", "Android", "sdk"));
const target = process.env.QUEST_BUILD_TARGET || (windows ? "x86_64-pc-windows-msvc"
  : arch() === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin");
const extension = windows ? ".exe" : "";
const binaries = resolve("src-tauri/binaries");
const tools = join(sdk, "platform-tools");
const aapt = process.env.QUEST_AAPT2 || join(sdk, "build-tools", "35.0.0", "aapt2" + extension);
await mkdir(binaries, { recursive: true });
async function stage(source, name) {
  await access(source).catch(() => { throw new Error(`Missing Android SDK file: ${source}`); });
  const destination = join(binaries, name);
  await copyFile(source, destination);
  if (!windows) await chmod(destination, name.endsWith(".txt") ? 0o644 : 0o755);
}
await stage(join(tools, "adb" + extension), `adb-${target}${extension}`);
await stage(aapt, `aapt2-${target}${extension}`);
await stage(join(tools, "NOTICE.txt"), "Android-Platform-Tools.txt");
await stage(join(dirname(aapt), "NOTICE.txt"), "Android-Build-Tools.txt");
if (windows) {
  for (const name of ["AdbWinApi.dll", "AdbWinUsbApi.dll"]) await stage(join(tools, name), name);
}
