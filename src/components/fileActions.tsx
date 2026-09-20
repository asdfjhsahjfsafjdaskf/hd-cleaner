import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  Boxes, ClipboardCopy, Copy, ExternalLink, FolderInput, FolderOpen, FolderSearch, Info, LayoutGrid, Pencil, SquareTerminal, Terminal, Trash2, XOctagon,
} from "lucide-react";
import { api } from "../services/api";
import { t } from "../i18n";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { MenuItem } from "./ContextMenu";

export interface FileTarget {
  scanId?: number;
  node?: number;
  path: string;
  isDir: boolean;
}

async function run(p: Promise<unknown>) {
  try {
    await p;
  } catch (e) {
    useApp.getState().toastError(e);
  }
}

export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    useApp.getState().toast("info", t("common.copied"));
  } catch (e) {
    useApp.getState().toastError(e);
  }
}

export function requestDelete(targets: FileTarget[], permanent: boolean, onDone?: () => void) {
  if (!targets.length) return;
  const scanId = targets[0].scanId;
  const useRecycle = useApp.getState().settings.useRecycleBin;
  useApp.getState().requestDelete({
    scanId,
    targets: targets.map((x) => (x.node !== undefined && x.scanId !== undefined ? { node: x.node } : { path: x.path })),
    mode: permanent || !useRecycle ? "permanent" : "recycleBin",
    onDone,
  });
}

async function transfer(target: FileTarget, kind: "copy" | "move", onDone?: () => void) {
  const dest = await openDialog({ directory: true, multiple: false });
  if (typeof dest !== "string") return;
  try {
    const res = await api.transfer([target.path], dest, kind);
    const failed = res.find((r) => r.status === "failed");
    if (failed && failed.status === "failed") useApp.getState().toastError(failed.error);
    else if (kind === "move") onDone?.();
  } catch (e) {
    useApp.getState().toastError(e);
  }
}

export interface ActionHooks {
  onAnalyzeFolder?: (path: string) => void;
  onShowInTreemap?: () => void;
  onChanged?: () => void;
}

/** Standard context menu for a file or folder. */
export function fileMenu(target: FileTarget, hooks: ActionHooks = {}): MenuItem[] {
  const folder = target.isDir ? target.path : target.path.slice(0, target.path.lastIndexOf("\\")) || target.path;
  const items: MenuItem[] = [
    { label: t("common.open"), icon: <ExternalLink size={15} />, onClick: () => run(api.openPath(target.path)), hint: "Enter" },
    { label: t("common.openFolder"), icon: <FolderOpen size={15} />, onClick: () => run(api.revealPath(target.path)) },
    { label: t("common.copyPath"), icon: <ClipboardCopy size={15} />, onClick: () => copyText(target.path), hint: "Ctrl+C" },
    { label: t("common.properties"), icon: <Info size={15} />, onClick: () => run(api.showProperties(target.path)) },
    "sep",
  ];
  if (target.isDir && hooks.onAnalyzeFolder) {
    items.push({ label: t("details.analyzeHere"), icon: <FolderSearch size={15} />, onClick: () => hooks.onAnalyzeFolder!(target.path) });
  }
  items.push({
    label: t("programs.identifyTitle"),
    icon: <Boxes size={15} />,
    onClick: async () => {
      try {
        const found = await api.identifyProgram(target.path);
        if (!found.length) {
          useApp.getState().toast("info", t("programs.identifyNone"));
          return;
        }
        usePrograms.getState().select(found[0].programId);
        usePrograms.getState().set({ query: "", showHidden: true });
        useApp.getState().navigate("programs");
      } catch (e) {
        useApp.getState().toastError(e);
      }
    },
  });
  if (hooks.onShowInTreemap) {
    items.push({ label: t("details.showInTreemap"), icon: <LayoutGrid size={15} />, onClick: hooks.onShowInTreemap });
  }
  items.push(
    { label: t("details.openTerminal"), icon: <Terminal size={15} />, onClick: () => run(api.openTerminal(folder, "cmd")) },
    { label: t("details.openPowerShell"), icon: <SquareTerminal size={15} />, onClick: () => run(api.openTerminal(folder, "powerShell")) },
    "sep",
    {
      label: t("details.rename"),
      icon: <Pencil size={15} />,
      hint: "F2",
      onClick: () => useApp.getState().requestRename(target.path, () => hooks.onChanged?.()),
    },
    { label: t("details.copyTo"), icon: <Copy size={15} />, onClick: () => transfer(target, "copy") },
    { label: t("details.moveTo"), icon: <FolderInput size={15} />, onClick: () => transfer(target, "move", hooks.onChanged) },
    "sep",
    { label: t("details.moveToRecycle"), icon: <Trash2 size={15} />, hint: "Del", onClick: () => requestDelete([target], false, hooks.onChanged) },
    { label: t("details.deletePermanently"), icon: <XOctagon size={15} />, hint: "Shift+Del", danger: true, onClick: () => requestDelete([target], true, hooks.onChanged) },
  );
  return items;
}
