export const themes = [
  { id: "dark", name: "Dark", description: "Matte charcoal with a periwinkle accent.", additional: false },
  { id: "light", name: "Light", description: "Cool paper with a deeper blue accent.", additional: false },
  { id: "sage-lime", name: "Sage & Lime", description: "Sage navigation, paper panels, and lime actions.", additional: true },
] as const;
export type Theme = (typeof themes)[number]["id"];

export type Page = "Install" | "Apps" | "Files" | "Logs" | "Settings";
export interface InstalledApp { package: string; versionCode: string }
export type Mode = "normal" | "clean" | "wipe";
export const modes: { id: Mode; label: string; description: string }[] = [
  { id: "normal", label: "Normal", description: "Install this build and keep existing app data." },
  { id: "clean", label: "Clean install", description: "Remove the installed copy and its data, then install this build." },
  { id: "wipe", label: "Clear app data", description: "Clear data from the installed copy and keep it installed." },
];

export interface Device { serial: string; status: string; name: string; wireless: boolean }
export interface DeviceSnapshot { devices: Device[]; error: string | null; loading: boolean }
export interface Build {
  name: string;
  package: string;
  version: string;
  versionCode: string;
  expansions: string[];
  estimatedBytes: number;
}
export interface History { source: string; name: string; version: string }
export interface Settings { theme: Theme; watchedFolder: string | null; nicknames: Record<string, string>; history: History[] }
export interface Progress {
  serial: string;
  stage: string;
  step: number;
  total: number;
  transfer: { sent: number; total: number } | null;
}
export interface InstallResult { history: History[]; notice: string | null }
export interface Entry { name: string; directory: boolean; size: number | null; modified: string }
export interface Failure { code: string; message: string; raw: string | null }

export const defaultSettings: Settings = { theme: "dark", watchedFolder: null, nicknames: {}, history: [] };
export const noDevices: DeviceSnapshot = { devices: [], error: null, loading: true };

export function failure(value: unknown): Failure {
  if (typeof value === "object" && value !== null && "message" in value) {
    const error = value as Partial<Failure>;
    return { code: error.code ?? "error", message: String(error.message), raw: error.raw ?? null };
  }
  return { code: "error", message: String(value), raw: null };
}

export function filename(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

export function formatSize(bytes: number | null) {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}
