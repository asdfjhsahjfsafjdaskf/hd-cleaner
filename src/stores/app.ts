import { create } from "zustand";
import { api, toError } from "../services/api";
import { DEFAULT_LANG, useLang, type Lang } from "../i18n";
import type { AppInfo, DeleteMode, DriveInfo, ErrorPayload } from "../types";
import type { Target } from "../services/api";

export type PageId =
  | "dashboard" | "analyzer" | "changes" | "largeFiles" | "duplicates"
  | "programs" | "windowsApps" | "cleaner" | "startup" | "processes" | "monitor" | "backups"
  | "history" | "tools" | "settings";

export interface Settings {
  theme: "dark" | "light" | "system";
  language: Lang;
  fontScale: number;
  preferFastScan: boolean;
  followJunctions: boolean;
  saveSnapshots: boolean;
  treemapMetric: "allocated" | "logical";
  treemapMaxRects: number;
  useRecycleBin: boolean;
  dryRunDefault: boolean;
  debugLogs: boolean;
  createRestorePoint: boolean;
  defaultLeftoverLevel: "safe" | "moderate" | "advanced";
}

export const DEFAULT_SETTINGS: Settings = {
  theme: "dark",
  language: DEFAULT_LANG,
  fontScale: 1,
  preferFastScan: true,
  followJunctions: false,
  saveSnapshots: true,
  treemapMetric: "allocated",
  treemapMaxRects: 60000,
  useRecycleBin: true,
  dryRunDefault: false,
  debugLogs: false,
  createRestorePoint: false,
  defaultLeftoverLevel: "moderate",
};

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
  error?: ErrorPayload;
}

export interface DeleteRequest {
  scanId?: number;
  targets: Target[];
  mode: DeleteMode;
  onDone?: () => void;
}

interface AppState {
  page: PageId;
  info?: AppInfo;
  drives: DriveInfo[];
  settings: Settings;
  settingsLoaded: boolean;
  toasts: Toast[];
  deleteRequest?: DeleteRequest;
  renameRequest?: { path: string; name: string; onDone?: (newPath: string) => void };
  uninstallRequest?: { programId: string };
  requestUninstall: (programId: string) => void;
  closeUninstall: () => void;
  forcedRequest?: { name?: string; exe?: string; folder?: string; programId?: string };
  requestForced: (r: { name?: string; exe?: string; folder?: string; programId?: string }) => void;
  closeForced: () => void;
  batchRequest?: { ids: string[]; quick: boolean };
  requestBatch: (ids: string[], quick: boolean) => void;
  closeBatch: () => void;
  /** Target Mode dialog (the window picker starts from it). */
  targetOpen: boolean;
  openTarget: () => void;
  closeTarget: () => void;
  navigate: (p: PageId) => void;
  init: () => Promise<void>;
  refreshDrives: () => Promise<void>;
  setSetting: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  toast: (kind: Toast["kind"], text: string, error?: ErrorPayload) => void;
  toastError: (e: unknown) => void;
  dismissToast: (id: number) => void;
  requestDelete: (r: DeleteRequest) => void;
  closeDelete: () => void;
  requestRename: (path: string, onDone?: (newPath: string) => void) => void;
  closeRename: () => void;
}

let toastSeq = 1;

export function applyAppearance(s: Settings) {
  const root = document.documentElement;
  const theme = s.theme === "system"
    ? (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : s.theme;
  root.dataset.theme = theme;
  root.style.setProperty("--font-scale", String(s.fontScale));
  useLang.getState().setLang(s.language);
}

export const useApp = create<AppState>((set, get) => ({
  page: "dashboard",
  drives: [],
  settings: DEFAULT_SETTINGS,
  settingsLoaded: false,
  toasts: [],
  navigate: (page) => set({ page }),
  init: async () => {
    try {
      const [info, stored] = await Promise.all([api.appInfo(), api.getSettings()]);
      const settings = { ...DEFAULT_SETTINGS, ...(stored as Partial<Settings>) };
      applyAppearance(settings);
      set({ info, settings, settingsLoaded: true });
    } catch (e) {
      applyAppearance(DEFAULT_SETTINGS);
      set({ settingsLoaded: true });
      get().toastError(e);
    }
    await get().refreshDrives();
  },
  refreshDrives: async () => {
    try {
      set({ drives: await api.listDrives() });
    } catch (e) {
      get().toastError(e);
    }
  },
  setSetting: (key, value) => {
    const settings = { ...get().settings, [key]: value };
    set({ settings });
    applyAppearance(settings);
    api.setSetting(key, value).catch((e) => get().toastError(e));
  },
  toast: (kind, text, error) => {
    const id = toastSeq++;
    set({ toasts: [...get().toasts, { id, kind, text, error }] });
    setTimeout(() => get().dismissToast(id), kind === "error" ? 9000 : 4000);
  },
  toastError: (e) => {
    const err = toError(e);
    if (err.kind === "cancelled") return;
    console.error(err);
    get().toast("error", err.message, err);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  requestDelete: (deleteRequest) => set({ deleteRequest }),
  closeDelete: () => set({ deleteRequest: undefined }),
  requestRename: (path, onDone) => {
    const name = path.split("\\").pop() ?? path;
    set({ renameRequest: { path, name, onDone } });
  },
  closeRename: () => set({ renameRequest: undefined }),
  requestUninstall: (programId) => set({ uninstallRequest: { programId } }),
  closeUninstall: () => set({ uninstallRequest: undefined }),
  requestForced: (forcedRequest) => set({ forcedRequest }),
  closeForced: () => set({ forcedRequest: undefined }),
  requestBatch: (ids, quick) => set({ batchRequest: { ids, quick } }),
  closeBatch: () => set({ batchRequest: undefined }),
  targetOpen: false,
  openTarget: () => set({ targetOpen: true }),
  closeTarget: () => set({ targetOpen: false }),
}));
