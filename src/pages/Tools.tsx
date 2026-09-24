import {
  Activity, Cpu, Eraser, FileText, Gauge, HardDrive, Keyboard, LayoutPanelLeft, ListChecks, MonitorCog, Network, Settings2, ShieldAlert, SquareTerminal, Terminal, Variable,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useT } from "../i18n";
import { api } from "../services/api";
import { useApp } from "../stores/app";
import { Modal } from "../components/Modal";
import type { DriveInfo, ToolInfo, WipePlan } from "../types";
import { formatBytes } from "../utils/format";

const ICONS: Record<string, ReactNode> = {
  taskManager: <Activity size={20} />,
  resourceMonitor: <Gauge size={20} />,
  systemInformation: <Cpu size={20} />,
  eventViewer: <FileText size={20} />,
  services: <ListChecks size={20} />,
  deviceManager: <MonitorCog size={20} />,
  diskManagement: <HardDrive size={20} />,
  registryEditor: <Network size={20} />,
  controlPanel: <LayoutPanelLeft size={20} />,
  windowsSettings: <Settings2 size={20} />,
  systemProperties: <Keyboard size={20} />,
  environmentVariables: <Variable size={20} />,
  powershell: <SquareTerminal size={20} />,
  commandPrompt: <Terminal size={20} />,
};

/** Filling the free space of a drive so deleted files stop being readable. */
function FreeSpaceWipe() {
  const t = useT();
  const app = useApp();
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [drive, setDrive] = useState<string>();
  const [plan, setPlan] = useState<WipePlan>();
  const [confirming, setConfirming] = useState(false);
  const [progress, setProgress] = useState<{ written: number; total: number }>();

  useEffect(() => {
    api.listDrives().then((d) => {
      const fixed = d.filter((x) => x.kind === "fixed" && x.ready);
      setDrives(fixed);
      setDrive((cur) => cur ?? fixed[0]?.letter);
    }).catch(app.toastError);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    setPlan(undefined);
    if (!drive) return;
    api.wipePlan(drive).then(setPlan).catch(() => {});
  }, [drive]);

  const run = async () => {
    if (!drive) return;
    setConfirming(false);
    setProgress({ written: 0, total: plan?.toWrite ?? 0 });
    try {
      const r = await api.wipeFreeSpace(drive, undefined, (e) => setProgress(e.data));
      app.toast("success", t("tools.wipeDone", { size: formatBytes(r.written) }));
      api.wipePlan(drive).then(setPlan).catch(() => {});
    } catch (e) {
      app.toastError(e);
    }
    setProgress(undefined);
  };

  return (
    <>
      <div className="section-title">{t("tools.wipeTitle")}</div>
      <div className="card">
        <div className="muted" style={{ marginBottom: "0.6rem" }}>{t("tools.wipeIntro")}</div>
        <div className="row wrap" style={{ gap: "0.6rem" }}>
          <select className="select" value={drive ?? ""} onChange={(e) => setDrive(e.target.value)} aria-label={t("tools.wipeDrive")}>
            {drives.map((d) => (
              <option key={d.letter} value={d.letter}>
                {d.letter}: {d.label} — {formatBytes(d.freeBytes)} {t("tools.wipeFree")}
              </option>
            ))}
          </select>
          {plan && <span className="faint">{t("tools.wipeWouldWrite", { size: formatBytes(plan.toWrite), reserve: formatBytes(plan.reserve) })}</span>}
          <span className="grow" />
          <button className="btn danger" disabled={!plan || !!progress || plan.toWrite === 0} onClick={() => setConfirming(true)}>
            <Eraser size={14} />{t("tools.wipeStart")}
          </button>
        </div>
        {plan?.isSsd && (
          <div className="banner warning" style={{ marginTop: "0.6rem" }}>
            <ShieldAlert size={16} color="var(--warning)" />
            <span>{t("tools.wipeSsd")}</span>
          </div>
        )}
        {progress && (
          <div style={{ marginTop: "0.6rem" }}>
            <div className="progress-track">
              <div style={{ width: `${(progress.written / Math.max(1, progress.total)) * 100}%`, height: "100%", background: "var(--accent)" }} />
            </div>
            <span className="faint">{formatBytes(progress.written)} / {formatBytes(progress.total)}</span>
          </div>
        )}
      </div>

      {confirming && plan && (
        <Modal
          title={t("tools.wipeTitle")}
          icon={<Eraser size={17} color="var(--danger)" />}
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirming(false)}>{t("common.cancel")}</button>
              <button className="btn danger" onClick={() => void run()}><Eraser size={14} />{t("tools.wipeStart")}</button>
            </>
          }
        >
          <p style={{ margin: 0 }}>{t("tools.wipeConfirm", { size: formatBytes(plan.toWrite), drive: drive ?? "", reserve: formatBytes(plan.reserve) })}</p>
          <div className="banner warning">{t("tools.wipeLimits")}</div>
          {plan.isSsd && <div className="banner warning">{t("tools.wipeSsd")}</div>}
        </Modal>
      )}
    </>
  );
}

export function Tools() {
  const t = useT();
  const app = useApp();
  const [tools, setTools] = useState<ToolInfo[]>([]);
  useEffect(() => {
    api.listTools().then(setTools).catch(app.toastError);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const groups = [...new Set(tools.map((x) => x.group))];
  return (
    <div className="page">
      <div className="page-header"><h1>{t("tools.title")}</h1></div>
      <p className="muted" style={{ marginTop: 0 }}>{t("tools.intro")}</p>
      {groups.map((g) => (
        <div key={g}>
          <div className="section-title">{t(`tools.group_${g}`)}</div>
          <div className="grid cols-auto">
            {tools.filter((x) => x.group === g).map((x) => (
              <button key={x.id} className="tool-tile" onClick={() => api.launchTool(x.id).catch(app.toastError)}>
                <span className="muted">{ICONS[x.id]}</span>
                <span>{t(`tools.${x.id}`)}</span>
              </button>
            ))}
          </div>
        </div>
      ))}
      <FreeSpaceWipe />
    </div>
  );
}
