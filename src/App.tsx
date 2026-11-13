import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { AppearanceSettings } from "./AppearanceSettings";
import { BuildStorage } from "./BuildStorage";
import { InstallProgress } from "./InstallProgress";
import { Build, DeviceSnapshot, Entry, Failure, InstalledApp, InstallResult, Mode, Page, Progress, Settings, Theme,
  defaultSettings, failure, filename, formatSize, modes, noDevices, themes } from "./model";
import { ErrorMessage, Modal } from "./ui";

const root = "/sdcard";
const deviceRoot = "/";
function sharedStorage(path: string) {
  return path === root || path.startsWith(root + "/");
}

function filePath(folder: string, name: string) {
  return folder === deviceRoot ? "/" + name : folder + "/" + name;
}

const quickFolders = [
  ["OBB", "/sdcard/Android/obb"], ["Data", "/sdcard/Android/data"],
  ["Screenshots", "/sdcard/Oculus/Screenshots"], ["Recordings", "/sdcard/Oculus/VideoShots"],
];
const pages: Page[] = ["Install", "Apps", "Files", "Logs", "Settings"];
const buildFilter = [{ name: "Quest builds", extensions: ["apk", "zip"] }];
const descriptions: Record<Page, string> = {
  Install: "Send a build to your headset.",
  Apps: "See and remove apps installed on your headset.",
  Files: "Browse and transfer files on your headset.",
  Logs: "Stream diagnostics for the selected build.",
  Settings: "Customize your workspace.",
};
type Prompt =
  | { kind: "install"; mode: "clean" | "wipe"; serial: string; device: string }
  | { kind: "uninstall"; app: InstalledApp; serial: string; device: string }
  | { kind: "mkdir" | "nickname"; serial: string }
  | { kind: "rename" | "delete"; entry: Entry; serial: string; device: string }
  | { kind: "wireless" }
  | { kind: "preview"; name: string; data: string };

