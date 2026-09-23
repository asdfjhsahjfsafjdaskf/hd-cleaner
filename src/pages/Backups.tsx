import { Archive, FolderOpen, RotateCcw, ShieldCheck, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { BackupEntry, BackupManifest, BackupSet, ErrorPayload, RestoreResult } from "../types";
import { formatBytes, formatDate, formatNumber } from "../utils/format";

const KIND_BADGE: Record<string, string> = {
  uninstall: "review",
  forced: "dangerous",
  cleanup: "safe",
  startup: "neutral",
};

/** Entries that hold something this app can put back. */
function canRestore(e: BackupEntry): boolean {
  return (e.kind === "registry" || ((e.kind === "file" || e.kind === "folder") && /^[a-zA-Z]:[\\/]/.test(e.path))) && !!e.stored;
}

export function Backups() {
  const t = useT();
  const app = useApp();
  const [sets, setSets] = useState<BackupSet[]>();
  const [error, setError] = useState<ErrorPayload>();
  const [open, setOpen] = useState<BackupSet>();
  const [manifest, setManifest] = useState<BackupManifest>();
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [results, setResults] = useState<RestoreResult[]>();
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      setSets(await api.backupsList());
      setError(undefined);
    } catch (e) {
      setError(toError(e));
    }
  };
  useEffect(() => {
    void load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const inspect = async (s: BackupSet) => {
    setOpen(s);
    setManifest(undefined);
    setResults(undefined);
    try {
      const m = await api.backupRead(s.id);
      setManifest(m);
      setChecked(new Set(m.entries.map((e, i) => (canRestore(e) ? i : -1)).filter((i) => i >= 0)));
    } catch (e) {
      app.toastError(e);
      setOpen(undefined);
    }
  };

  const restore = async () => {
    if (!open || !checked.size) return;
    setBusy(true);
    try {
      const r = await api.backupRestore(open.id, [...checked].sort((a, b) => a - b));
      setResults(r);
      const n = r.filter((x) => x.status === "restored" || x.status === "partial").length;
      app.toast(n ? "success" : "info", t("backups.restored", { n }));
      await load();
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const remove = async (s: BackupSet) => {
    setBusy(true);
    try {
      await api.backupDelete(s.id);
      if (open?.id === s.id) setOpen(undefined);
      await load();
      app.toast("success", t("backups.deleted", { name: s.label }));
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const makeRestorePoint = async () => {
    setBusy(true);
    try {
      await api.createRestorePoint(t("backups.restorePointName"));
      app.toast("success", t("backups.restorePointDone"));
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const toggle = (i: number) => {
    const n = new Set(checked);
    if (n.has(i)) n.delete(i);
    else n.add(i);
    setChecked(n);
  };

  const totalBytes = sets?.reduce((a, s) => a + s.bytes, 0) ?? 0;

  if (open) {
    const entries = manifest?.entries ?? [];
    const byIndex = new Map(results?.map((r) => [r.index, r]));
    return (
      <div className="page fill">
        <div className="toolbar">
          <button className="btn sm" onClick={() => setOpen(undefined)}>{t("common.back")}</button>
          <strong>{open.label}</strong>
          <span className={`badge ${KIND_BADGE[open.kind] ?? "neutral"}`}>{t(`backups.kind_${open.kind}`)}</span>
          <span className="grow" />
          <button className="btn sm" onClick={() => api.openPath(open.path).catch(app.toastError)}>
            <FolderOpen size={14} />{t("backups.openFolder")}
          </button>
          <button className="btn sm primary" disabled={busy || !checked.size} onClick={() => void restore()}>
            <RotateCcw size={14} />{t("backups.restoreSelected", { n: checked.size })}
          </button>
        </div>
        <div className="page-body">
          <div className="muted" style={{ marginBottom: "0.8rem" }}>{t("backups.restoreHint")}</div>
          {open.legacy && <div className="banner warning" style={{ marginBottom: "0.8rem" }}>{t("backups.legacyHint")}</div>}
          <div className="col" style={{ gap: "0.3rem" }}>
            {entries.map((e, i) => {
              const ok = canRestore(e);
              const r = byIndex.get(i);
              return (
                <div key={i} className="card" style={{ padding: "0.5rem 0.7rem", opacity: ok ? 1 : 0.6 }}>
                  <div className="row" style={{ gap: "0.5rem" }}>
                    <input
                      type="checkbox"
                      checked={checked.has(i)}
                      disabled={!ok}
                      onChange={() => toggle(i)}
                      aria-label={e.path}
                      style={{ accentColor: "var(--accent)" }}
                    />
                    <span className="badge neutral">{t(`backups.entry_${e.kind}`)}</span>
                    <span className="grow ellipsis mono" style={{ fontSize: "0.85rem" }} title={e.path}>{e.path}</span>
                    {!!e.size && <span className="faint">{formatBytes(e.size)}</span>}
                    {r && (
                      <span className={r.status === "restored" || r.status === "partial" ? "" : "faint"} style={{ color: r.status === "failed" ? "var(--danger)" : undefined }}>
                        {t(`backups.result_${r.status}`)}
                        {r.values != null && ` (${formatNumber(r.values)})`}
                      </span>
                    )}
                  </div>
                  {!ok && <div className="faint" style={{ fontSize: "0.8rem", paddingLeft: "1.4rem" }}>{t("backups.cannotRestore")}</div>}
                  {r?.error && <div style={{ color: "var(--danger)", fontSize: "0.8rem", paddingLeft: "1.4rem" }}>{r.error.message}</div>}
                </div>
              );
            })}
            {!entries.length && <div className="empty">{t("backups.emptySet")}</div>}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="page fill">
      <div className="toolbar">
        <Archive size={16} className="muted" />
        <strong>{t("backups.title")}</strong>
        <span className="grow" />
        {!!sets?.length && <span className="faint">{t("backups.total", { n: sets.length, size: formatBytes(totalBytes) })}</span>}
        <button className="btn sm" disabled={busy} onClick={() => void makeRestorePoint()}>
          <ShieldCheck size={14} />{t("backups.newRestorePoint")}
        </button>
      </div>
      <div className="page-body">
        {error && <ErrorView error={error} onRetry={() => void load()} />}
        <div className="muted" style={{ marginBottom: "0.9rem" }}>{t("backups.intro")}</div>
        <div className="col" style={{ gap: "0.35rem" }}>
          {sets?.map((s) => (
            <div key={s.id} className="card" style={{ padding: "0.6rem 0.8rem" }}>
              <div className="row" style={{ gap: "0.6rem" }}>
                <span className={`badge ${KIND_BADGE[s.kind] ?? "neutral"}`}>{t(`backups.kind_${s.kind}`)}</span>
                <span className="grow ellipsis"><strong>{s.label}</strong></span>
                <span className="faint">{formatDate(s.createdMs)}</span>
                <span className="faint">{t("backups.items", { n: formatNumber(s.entries) })}</span>
                <strong style={{ minWidth: "5rem", textAlign: "right" }}>{formatBytes(s.bytes)}</strong>
                <button className="btn sm" onClick={() => void inspect(s)}>{t("backups.inspect")}</button>
                <button className="btn sm ghost icon" aria-label={t("backups.delete")} title={t("backups.delete")} disabled={busy} onClick={() => void remove(s)}>
                  <Trash2 size={14} />
                </button>
              </div>
              <div className="faint" style={{ fontSize: "0.8rem", paddingLeft: "0.2rem" }}>
                {s.restorable ? t("backups.restorable", { n: s.restorable }) : t("backups.nothingToRestore")}
              </div>
            </div>
          ))}
          {sets && !sets.length && <div className="empty">{t("backups.empty")}</div>}
        </div>
      </div>
    </div>
  );
}
