// Shared pieces of the process manager, the startup manager and Target Mode.
import { AlertTriangle, ShieldAlert, Square } from "lucide-react";
import { useState } from "react";
import { t as tr, useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { ErrorPayload, ProcessRow, StartupItem, TreeKill } from "../types";
import { ErrorView } from "./ErrorView";
import { Modal } from "./Modal";

/** Why a process cannot be ended from the UI, if it cannot. */
export function endBlocked(row: ProcessRow): "windowsProcess" | "selfProcess" | null {
  if (row.isSelf) return "selfProcess";
  if (row.isWindows) return "windowsProcess";
  return null;
}

/** Confirmation + execution of "end process" / "end process tree". */
export function EndProcessDialog({ row, tree, children, onClose, onDone }: {
  row: ProcessRow;
  tree: boolean;
  children: number;
  onClose: () => void;
  onDone: () => void;
}) {
  const t = useT();
  const toast = useApp((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const [failed, setFailed] = useState<TreeKill[]>([]);

  const run = async (elevated: boolean) => {
    setBusy(true);
    setError(undefined);
    try {
      const r = await api.processEnd(row.pid, row.path, tree, elevated);
      const ok = r.filter((k) => k.ok).length;
      const kept = r.filter((k) => k.skipped).length;
      const bad = r.filter((k) => !k.ok && !k.skipped);
      let msg = tree ? t("processes.treeResult", { ok, total: r.length - kept }) : t("processes.ended", { name: row.name });
      if (kept) msg += ` · ${t("processes.keptWindows", { n: kept })}`;
      toast(bad.length ? "info" : "success", msg);
      onDone();
      if (bad.length) setFailed(bad);
      else onClose();
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  };

  // Without a readable path only the elevated helper can identify and end it.
  const denied = error?.kind === "accessDenied" || !row.path;
  return (
    <Modal
      title={tree ? t("processes.endTree") : t("processes.end")}
      icon={<Square size={17} color="var(--danger)" />}
      onClose={onClose}
      locked={busy}
      footer={
        failed.length ? (
          <button className="btn primary" onClick={onClose}>{t("common.close")}</button>
        ) : (
          <>
            <button className="btn" onClick={onClose} disabled={busy}>{t("common.cancel")}</button>
            {denied ? (
              <button className="btn danger" disabled={busy} onClick={() => run(true)}><ShieldAlert size={14} />{t("processes.endElevated")}</button>
            ) : (
              <button className="btn danger" disabled={busy} onClick={() => run(false)}><Square size={14} />{tree ? t("processes.endTree") : t("processes.end")}</button>
            )}
          </>
        )
      }
    >
      {failed.length > 0 ? (
        <>
          <div>{t("processes.notAllEnded")}</div>
          <div className="col" style={{ gap: "0.3rem" }}>
            {failed.map((k) => (
              <div key={k.pid} className="mono" style={{ fontSize: "0.82rem" }}>
                {k.name} (PID {k.pid}) — {k.error?.message}
              </div>
            ))}
          </div>
        </>
      ) : (
        <>
          <p style={{ margin: 0 }}>
            {tree ? t("processes.confirmEndTree", { name: row.name, n: children }) : t("processes.confirmEnd", { name: row.name, pid: row.pid })}
          </p>
          {row.path && <div className="mono faint selectable" style={{ fontSize: "0.82rem", wordBreak: "break-all" }}>{row.path}</div>}
          <div className="banner warning"><AlertTriangle size={16} color="var(--warning)" /><span>{t("processes.endWarning")}</span></div>
          {denied && <div className="muted">{row.path ? t("processes.accessDeniedHint") : t("processes.noPathHint")}</div>}
          {error && <ErrorView error={error} />}
        </>
      )}
    </Modal>
  );
}

export function startupSourceLabel(i: StartupItem, t: (k: string, v?: Record<string, string | number>) => string) {
  switch (i.source.kind) {
    case "runKey":
      return i.source.once ? t("startup.source_runOnce") : i.source.hive === "currentUser" ? t("startup.source_runUser") : t("startup.source_runMachine");
    case "startupFolder":
      return i.source.common ? t("startup.source_folderCommon") : t("startup.source_folderUser");
    case "task":
      return t("startup.source_task");
    case "service":
      return t("startup.source_service");
  }
}

export function startupDetailLabel(i: StartupItem, t: (k: string) => string) {
  if (!i.detail || i.detail === "once") return undefined;
  if (i.source.kind === "task") return t(`startup.trigger_${i.detail}`);
  if (i.source.kind === "service") return t(`startup.mode_${i.detail}`);
  return undefined;
}

/** Enable/disable one startup entry (UAC when it needs it). */
export async function toggleStartup(i: StartupItem, enabled: boolean): Promise<boolean> {
  const app = useApp.getState();
  try {
    await api.startupSetEnabled(i.id, i.command, enabled);
    app.toast("success", enabled ? tr("startup.toggledOn", { name: i.name }) : tr("startup.toggledOff", { name: i.name }));
    return true;
  } catch (e) {
    app.toastError(e);
    return false;
  }
}
