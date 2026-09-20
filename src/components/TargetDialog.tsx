import { Crosshair, FolderSearch, Info, Search } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { t as tr, useT } from "../i18n";
import { api, toError } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import type { ErrorPayload, TargetPicked } from "../types";
import { ErrorView } from "./ErrorView";
import { Modal } from "./Modal";
import { ProcessPanel } from "./ProcessPanel";

/** Search for a file name in the analyzer, scanning its drive first if needed. */
function searchInAnalyzer(path: string) {
  const app = useApp.getState();
  const a = useAnalyzer.getState();
  const name = path.split("\\").pop() ?? path;
  const drive = path.slice(0, 3);
  const covered = a.meta && a.scanId !== undefined && path.toLowerCase().startsWith(a.meta.rootPath.toLowerCase().replace(/\\$/, ""));
  app.navigate("analyzer");
  if (covered) void a.runSearch(`"${name}"`);
  else {
    a.set({ pendingSearch: `"${name}"` });
    a.setTarget(drive);
    app.toast("info", tr("programs.mapScanning", { drive }));
    void a.startScan(drive, false);
  }
}

function analyzeFolder(path: string) {
  const folder = path.slice(0, path.lastIndexOf("\\")) || path;
  const a = useAnalyzer.getState();
  a.setTarget(folder);
  useApp.getState().navigate("analyzer");
  void a.startScan(folder, false);
}

export function TargetDialog() {
  const t = useT();
  const { targetOpen, closeTarget } = useApp();
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<TargetPicked | null>();
  const [error, setError] = useState<ErrorPayload>();
  const started = useRef(false);

  const pick = useCallback(async () => {
    setPicking(true);
    setError(undefined);
    try {
      setPicked(await api.targetPick(tr("target.hint")));
    } catch (e) {
      setError(toError(e));
    }
    setPicking(false);
  }, []);

  // Clicking "Target Mode" starts picking right away.
  useEffect(() => {
    if (targetOpen && !started.current) {
      started.current = true;
      setPicked(undefined);
      void pick();
    }
    if (!targetOpen) started.current = false;
  }, [targetOpen, pick]);

  if (!targetOpen) return null;
  const w = picked?.window;
  const go = (f: () => void) => {
    closeTarget();
    f();
  };

  return (
    <Modal
      title={t("target.title")}
      icon={<Crosshair size={18} color="var(--accent)" />}
      onClose={closeTarget}
      locked={picking}
      wide
      footer={
        <>
          <button className="btn" onClick={closeTarget} disabled={picking}>{t("common.close")}</button>
          <button className="btn primary" onClick={() => void pick()} disabled={picking}><Crosshair size={14} />{picked ? t("target.pickAgain") : t("target.start")}</button>
        </>
      }
    >
      {picking && <div className="banner info"><Crosshair size={16} /><span>{t("target.picking")}</span></div>}
      {!picking && picked === null && <div className="muted">{t("target.cancelled")}</div>}
      {!picking && picked === undefined && !error && <p style={{ margin: 0 }}>{t("target.intro")}</p>}
      {error && <ErrorView error={error} />}
      {!picking && w && (
        <>
          <div className="card" style={{ padding: "0.7rem 0.9rem" }}>
            <dl className="kv" style={{ margin: 0 }}>
              <dt>{t("target.windowTitle")}</dt><dd>{w.title || <span className="faint">—</span>}</dd>
              <dt>{t("target.className")}</dt><dd className="mono">{w.className}</dd>
              <dt>{t("target.process")}</dt><dd className="mono">{picked?.process?.name ?? "?"} (PID {w.pid})</dd>
            </dl>
          </div>
          {w.trayIcon ? (
            <div className="banner info"><Info size={16} /><span>{t("target.trayIcon", { tip: w.trayIcon.tooltip.split("\n")[0] || "—" })}</span></div>
          ) : w.shell === "desktop" ? (
            <div className="banner info"><Info size={16} /><span>{t("target.shell_desktop")}</span></div>
          ) : w.shell ? (
            <div className="banner info"><Info size={16} /><span>{w.trayReadable ? t("target.shell_taskbar") : t("target.trayUnsupported")}</span></div>
          ) : null}
          {w.hostPid != null && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("target.uwp", { pid: w.hostPid })}</div>}
          {picked?.process ? (
            <ProcessPanel
              key={w.pid}
              pid={w.pid}
              extra={(d) =>
                d.row.path ? (
                  <>
                    <button className="btn sm" onClick={() => go(() => searchInAnalyzer(d.row.path!))}><Search size={14} />{t("target.searchFile")}</button>
                    <button className="btn sm" onClick={() => go(() => analyzeFolder(d.row.path!))}><FolderSearch size={14} />{t("target.analyzeSpace")}</button>
                  </>
                ) : null
              }
            />
          ) : (
            <div className="muted">{t("target.processGone")}</div>
          )}
        </>
      )}
    </Modal>
  );
}
