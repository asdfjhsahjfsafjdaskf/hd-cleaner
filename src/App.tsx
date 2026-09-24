import { AlertTriangle } from "lucide-react";
import { useEffect, useState } from "react";
import { ContextMenuHost } from "./components/ContextMenu";
import { DeleteDialog } from "./components/DeleteDialog";
import { Modal } from "./components/Modal";
import { RenameDialog } from "./components/RenameDialog";
import { NOT_IMPLEMENTED_PAGES, Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { UninstallWizard } from "./components/UninstallWizard";
import { ForcedUninstallDialog } from "./components/ForcedUninstallDialog";
import { BatchUninstallDialog } from "./components/BatchUninstallDialog";
import { useT } from "./i18n";
import { Changes } from "./pages/Changes";
import { Dashboard } from "./pages/Dashboard";
import { DiskAnalyzer } from "./pages/DiskAnalyzer";
import { Duplicates } from "./pages/Duplicates";
import { History } from "./pages/History";
import { LargeFiles } from "./pages/LargeFiles";
import { NotImplemented } from "./pages/NotImplemented";
import { Cleaner } from "./pages/Cleaner";
import { Backups } from "./pages/Backups";
import { WindowsApps } from "./pages/WindowsApps";
import { Extensions } from "./pages/Extensions";
import { Monitor } from "./pages/Monitor";
import { Processes } from "./pages/Processes";
import { Programs } from "./pages/Programs";
import { Startup } from "./pages/Startup";
import { TargetDialog } from "./components/TargetDialog";
import { Settings } from "./pages/Settings";
import { Tools } from "./pages/Tools";
import { api } from "./services/api";
import { useApp } from "./stores/app";
import type { OperationRecord } from "./types";
import { formatDate } from "./utils/format";

function InterruptedDialog() {
  const t = useT();
  const { info, requestDelete, toastError } = useApp();
  const [queue, setQueue] = useState<OperationRecord[]>([]);
  const [inspect, setInspect] = useState(false);
  useEffect(() => setQueue(info?.interrupted ?? []), [info]);
  const op = queue[0];
  if (!op) return null;
  const next = () => {
    setInspect(false);
    setQueue((q) => q.slice(1));
  };
  const resolve = async (action: "resume" | "discard") => {
    try {
      const paths = await api.resolveInterrupted(op.id, action);
      next();
      if (action === "resume" && paths.length) {
        // A brand-new plan: every item is re-checked before anything happens.
        requestDelete({ targets: paths.map((path) => ({ path })), mode: op.details?.mode === "permanent" ? "permanent" : "recycleBin" });
      }
    } catch (e) {
      toastError(e);
    }
  };
  return (
    <Modal
      title={t("interrupted.title")}
      icon={<AlertTriangle size={18} color="var(--warning)" />}
      onClose={() => resolve("discard")}
      footer={
        <>
          <button className="btn" onClick={() => setInspect((v) => !v)}>{t("interrupted.inspect")}</button>
          <button className="btn" onClick={() => resolve("discard")}>{t("interrupted.discard")}</button>
          <button className="btn primary" onClick={() => resolve("resume")} disabled={!op.details?.paths?.length}>{t("interrupted.resume")}</button>
        </>
      }
    >
      <p style={{ margin: 0 }}>{t("interrupted.body")}</p>
      <div className="kv">
        <dt>{t("common.type")}</dt><dd>{op.kind}</dd>
        <dt>{t("common.date")}</dt><dd>{formatDate(op.startedMs)}</dd>
        <dt>{t("common.details")}</dt><dd>{op.summary}</dd>
      </div>
      {inspect && (
        <pre className="mono selectable" style={{ maxHeight: "16rem", overflow: "auto", background: "var(--bg)", padding: "0.6rem", borderRadius: 6, fontSize: "0.8rem" }}>
          {JSON.stringify(op.details, null, 2)}
        </pre>
      )}
    </Modal>
  );
}

export default function App() {
  const { page, init, settingsLoaded } = useApp();
  useEffect(() => {
    init();
  }, [init]);

  if (!settingsLoaded) return null;

  let content;
  if (NOT_IMPLEMENTED_PAGES.has(page)) content = <NotImplemented page={page} />;
  else
    switch (page) {
      case "dashboard": content = <Dashboard />; break;
      case "analyzer": content = <DiskAnalyzer />; break;
      case "largeFiles": content = <LargeFiles />; break;
      case "duplicates": content = <Duplicates />; break;
      case "changes": content = <Changes />; break;
      case "cleaner": content = <Cleaner />; break;
      case "programs": content = <Programs />; break;
      case "startup": content = <Startup />; break;
      case "processes": content = <Processes />; break;
      case "monitor": content = <Monitor />; break;
      case "backups": content = <Backups />; break;
      case "windowsApps": content = <WindowsApps />; break;
      case "extensions": content = <Extensions />; break;
      case "history": content = <History />; break;
      case "tools": content = <Tools />; break;
      case "settings": content = <Settings />; break;
      default: content = <NotImplemented page={page} />;
    }

  return (
    <div className="app">
      <Sidebar />
      <main className="main">{content}</main>
      <DeleteDialog />
      <RenameDialog />
      <UninstallWizard />
      <ForcedUninstallDialog />
      <BatchUninstallDialog />
      <TargetDialog />
      <InterruptedDialog />
      <ContextMenuHost />
      <Toasts />
    </div>
  );
}