export function App() {
  const [page, setPage] = useState<Page>("Install");
  const [snapshot, setSnapshot] = useState<DeviceSnapshot>(noDevices);
  const [selected, setSelected] = useState("");
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [build, setBuild] = useState<Build | null>(null);
  const [mode, setMode] = useState<Mode>("normal");
  const [backup, setBackup] = useState(false);
  const [launch, setLaunch] = useState(false);
  const [busy, setBusy] = useState("");
  const locked = useRef(false);
  const [error, setError] = useState<Failure | null>(null);
  const [notice, setNotice] = useState("");
  const [progress, setProgress] = useState<Progress | null>(null);
  const installTarget = useRef<string | null>(null);
  const [watched, setWatched] = useState<string[]>([]);
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [value, setValue] = useState("");
  const [pairCode, setPairCode] = useState("");
  const [dragging, setDragging] = useState(false);
  const [appQuery, setAppQuery] = useState("");
  const [appRefresh, setAppRefresh] = useState(0);
  const [inventory, setInventory] = useState<{ serial: string; apps: InstalledApp[]; error?: Failure } | null>(null);
  const [path, setPath] = useState(root);
  const [listing, setListing] = useState<{ serial: string; path: string; entries: Entry[]; error?: Failure } | null>(null);
  const [fileRefresh, setFileRefresh] = useState(0);
  const [reveal, setReveal] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [streaming, setStreaming] = useState(false);
  const logSession = useRef("");
  const logView = useRef<HTMLPreElement>(null);
  const dropHandler = useRef<(paths: string[]) => void>(() => {});

  const device = snapshot.devices.find((item) => item.serial === selected);
  const ready = device?.status === "device";
  const name = device ? (settings.nicknames[device.serial] || device.name) : "";
  const connection = device?.wireless ? "Wi-Fi" : "USB";
  const modeName = modes.find((item) => item.id === mode)!.label;
  const currentListing = listing?.serial === selected && listing.path === path ? listing : null;
  const files = currentListing?.entries ?? [];
  const fileError = currentListing?.error;
  const filesLoading = ready && !currentListing;
  const writableFolder = sharedStorage(path);
  const fileReady = ready && !!currentListing && !fileError && writableFolder;
  const pathParts = path.split("/").filter(Boolean);

  useEffect(() => {
    const theme = themes.some((option) => option.id === settings.theme) ? settings.theme : "dark";
    document.documentElement.dataset.theme = theme;
  }, [settings.theme]);

  useEffect(() => {
    let live = true;
    let receivedDevices = false;
    const subscriptions = [
      listen<DeviceSnapshot>("devices:changed", ({ payload }) => {
        receivedDevices = true;
        if (live) setSnapshot(payload);
      }).then(async (unsubscribe) => {
        if (!live) { unsubscribe(); return unsubscribe; }
        const initial = await invoke<DeviceSnapshot>("devices");
        if (live && !receivedDevices) setSnapshot(initial);
        return unsubscribe;
      }),
      listen<Progress>("install:progress", ({ payload }) => {
        if (live && payload.serial === installTarget.current) setProgress(payload);
      }),
      listen<string>("build:watched", ({ payload }) => {
        if (live) setWatched((items) => [payload, ...items.filter((item) => item !== payload)].slice(0, 8));
      }),
      listen<{ session: string; line: string | null }>("logs:entry", ({ payload }) => {
        if (!live || payload.session !== logSession.current) return;
        if (payload.line === null) setStreaming(false);
        else setLogs((lines) => [...lines.slice(-999), payload.line!]);
      }),
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        if (!live) return;
        setDragging(payload.type === "enter" || payload.type === "over");
        if (payload.type === "drop") dropHandler.current(payload.paths);
      }),
    ];
    void Promise.all(subscriptions).catch((cause) => { if (live) setError(failure(cause)); });
    void invoke<Settings>("settings").then((next) => { if (live) setSettings(next); })
      .catch((cause) => { if (live) setError(failure(cause)); });
    return () => {
      live = false;
      void Promise.allSettled(subscriptions).then((results) => {
        for (const result of results) if (result.status === "fulfilled") result.value();
      });
    };
  }, []);

  useEffect(() => {
    if (snapshot.devices.some((item) => item.serial === selected)) return;
    setSelected(snapshot.devices.find((item) => item.status === "device")?.serial ?? snapshot.devices[0]?.serial ?? "");
  }, [snapshot, selected]);

  useEffect(() => {
    setPath(root);
    setListing(null);
  }, [selected, ready]);

  useEffect(() => {
    logSession.current = "";
    setStreaming(false);
    setLogs([]);
    void invoke("stop_logs").catch(() => {});
  }, [selected, ready, build?.package]);

  useEffect(() => {
    if (page !== "Apps" || !ready) return;
    let live = true;
    setInventory(null);
    void invoke<InstalledApp[]>("list_installed_apps", { serial: selected }).then((apps) => {
      if (live) setInventory({ serial: selected, apps });
    }).catch((cause) => {
      if (live) setInventory({ serial: selected, apps: [], error: failure(cause) });
    });
    return () => { live = false; };
  }, [page, ready, selected, appRefresh]);

  useEffect(() => {
    if (page !== "Files" || !ready) return;
    let live = true;
    void invoke<Entry[]>("list_files", { serial: selected, path }).then((entries) => {
      if (live) setListing({ serial: selected, path, entries });
    }).catch((cause) => {
      if (live) setListing({ serial: selected, path, entries: [], error: failure(cause) });
    });
    return () => { live = false; };
  }, [page, ready, selected, path, fileRefresh]);

  useEffect(() => {
    if (streaming && logView.current) logView.current.scrollTop = logView.current.scrollHeight;
  }, [logs, streaming]);

  async function work(label: string, run: () => Promise<void>) {
    if (locked.current) return;
    locked.current = true;
    setBusy(label);
    setError(null);
    setNotice("");
    try { await run(); }
    catch (cause) { setError(failure(cause)); }
    finally { setBusy(""); locked.current = false; }
  }

  async function inspect(source: string) {
    setBuild(null);
    const next = await invoke<Build>("inspect_build", { source });
    setBuild(next);
    setWatched((items) => items.filter((item) => item !== source));
  }

  function chooseBuild(directory: boolean) {
    void work("Choosing build…", async () => {
      const source = await open({ directory, multiple: false, ...(directory ? {} : { filters: buildFilter }) });
      if (typeof source === "string") {
        setBusy("Reading build…");
        await inspect(source);
      }
    });
  }

  async function runInstall(targetMode: Mode) {
    installTarget.current = selected;
    setProgress({ serial: selected, stage: "Preparing operation", step: 0, total: 0, transfer: null });
    try {
      const result = await invoke<InstallResult>("run_install", {
        serial: selected, mode: targetMode, backup: targetMode === "clean" && backup, launch,
      });
      setSettings((current) => ({ ...current, history: result.history }));
      const message = targetMode === "wipe" ? `App data cleared on ${name}.` : `Build installed on ${name}.`;
      setNotice(result.notice ? `${message} ${result.notice}` : message);
    } finally {
      installTarget.current = null;
      setProgress(null);
    }
  }

  function install(targetMode: Mode) {
    setPrompt(null);
    void work("Working on headset…", () => runInstall(targetMode));
  }

  function uninstall(app: InstalledApp, serial: string) {
    setPrompt(null);
    void work("Uninstalling app…", async () => {
      try {
        const result = await invoke<{ cleanup_errors: string[] }>("uninstall_app", {
          serial, package: app.package, versionCode: app.versionCode,
        });
        if (result.cleanup_errors.length) {
          setError({ code: "cleanup", message: "App uninstalled. Some folders could not be cleared or verified.",
            raw: result.cleanup_errors.join("\n\n") });
        } else {
          setNotice(`${app.package} was removed from ${name}.`);
        }
      } finally {
        setAppRefresh((count) => count + 1);
      }
    });
  }

  function reinstall(source: string) {
    setMode("normal");
    void work("Reading build…", async () => {
      await inspect(source);
      await runInstall("normal");
    });
  }

  function refreshFiles() {
    setListing(null);
    setFileRefresh((count) => count + 1);
  }

  function browse(next: string) {
    refreshFiles();
    setPath(next);
    setError(null);
  }

  async function transfer(action: string, input: { name?: string; newName?: string; local?: string } = {}) {
    await invoke("file_action", { serial: selected, path, action, reveal, ...input });
    if (action !== "pull") refreshFiles();
  }

  function upload() {
    void work("Uploading…", async () => {
      const local = await open({ multiple: false });
      if (typeof local === "string") { await transfer("push", { local }); setNotice("Upload complete."); }
    });
  }

  function download(entry: Entry) {
    void work("Downloading…", async () => {
      const local = await open({ directory: true, multiple: false });
      if (typeof local === "string") { await transfer("pull", { name: entry.name, local }); setNotice("Download complete."); }
    });
  }

  dropHandler.current = (paths) => {
    if (prompt || !paths.length) return;
    if (page === "Install") {
      void work("Reading build…", () => inspect(paths[0]));
    } else if (page === "Files" && fileReady) {
      void work("Uploading…", async () => {
        for (const local of paths) await transfer("push", { local });
        setNotice("Upload complete.");
      });
    }
  };

  function control(action: "launch" | "stop") {
    void work(action === "launch" ? "Opening installed app…" : "Closing running app…", async () => {
      await invoke("app_control", { serial: selected, action });
      setNotice(action === "launch" ? `App opened on ${name}.` : `App closed on ${name}.`);
    });
  }

  function startLogs() {
    void work("Starting log stream…", async () => {
      const session = crypto.getRandomValues(new Uint32Array(4)).join("-");
      logSession.current = session;
      setLogs([]);
      setStreaming(true);
      try { await invoke("start_logs", { serial: selected, package: build?.package, session }); }
      catch (cause) { setStreaming(false); throw cause; }
    });
  }

  function stopLogs() {
    void work("Stopping log stream…", async () => {
      await invoke("stop_logs");
      logSession.current = "";
      setStreaming(false);
    });
  }

  function exportLogs() {
    void work("Saving logs…", async () => {
      const target = await save({ defaultPath: `${build?.package ?? "quest"}-log.txt`, filters: [{ name: "Text log", extensions: ["txt"] }] });
      if (target) { await invoke("save_logs", { path: target, text: logs.join("\n") + "\n" }); setNotice("Log saved."); }
    });
  }

  function updateTheme(theme: Theme) {
    void work("Saving theme…", async () => {
      const updated = await invoke<Settings>("set_theme", { theme });
      setSettings(updated);
    });
  }

  function watchFolder(clear: boolean) {
    void work("Saving watched folder…", async () => {
      const folder = clear ? null : await open({ directory: true, multiple: false });
      if (typeof folder === "string" || clear) {
        setSettings(await invoke<Settings>("set_watched_folder", { folder }));
        setWatched([]);
      }
    });
  }

  function openPrompt(next: Prompt, initial = "") {
    setValue(initial);
    setPairCode("");
    setPrompt(next);
  }

  function confirmInstall(mode: "clean" | "wipe") {
    openPrompt({ kind: "install", mode, serial: selected, device: name });
  }

  function confirmText(event: React.FormEvent) {
    event.preventDefault();
    const current = prompt;
    if (!current) return;
    if ("serial" in current && (current.serial !== selected || !ready)) return;
    setPrompt(null);
    void work("Saving changes…", async () => {
      if (current.kind === "nickname") {
        setSettings(await invoke<Settings>("set_nickname", { serial: selected, nickname: value }));
      } else if (current.kind === "mkdir") {
        await transfer("mkdir", { name: value });
      } else if (current.kind === "rename") {
        await transfer("rename", { name: current.entry.name, newName: value });
      }
    });
  }

  function wireless(action: "pair" | "connect" | "usb") {
    setPrompt(null);
    void work("Connecting headset…", async () => {
      const reply = await invoke<string>("wireless", { action, serial: selected, endpoint: value, code: pairCode });
      setNotice(action === "pair" ? "Headset paired. Connect using the address shown on its Wireless debugging screen."
        : action === "usb" ? "Wireless debugging enabled on port 5555. Connect using the headset’s local IP address." : reply);
    });
  }

  function go(next: Page) {
    if (next === "Apps" && page !== next) setInventory(null);
    if (next === "Files" && page !== next) setListing(null);
    setPage(next);
    setError(null);
    setNotice("");
  }

  const promptTargetLost = !!prompt && "serial" in prompt && (prompt.serial !== selected || !ready);
  const selectedInventory = inventory?.serial === selected ? inventory : null;
  const visibleApps = selectedInventory?.apps.filter((app) => app.package.toLowerCase().includes(appQuery.trim().toLowerCase())) ?? [];
  return <div className="shell">
    <header className="topbar">
      <div className="brand whitespace-nowrap"><span className="mark" aria-hidden="true" />Quest Installer</div>
      <nav className="navigation" aria-label="Workspace">
        {pages.map((item) => <button key={item} aria-current={item === page ? "page" : undefined} disabled={!!busy} onClick={() => go(item)}>{item}</button>)}
      </nav>
    </header>
    <aside className="sidebar" aria-labelledby="headsets-label">
      <h2 className="label" id="headsets-label">Headsets</h2>
      {snapshot.loading && <p className="device-message" role="status">Looking for headsets…</p>}
      {snapshot.error && <p className="device-message" role="status">{snapshot.error}</p>}
      {!snapshot.loading && !snapshot.error && !snapshot.devices.length &&
        <p className="device-message">Connect by USB and allow USB debugging on your Quest.</p>}
      <div className="device-list">
        {snapshot.devices.map((item) => <button key={item.serial} type="button" className="device"
          aria-pressed={item.serial === selected} disabled={!!busy} onClick={() => setSelected(item.serial)}>
          <strong>{settings.nicknames[item.serial] || item.name}</strong>
          <small><span className={`dot ${item.status !== "device" ? "pending" : ""}`} />
            {item.status === "device" ? (item.wireless ? "Wi-Fi connected" : "USB connected")
              : item.status === "unauthorized" ? "Allow debugging on headset" : item.status}</small>
        </button>)}
      </div>
      <button className="button quiet" disabled={!!busy} onClick={() => openPrompt({ kind: "wireless" })}>Connect over Wi-Fi</button>
      {ready && <button className="button quiet" disabled={!!busy} onClick={() => openPrompt({ kind: "nickname", serial: selected }, settings.nicknames[selected] || "")}>Edit headset name</button>}
    </aside>
    <main id="main">
      <header className="heading">
        <div><h1>{page}</h1><p>{descriptions[page]}</p></div>
        {page !== "Settings" && <div className="heading-device">
          <span className="label">Destination</span>
          <strong>{ready ? <><span className="dot" />{name} · {connection}</> : name || "No headset connected"}</strong>
        </div>}
      </header>
      <div className="feedback">
        {error && <ErrorMessage error={error}>
          {error.code === "signature" && ready && build && <button className="button" onClick={() => {
            setMode("clean"); confirmInstall("clean");
          }}>Retry as clean install</button>}
        </ErrorMessage>}
        {progress && <InstallProgress progress={progress} />}
        {busy && !progress && <div className="notice" role="status">{busy}</div>}
        {notice && <div className="notice success" role="status">{notice}</div>}
      </div>
      {page === "Install" && <>
        <div className="columns">
          <div>
            <section className="panel panel-body" aria-labelledby="build-heading">
              <h2 id="build-heading">Selected build</h2>
              <div className={`drop ${dragging ? "is-over" : ""}`}>
                <div className="drop-text">
                  <strong>Drop a build here</strong>
                  <small>APK, ZIP or build folder</small>
                </div>
                <div className="button-row">
                  <button className="button" disabled={!!busy} onClick={() => chooseBuild(false)}>Browse files</button>
                  <button className="button" disabled={!!busy} onClick={() => chooseBuild(true)}>Browse folders</button>
                </div>
              </div>
              {build && <>
                <div className="build-name">{build.name}</div>
                <div className="mono muted">{build.package}</div>
                <dl>
                  <dt>Version</dt>
                  <dd className="mono">{build.version || "—"}{build.versionCode && ` · build ${build.versionCode}`}</dd>
                  <dt>Expansion</dt>
                  <dd className="mono">
                    {build.expansions.length ? build.expansions.map((item) => <div key={item}>{item}</div>) : "None"}
                  </dd>
                </dl>
                <BuildStorage estimatedBytes={build.estimatedBytes} serial={ready ? selected : null} busy={!!busy} />
              </>}
            </section>
            {build && <details className="panel installed-controls below">
              <summary>Installed app controls</summary>
              <div className="panel-body">
                <p className="muted">These buttons use the copy already installed on your headset.</p>
                <div className="button-row controls">
                  <button className="button" disabled={!ready || !!busy} onClick={() => control("launch")}>Open installed app</button>
                  <button className="button" disabled={!ready || !!busy} onClick={() => control("stop")}>Close running app</button>
                </div>
              </div>
            </details>}
          </div>
          <section className="panel" aria-labelledby="mode-heading">
            <div className="panel-body"><h2 id="mode-heading">Install mode</h2>
              <fieldset aria-labelledby="mode-heading" disabled={!!busy}>
                {modes.map((item) => <label className="mode" key={item.id}>
                  <input type="radio" name="mode" value={item.id} checked={mode === item.id} onChange={() => setMode(item.id)} />
                  <span><strong>{item.label}</strong><small>{item.description}</small></span>
                </label>)}
              </fieldset>
              {mode === "clean" && <label className="check"><input type="checkbox" checked={backup} disabled={!!busy} onChange={(event) => setBackup(event.target.checked)} />Back up accessible saves first</label>}
              {mode !== "wipe" && <label className="check"><input type="checkbox" checked={launch} disabled={!!busy} onChange={(event) => setLaunch(event.target.checked)} />Open this build after installing</label>}
            </div>
            <div className="panel-footer"><button className={`button ${mode === "wipe" ? "danger" : "primary"}`}
              disabled={!ready || !build || !!busy} onClick={() => mode === "normal" ? void install(mode) : confirmInstall(mode)}>
              {mode === "normal" ? "Install this build" : modeName}{ready ? ` on ${name}` : ""}
            </button></div>
          </section>
        </div>
        {!!watched.length && <section className="panel panel-body below"><h2>New builds</h2>
          {watched.map((source) => <div className="list-row" key={source}>
            <span className="mono">{filename(source)}</span><button className="button" disabled={!!busy} onClick={() => void work("Reading build…", () => inspect(source))}>Review build</button>
          </div>)}
        </section>}
        {!!settings.history.length && <section className="panel panel-body below"><h2>Recent builds</h2>
          {settings.history.map((entry) => <div className="list-row" key={entry.source}>
            <div><strong>{entry.name}</strong><small className="mono">{entry.version} · {entry.source}</small></div>
            <button className="button" disabled={!!busy || !ready} onClick={() => reinstall(entry.source)}
              aria-label={`Reinstall ${entry.name} on ${name}`}>Reinstall</button>
          </div>)}
        </section>}
      </>}
      {page === "Apps" && <section className="panel panel-body" aria-labelledby="apps-heading">
        <div className="files-toolbar">
          <div><h2 id="apps-heading">Installed apps</h2><p className="muted">Apps added to the active headset profile.</p></div>
          {ready && <div className="button-row apps-controls">
            <input type="search" aria-label="Search installed apps" placeholder="Search package IDs" value={appQuery}
              onChange={(event) => setAppQuery(event.target.value)} />
            <button className="button" disabled={!!busy || !selectedInventory} onClick={() => setAppRefresh((count) => count + 1)}>Refresh</button>
          </div>}
        </div>
        {!ready ? <p className="empty-state">Connect a headset to see its installed apps.</p>
          : !selectedInventory ? <p className="empty-state" role="status">Reading installed apps…</p>
          : selectedInventory.error ? <ErrorMessage error={selectedInventory.error} />
          : !visibleApps.length ? <p className="empty-state">{appQuery.trim() ? "No apps match that search." : "No apps found."}</p>
          : <div className="file-scroll apps-list"><table>
            <thead><tr><th scope="col">Package ID</th><th scope="col">Build</th><th scope="col">Actions</th></tr></thead>
            <tbody>{visibleApps.map((app) => <tr key={app.package}>
              <td className="mono">{app.package}</td><td className="mono">{app.versionCode}</td>
              <td><div className="file-actions button-row">
                <button className="button quiet" aria-label={`Uninstall ${app.package}`} disabled={!!busy}
                  onClick={() => openPrompt({ kind: "uninstall", app, serial: selected, device: name })}>Uninstall</button>
              </div></td>
            </tr>)}</tbody>
          </table></div>}
      </section>}
      {page === "Files" && <section className={`panel panel-body ${dragging && fileReady ? "is-over" : ""}`} aria-label="Headset files">
        {!ready ? <p className="muted">Connect a headset to browse its storage.</p> : <>
          <nav className="button-row breadcrumb" aria-label="Folder path">
            <button className="button quiet" disabled={!!busy} aria-current={path === deviceRoot ? "location" : undefined}
              onClick={() => browse(deviceRoot)}>Device root</button>
            {pathParts.map((part, index) => <button className="button quiet" disabled={!!busy}
              aria-current={index === pathParts.length - 1 ? "location" : undefined} key={index}
              onClick={() => browse("/" + pathParts.slice(0, index + 1).join("/"))}>
              <span className="breadcrumb-divider" aria-hidden="true">/</span>
              {index === 0 && part === "sdcard" ? "Internal storage" : part}
            </button>)}
          </nav>
          <div className="files-toolbar">
            <div className="button-row">
              {quickFolders.map(([label, next]) => <button className="button" key={label} disabled={!!busy} onClick={() => browse(next)}>{label}</button>)}
              <button className="button" disabled={!!busy} onClick={() => browse(path)}>Refresh</button>
            </div>
            {writableFolder ? <div className="button-row"><button className="button" disabled={!!busy || !fileReady} onClick={() => openPrompt({ kind: "mkdir", serial: selected })}>New folder</button>
              <button className="button primary" disabled={!!busy || !fileReady} onClick={upload}>Upload files</button></div>
              : <button className="button" disabled={!!busy} onClick={() => browse(root)}>Internal storage</button>}
          </div>
          {fileError && <div className="below"><ErrorMessage error={fileError} /></div>}
          {filesLoading ? <p className="muted" role="status">Reading folder…</p> : !fileError && <>
            <label className="check"><input type="checkbox" checked={reveal} onChange={(event) => setReveal(event.target.checked)} />Open folder after download</label>
            {!files.length ? <p className="empty-state">This folder is empty.{writableFolder && " Drop files here to upload."}</p> :
              <div className="file-scroll"><table className="files-table"><thead><tr><th>Name</th><th>Size</th><th>Modified</th><th>Actions</th></tr></thead>
                <tbody>{files.map((entry) => <tr key={entry.name}>
                  <td>{entry.directory ? <button className="button quiet mono" disabled={!!busy} onClick={() => browse(filePath(path, entry.name))}>{entry.name}/</button>
                    : entry.regular && /\.(png|jpe?g|webp)$/i.test(entry.name) ? <button className="button quiet mono" disabled={!!busy} onClick={() => void work("Opening preview…", async () => {
                      const data = await invoke<string>("preview_image", { serial: selected, path, name: entry.name });
                      openPrompt({ kind: "preview", name: entry.name, data });
                    })}>{entry.name}</button> : <span className="mono">{entry.name}</span>}</td>
                  <td className="mono">{formatSize(entry.size)}</td><td className="mono">{entry.modified}</td>
                  <td><div className="button-row file-actions">
                    {(entry.regular || (writableFolder && entry.directory)) && <button className="button quiet" aria-label={`Download ${entry.name}`} disabled={!!busy} onClick={() => download(entry)}>Download</button>}
                    {writableFolder && <>
                      <button className="button quiet" aria-label={`Rename ${entry.name}`} disabled={!!busy} onClick={() => openPrompt({ kind: "rename", entry, serial: selected, device: name }, entry.name)}>Rename</button>
                      <button className="button quiet" aria-label={`Delete ${entry.name}`} disabled={!!busy} onClick={() => openPrompt({ kind: "delete", entry, serial: selected, device: name })}>Delete</button>
                    </>}
                  </div></td>
                </tr>)}</tbody></table></div>}
          </>}
        </>}
      </section>}
      {page === "Logs" && <section className="panel panel-body" aria-labelledby="log-heading">
        <div className="files-toolbar"><div><h2 id="log-heading">App log</h2><p className="muted mono">{build?.package ?? "Select a build to filter logs."}</p></div>
          <div className="button-row"><button className="button" disabled={!!busy || !logs.length} onClick={exportLogs}>Save log</button>
            <button className="button primary" disabled={!!busy || (!streaming && (!ready || !build))} onClick={streaming ? stopLogs : startLogs}>{streaming ? "Stop stream" : "Start stream"}</button></div>
        </div>
        <pre className="log" ref={logView} tabIndex={0} aria-label="Latest app logs">{logs.join("\n") || (streaming ? "Waiting for log entries…" : "Launch your build on the headset, then start streaming.")}</pre>
      </section>}
      {page === "Settings" && <div className="columns settings-columns">
        <AppearanceSettings selected={settings.theme} disabled={!!busy} onChange={updateTheme} />
        <section className="panel panel-body" aria-labelledby="watched-heading">
          <h2 id="watched-heading">Watched folder</h2>
          <p className="muted section-copy">Show new APK and ZIP builds on the Install screen.</p>
          {settings.watchedFolder && <p className="mono watched-path">{settings.watchedFolder}</p>}
          <div className="button-row controls"><button className="button" disabled={!!busy} onClick={() => watchFolder(false)}>{settings.watchedFolder ? "Change folder" : "Choose folder"}</button>
            {settings.watchedFolder && <button className="button quiet" disabled={!!busy} onClick={() => watchFolder(true)}>Stop watching</button>}</div>
        </section>
      </div>}
    </main>
    {prompt?.kind === "install" && build && <Modal title={`${prompt.mode === "wipe" ? "Clear app data" : "Clean install"} on ${prompt.device}?`} close={() => setPrompt(null)}>
      <p className="muted">{prompt.mode === "wipe" ? "This clears app data and keeps the installed build." : "This uninstalls the old app and removes its data before reinstalling."}</p>
      <p className="mono">{build.package}</p>
      {prompt.mode === "clean" && backup && <p className="muted">Accessible saves will be backed up first.</p>}
      {promptTargetLost && <p role="status">The selected headset disconnected.</p>}
      <div className="dialog-actions"><button className="button" autoFocus onClick={() => setPrompt(null)}>Cancel</button>
        <button className="button danger" disabled={promptTargetLost} onClick={() => void install(prompt.mode)}>{prompt.mode === "wipe" ? "Clear app data" : "Clean install"}</button></div>
    </Modal>}
    {prompt?.kind === "delete" && <Modal title={`Delete ${prompt.entry.name}?`} close={() => setPrompt(null)}>
      <p className="muted">This permanently removes the {prompt.entry.directory ? "folder and its contents" : "file"} from {prompt.device}.</p>
      {promptTargetLost && <p role="status">The selected headset disconnected.</p>}
      <div className="dialog-actions"><button className="button" autoFocus onClick={() => setPrompt(null)}>Cancel</button>
        <button className="button danger" disabled={promptTargetLost} onClick={() => { setPrompt(null); void work("Deleting…", () => transfer("delete", { name: prompt.entry.name })); }}>Delete</button></div>
    </Modal>}
    {prompt?.kind === "uninstall" && <Modal title={`Uninstall from ${prompt.device}?`} close={() => setPrompt(null)}>
      <p className="mono">{prompt.app.package} · build {prompt.app.versionCode}</p>
      <p className="muted">This removes the app for all headset profiles, including its local saves, settings, and extra game files.</p>
      <p className="muted">Screenshots and recordings in the Quest’s capture folders stay on the headset.</p>
      {promptTargetLost && <p role="status">The selected headset disconnected.</p>}
      <div className="dialog-actions"><button className="button" autoFocus onClick={() => setPrompt(null)}>Cancel</button>
        <button className="button danger" disabled={promptTargetLost} onClick={() => uninstall(prompt.app, prompt.serial)}>Uninstall</button></div>
    </Modal>}
    {prompt?.kind === "preview" && <Modal title={prompt.name} close={() => setPrompt(null)}>
      <img className="image-preview" src={prompt.data} alt={prompt.name} />
      <div className="dialog-actions"><button className="button" autoFocus onClick={() => setPrompt(null)}>Close</button></div>
    </Modal>}
    {(prompt?.kind === "nickname" || prompt?.kind === "mkdir" || prompt?.kind === "rename") && <Modal
      title={prompt.kind === "nickname" ? "Edit headset name" : prompt.kind === "mkdir" ? "New folder" : "Rename item"} close={() => setPrompt(null)}>
      <form onSubmit={confirmText}><label className="field">Name
        <input autoFocus value={value} required={prompt.kind !== "nickname"} maxLength={prompt.kind === "nickname" ? 32 : 255} onChange={(event) => setValue(event.target.value)} /></label>
        <div className="dialog-actions"><button className="button" type="button" onClick={() => setPrompt(null)}>Cancel</button>
          <button className="button primary" type="submit" disabled={promptTargetLost}>Save</button></div></form>
    </Modal>}
    {prompt?.kind === "wireless" && <Modal title="Connect over Wi-Fi" close={() => setPrompt(null)}>
      <p className="muted">Use the pairing address and code to pair. Then enter the connection address from Wireless debugging.</p>
      <label className="field">Headset IP and port<input autoFocus value={value} placeholder="192.168.1.20:5555" onChange={(event) => setValue(event.target.value)} /></label>
      <label className="field">Pairing code<input inputMode="numeric" value={pairCode} maxLength={6} onChange={(event) => setPairCode(event.target.value)} /></label>
      {ready && !device?.wireless && <button className="button quiet" onClick={() => wireless("usb")}>Enable Wi-Fi debugging over USB</button>}
      <div className="dialog-actions"><button className="button" onClick={() => setPrompt(null)}>Cancel</button>
        <button className="button" disabled={!value.trim() || !/^\d{6}$/.test(pairCode)} onClick={() => wireless("pair")}>Pair</button>
        <button className="button primary" disabled={!value.trim()} onClick={() => wireless("connect")}>Connect</button></div>
    </Modal>}
  </div>;
}
