import { AppWindow, RefreshCw, ShieldAlert, Trash2, Wrench } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { Modal } from "../components/Modal";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { ErrorPayload, WindowsApp } from "../types";
import { formatBytes, formatNumber } from "../utils/format";

export function WindowsApps() {
  const t = useT();
  const app = useApp();
  const [apps, setApps] = useState<WindowsApp[]>();
  const [error, setError] = useState<ErrorPayload>();
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [showSystem, setShowSystem] = useState(false);
  const [selected, setSelected] = useState<WindowsApp>();
  const [removing, setRemoving] = useState<{ app: WindowsApp; allUsers: boolean }>();

  const load = async (measure = false) => {
    setBusy(true);
    try {
      setApps(await api.windowsAppsList(measure));
      setError(undefined);
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  };
  useEffect(() => {
    void load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (apps ?? []).filter((a) => {
      if (!showSystem && (a.critical || a.isFramework)) return false;
      return !q || a.name.toLowerCase().includes(q) || (a.publisher ?? "").toLowerCase().includes(q);
    });
  }, [apps, query, showSystem]);

  const repair = async (a: WindowsApp) => {
    setBusy(true);
    try {
      await api.windowsAppRepair(a.packageFullName!);
      app.toast("success", t("windowsApps.repaired", { name: a.name }));
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const remove = async () => {
    if (!removing) return;
    const { app: target, allUsers } = removing;
    setRemoving(undefined);
    setBusy(true);
    try {
      await api.windowsAppRemove(target.packageFullName!, allUsers);
      app.toast("success", t("windowsApps.removed", { name: target.name }));
      if (selected?.packageFullName === target.packageFullName) setSelected(undefined);
      await load();
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  return (
    <div className="page fill">
      <div className="toolbar">
        <AppWindow size={16} className="muted" />
        <strong>{t("nav.windowsApps")}</strong>
        <input
          className="input"
          style={{ width: "16rem" }}
          placeholder={t("common.search")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label={t("common.search")}
        />
        <label className="row" style={{ gap: "0.4rem", fontSize: "0.88rem" }}>
          <input type="checkbox" checked={showSystem} onChange={(e) => setShowSystem(e.target.checked)} style={{ accentColor: "var(--accent)" }} />
          <span className="muted">{t("windowsApps.showSystem")}</span>
        </label>
        <span className="grow" />
        {apps && <span className="faint">{t("windowsApps.count", { n: formatNumber(shown.length) })}</span>}
        <button className="btn sm" disabled={busy} onClick={() => void load(true)}><RefreshCw size={14} />{t("windowsApps.measure")}</button>
      </div>

      <div className="page-body">
        {error && <ErrorView error={error} onRetry={() => void load()} />}
        <div className="muted" style={{ marginBottom: "0.8rem" }}>{t("windowsApps.intro")}</div>
        {!apps && busy && <div className="empty">{t("common.loading")}</div>}
        <div className="col" style={{ gap: "0.35rem" }}>
          {shown.map((a) => (
            <div key={a.packageFullName} className="card" style={{ padding: "0.55rem 0.75rem" }}>
              <div className="row" style={{ gap: "0.5rem" }}>
                <span className="grow ellipsis"><strong>{a.name}</strong></span>
                {a.critical && <span className="badge blocked">{t("windowsApps.system")}</span>}
                {a.isFramework && <span className="badge neutral">{t("programs.badge_framework")}</span>}
                <span className="faint" style={{ fontSize: "0.82rem" }}>{a.version}</span>
                {a.bytes != null && <strong style={{ minWidth: "5rem", textAlign: "right" }}>{formatBytes(a.bytes)}</strong>}
                <button className="btn sm" onClick={() => setSelected(selected?.packageFullName === a.packageFullName ? undefined : a)}>
                  {t("windowsApps.details")}
                </button>
                <button className="btn sm" disabled={busy} onClick={() => void repair(a)} title={t("windowsApps.repairHint")}>
                  <Wrench size={13} />{t("windowsApps.repair")}
                </button>
                <button
                  className="btn sm danger"
                  disabled={busy || a.critical}
                  title={a.critical ? t("windowsApps.systemHint") : undefined}
                  onClick={() => setRemoving({ app: a, allUsers: false })}
                >
                  <Trash2 size={13} />{t("programs.uninstall")}
                </button>
              </div>
              <div className="faint" style={{ fontSize: "0.8rem" }}>{a.publisher}</div>
              {selected?.packageFullName === a.packageFullName && (
                <div className="col" style={{ gap: "0.2rem", marginTop: "0.4rem", fontSize: "0.82rem" }}>
                  <span className="mono selectable" style={{ wordBreak: "break-all" }}>{a.packageFullName}</span>
                  {a.installLocation && <span className="mono faint" style={{ wordBreak: "break-all" }}>{a.installLocation}</span>}
                  {a.dependencies.length > 0 && (
                    <span className="faint">{t("windowsApps.dependencies")}: {a.dependencies.join(", ")}</span>
                  )}
                </div>
              )}
            </div>
          ))}
          {apps && shown.length === 0 && <div className="empty">{t("windowsApps.none")}</div>}
        </div>
      </div>

      {removing && (
        <Modal
          title={t("windowsApps.removeTitle", { name: removing.app.name })}
          icon={<ShieldAlert size={17} color="var(--danger)" />}
          onClose={() => setRemoving(undefined)}
          footer={
            <>
              <button className="btn" onClick={() => setRemoving(undefined)}>{t("common.cancel")}</button>
              <button className="btn danger" onClick={() => void remove()}><Trash2 size={14} />{t("programs.uninstall")}</button>
            </>
          }
        >
          <p style={{ margin: 0 }}>{t("windowsApps.removeBody")}</p>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={removing.allUsers}
              onChange={(e) => setRemoving({ ...removing, allUsers: e.target.checked })}
            />
            <span>{t("windowsApps.allUsers")}</span>
          </label>
          <div className="mono faint" style={{ fontSize: "0.8rem", wordBreak: "break-all" }}>{removing.app.packageFullName}</div>
        </Modal>
      )}
    </div>
  );
}
