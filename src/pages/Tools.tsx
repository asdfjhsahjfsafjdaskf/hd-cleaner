import {
  Activity, Cpu, FileText, Gauge, HardDrive, Keyboard, LayoutPanelLeft, ListChecks, MonitorCog, Network, Settings2, SquareTerminal, Terminal, Variable,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useT } from "../i18n";
import { api } from "../services/api";
import { useApp } from "../stores/app";
import type { ToolInfo } from "../types";

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
    </div>
  );
}
