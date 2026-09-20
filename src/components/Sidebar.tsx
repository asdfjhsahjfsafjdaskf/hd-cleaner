import {
  AppWindow, Archive, BarChart3, Boxes, Copy, Cpu, Crosshair, FileStack, GitCompareArrows, HardDrive, History, LayoutDashboard, Power,
  Radar, Settings, ShieldCheck, Sparkles, Wrench,
} from "lucide-react";
import type { ReactNode } from "react";
import { APP_NAME } from "../config/branding";
import { useT } from "../i18n";
import { useApp, type PageId } from "../stores/app";

interface Item {
  id: PageId;
  icon: ReactNode;
  notImplemented?: boolean;
}

const GROUPS: { title?: string; items: Item[]; action?: boolean }[] = [
  { items: [{ id: "dashboard", icon: <LayoutDashboard size={17} /> }] },
  {
    title: "nav.groupStorage",
    items: [
      { id: "analyzer", icon: <HardDrive size={17} /> },
      { id: "largeFiles", icon: <FileStack size={17} /> },
      { id: "duplicates", icon: <Copy size={17} /> },
      { id: "changes", icon: <GitCompareArrows size={17} /> },
      { id: "cleaner", icon: <Sparkles size={17} /> },
    ],
  },
  {
    title: "nav.groupApps",
    items: [
      { id: "programs", icon: <Boxes size={17} /> },
      { id: "windowsApps", icon: <AppWindow size={17} />, notImplemented: true },
      { id: "startup", icon: <Power size={17} /> },
      { id: "processes", icon: <Cpu size={17} /> },
      { id: "monitor", icon: <Radar size={17} /> },
    ],
    action: true,
  },
  {
    title: "nav.groupSystem",
    items: [
      { id: "backups", icon: <Archive size={17} />, notImplemented: true },
      { id: "history", icon: <History size={17} /> },
      { id: "tools", icon: <Wrench size={17} /> },
      { id: "settings", icon: <Settings size={17} /> },
    ],
  },
];

export const NOT_IMPLEMENTED_PAGES = new Set<PageId>(
  GROUPS.flatMap((g) => g.items.filter((i) => i.notImplemented).map((i) => i.id)),
);

export function Sidebar() {
  const t = useT();
  const { page, navigate, info, openTarget } = useApp();
  return (
    <nav className="sidebar" aria-label="Main">
      <div className="brand">
        <img className="brand-mark" src="/logo.svg" alt="" />
        <span>{APP_NAME}</span>
      </div>
      {GROUPS.map((g, gi) => (
        <div key={gi} className="nav-group">
          {g.title && <div className="nav-group-title">{t(g.title)}</div>}
          {g.items.map((it) => (
            <button
              key={it.id}
              className={`nav-item ${page === it.id ? "active" : ""}`}
              onClick={() => navigate(it.id)}
              aria-current={page === it.id ? "page" : undefined}
            >
              {it.icon}
              <span>{t(`nav.${it.id}`)}</span>
              {it.notImplemented && <span className="badge-ni" title={t("common.notImplemented")}>—</span>}
            </button>
          ))}
          {g.action && (
            <button className="nav-item" onClick={openTarget} title={t("target.intro")}>
              <Crosshair size={17} />
              <span>{t("nav.target")}</span>
            </button>
          )}
        </div>
      ))}
      <div className="sidebar-footer">
        {info && (
          <span className="row" title={info.os.elevated ? t("common.elevated") : t("common.standardUser")}>
            {info.os.elevated ? <ShieldCheck size={14} color="var(--warning)" /> : <BarChart3 size={14} />}
            {info.os.elevated ? t("common.elevated") : t("common.standardUser")}
          </span>
        )}
        {info && <span>v{info.version}</span>}
      </div>
    </nav>
  );
}
