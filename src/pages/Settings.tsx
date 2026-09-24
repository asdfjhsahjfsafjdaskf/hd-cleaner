import { useEffect, useState, type ReactNode } from "react";
import { api } from "../services/api";
import { Switch } from "../components/ui";
import { useT } from "../i18n";
import { useApp } from "../stores/app";

function Row({ label, desc, children, disabled }: { label: string; desc?: string; children: ReactNode; disabled?: boolean }) {
  const t = useT();
  return (
    <div className="settings-row" style={disabled ? { opacity: 0.6 } : undefined}>
      <div className="col" style={{ gap: "0.15rem" }}>
        <span>{label}</span>
        {desc && <span className="desc">{desc}</span>}
        {disabled && <span className="desc">{t("common.notImplemented")}</span>}
      </div>
      {children}
    </div>
  );
}

export function Settings() {
  const t = useT();
  const app = useApp();
  const { settings: s, setSetting, info } = app;
  // Read from the registry, not from the app's own settings: the entry can be
  // changed from outside (the Startup page, Task Manager, Windows itself).
  const [autoStart, setAutoStart] = useState<boolean>();
  const [explorerMenu, setExplorerMenu] = useState<boolean>();
  useEffect(() => {
    api.startWithWindows().then(setAutoStart).catch(() => setAutoStart(false));
    api.explorerMenu().then(setExplorerMenu).catch(app.toastError);
  }, []);

  const toggleAutoStart = async (v: boolean) => {
    setAutoStart(v);
    try {
      await api.setStartWithWindows(v);
    } catch (e) {
      setAutoStart(!v);
      app.toastError(e);
    }
  };

  const toggleExplorerMenu = async (enabled: boolean) => {
    setExplorerMenu(enabled);
    try {
      await api.setExplorerMenu(enabled, s.language === "pt-BR"
        ? "Analisar com HD Cleaner" : "Analyze with HD Cleaner");
    } catch (e) {
      setExplorerMenu(!enabled);
      app.toastError(e);
    }
  };

  return (
    <div className="page" style={{ maxWidth: "58rem" }}>
      <div className="page-header"><h1>{t("settings.title")}</h1></div>

      <div className="section-title" style={{ marginTop: 0 }}>{t("settings.general")}</div>
      <div className="card">
        <Row label={t("settings.theme")}>
          <div className="seg">
            {(["dark", "light", "system"] as const).map((v) => (
              <button key={v} className={s.theme === v ? "on" : ""} onClick={() => setSetting("theme", v)}>
                {t(`settings.theme${v[0].toUpperCase()}${v.slice(1)}`)}
              </button>
            ))}
          </div>
        </Row>
        <Row label={t("settings.language")}>
          <select className="select" value={s.language} onChange={(e) => setSetting("language", e.target.value as typeof s.language)}>
            <option value="en">English</option>
            <option value="pt-BR">Português (Brasil)</option>
          </select>
        </Row>
        <Row label={t("settings.fontScale")}>
          <div className="seg">
            {[0.9, 1, 1.15, 1.3].map((v) => (
              <button key={v} className={s.fontScale === v ? "on" : ""} onClick={() => setSetting("fontScale", v)}>{Math.round(v * 100)}%</button>
            ))}
          </div>
        </Row>
        <Row label={t("settings.startWithWindows")} desc={t("settings.startWithWindowsDesc")}>
          <Switch checked={autoStart ?? false} disabled={autoStart === undefined} onChange={(v) => void toggleAutoStart(v)} label={t("settings.startWithWindows")} />
        </Row>
        <Row label={t("settings.checkUpdates")} disabled><Switch checked={false} disabled onChange={() => {}} label={t("settings.checkUpdates")} /></Row>
        <Row label={t("settings.explorerMenu").replace("…", "HD Cleaner")}>
          <Switch checked={explorerMenu ?? false} disabled={explorerMenu === undefined} onChange={(v) => void toggleExplorerMenu(v)} label={t("settings.explorerMenu")} />
        </Row>
      </div>

      <div className="section-title">{t("settings.analyzer")}</div>
      <div className="card">
        <Row label={t("settings.preferFastScan")}><Switch checked={s.preferFastScan} onChange={(v) => setSetting("preferFastScan", v)} label={t("settings.preferFastScan")} /></Row>
        <Row label={t("settings.followJunctions")}><Switch checked={s.followJunctions} onChange={(v) => setSetting("followJunctions", v)} label={t("settings.followJunctions")} /></Row>
        <Row label={t("settings.saveSnapshots")}><Switch checked={s.saveSnapshots} onChange={(v) => setSetting("saveSnapshots", v)} label={t("settings.saveSnapshots")} /></Row>
        <Row label={t("settings.treemapMetric")}>
          <select className="select" value={s.treemapMetric} onChange={(e) => setSetting("treemapMetric", e.target.value as typeof s.treemapMetric)}>
            <option value="allocated">{t("settings.metricAllocated")}</option>
            <option value="logical">{t("settings.metricLogical")}</option>
          </select>
        </Row>
        <Row label={t("settings.treemapMaxRects")}>
          <select className="select" value={s.treemapMaxRects} onChange={(e) => setSetting("treemapMaxRects", Number(e.target.value))}>
            {[20000, 60000, 120000, 250000].map((n) => <option key={n} value={n}>{n.toLocaleString()}</option>)}
          </select>
        </Row>
      </div>

      <div className="section-title">{t("settings.safety")}</div>
      <div className="card">
        <Row label={t("settings.useRecycleBin")}><Switch checked={s.useRecycleBin} onChange={(v) => setSetting("useRecycleBin", v)} label={t("settings.useRecycleBin")} /></Row>
        <Row label={t("settings.dryRunDefault")}><Switch checked={s.dryRunDefault} onChange={(v) => setSetting("dryRunDefault", v)} label={t("settings.dryRunDefault")} /></Row>
      </div>

      <div className="section-title">{t("settings.uninstaller")}</div>
      <div className="card">
        <Row label={t("uninstall.level")}>
          <select className="select" value={s.defaultLeftoverLevel} onChange={(e) => setSetting("defaultLeftoverLevel", e.target.value as typeof s.defaultLeftoverLevel)}>
            {(["safe", "moderate", "advanced"] as const).map((l) => <option key={l} value={l}>{t(`uninstall.level_${l}`)}</option>)}
          </select>
        </Row>
        <Row label={t("uninstall.restorePoint")}><Switch checked={s.createRestorePoint} onChange={(v) => setSetting("createRestorePoint", v)} label={t("uninstall.restorePoint")} /></Row>
        <Row label={t("settings.registryBackup")} desc={t("settings.registryBackupDesc")}><Switch checked disabled onChange={() => {}} label={t("settings.registryBackup")} /></Row>
      </div>

      <div className="section-title">{t("settings.cleaner")}</div>
      <div className="card">
        <Row label={t("settings.cleaner")} disabled><span /></Row>
      </div>

      <div className="section-title">{t("settings.advanced")}</div>
      <div className="card">
        <Row label={t("settings.debugLogs")}><Switch checked={s.debugLogs} onChange={(v) => setSetting("debugLogs", v)} label={t("settings.debugLogs")} /></Row>
        <Row label={t("settings.dataLocation")}><span className="mono muted selectable">{info?.dataDir}</span></Row>
        <Row label={t("settings.version", { v: info?.version ?? "" })}>
          <span className="muted">{t("settings.running", { who: info?.os.elevated ? t("common.elevated") : t("common.standardUser") })} · {info?.os.arch} · build {info?.os.build}</span>
        </Row>
      </div>
      <p className="muted" style={{ marginTop: "1rem" }}>{t("settings.privacy")}</p>
    </div>
  );
}
