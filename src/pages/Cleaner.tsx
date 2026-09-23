import { AlertTriangle, ChevronDown, ChevronRight, LogOut, Play, ShieldAlert, Sparkles, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { Modal } from "../components/Modal";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { CategorySummary, CleanGroup, CleanItem, CleanReport, ErrorPayload } from "../types";
import { formatBytes, formatNumber } from "../utils/format";

const GROUPS: CleanGroup[] = ["system", "apps", "browsers", "privacy"];

function label(c: CategorySummary["category"], t: (k: string) => string) {
  const base = t(`cleaner.cat_${c.label}`);
  return c.owner ? `${c.owner} — ${base}` : base;
}

/** The items a category would remove, in pages. */
function Items({ id, count }: { id: string; count: number }) {
  const t = useT();
  const [rows, setRows] = useState<CleanItem[]>([]);
  const [error, setError] = useState<ErrorPayload>();
  const load = useCallback(
    async (offset: number) => {
      try {
        const p = await api.cleanerItems(id, offset, 200);
        // Replace on the first page so a re-run of the effect cannot duplicate rows.
        setRows((r) => (offset === 0 ? p.rows : [...r, ...p.rows]));
      } catch (e) {
        setError(toError(e));
      }
    },
    [id],
  );
  useEffect(() => {
    setRows([]);
    void load(0);
  }, [load]);
  return (
    <div className="col" style={{ gap: "0.15rem", padding: "0.3rem 0 0.5rem 2.2rem", maxHeight: "18rem", overflow: "auto" }}>
      {error && <ErrorView error={error} />}
      {rows.map((i) => (
        <div key={i.path} className="row" style={{ gap: "0.6rem", fontSize: "0.8rem" }}>
          <span className="mono ellipsis grow" title={i.path}>{i.display || i.path}</span>
          {i.size > 0 && <span className="faint">{formatBytes(i.size)}</span>}
        </div>
      ))}
      {rows.length < count && (
        <button className="btn ghost sm" style={{ alignSelf: "flex-start" }} onClick={() => void load(rows.length)}>
          {t("cleaner.loadMore")} ({formatNumber(count - rows.length)})
        </button>
      )}
    </div>
  );
}

export function Cleaner() {
  const t = useT();
  const app = useApp();
  const [cats, setCats] = useState<CategorySummary[]>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [dryRun, setDryRun] = useState(app.settings.dryRunDefault);
  const [confirming, setConfirming] = useState(false);
  const [restorePoint, setRestorePoint] = useState(false);
  const [running, setRunning] = useState<string>();
  const [report, setReport] = useState<CleanReport>();

  // keep_report: after cleaning, the summary of what was removed stays on screen.
  const analyze = useCallback(async (keepReport = false) => {
    setBusy(true);
    setError(undefined);
    if (!keepReport) setReport(undefined);
    try {
      const r = await api.cleanerAnalyze();
      setCats(r);
      // Pre-selected: safe categories with something to clean and not blocked.
      setSelected(new Set(r.filter((c) => c.category.defaultOn && c.count > 0 && !c.running && !c.error).map((c) => c.category.id)));
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  }, []);

  // Machine-wide folders (C:\Windows\Temp...) cannot even be listed by a
  // standard user: one elevated read lists them.
  const analyzeAdmin = async () => {
    setBusy(true);
    try {
      setCats(await api.cleanerAnalyzeAdmin());
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  // Ask a blocked browser/app to close (never forced), then analyze again.
  const closeProgram = async (id: string, name: string) => {
    setBusy(true);
    try {
      const left = await api.cleanerCloseProgram(id);
      if (left.length) app.toast("info", t("cleaner.stillOpen", { name }));
      else app.toast("success", t("cleaner.closed", { name }));
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
    await analyze(true);
  };

  const byId = useMemo(() => new Map((cats ?? []).map((c) => [c.category.id, c])), [cats]);
  const chosen = useMemo(() => [...selected].map((id) => byId.get(id)).filter((c): c is CategorySummary => !!c), [selected, byId]);
  const totalBytes = chosen.reduce((a, c) => a + c.bytes, 0);
  // A big or risky cleanup is where a restore point earns its minute.
  const big = totalBytes >= 2 * 1024 ** 3 || chosen.some((c) => c.category.risk === "dangerous" || c.category.admin);

  const toggle = (id: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const run = async () => {
    setConfirming(false);
    setBusy(true);
    setError(undefined);
    try {
      if (restorePoint && !dryRun) {
        setRunning(t("cleaner.creatingRestorePoint"));
        try {
          await api.createRestorePoint(t("backups.restorePointName"));
        } catch (e) {
          // Windows refuses when System Protection is off, or one was just
          // made: say so and let the user decide, without cleaning anything.
          setError(toError(e));
          setRunning(undefined);
          setBusy(false);
          return;
        }
      }
      const r = await api.cleanerRun([...selected], dryRun, (e) => {
        if (e.event === "category") setRunning(e.data.id);
      });
      setReport(r);
      app.toast("success", r.dryRun ? t("cleaner.wouldFree", { size: formatBytes(r.freed) }) : t("cleaner.freed", { size: formatBytes(r.freed) }));
      if (!r.dryRun) await analyze(true);
    } catch (e) {
      setError(toError(e));
    }
    setRunning(undefined);
    setBusy(false);
  };

  useEffect(() => {
    if (!cats && !busy) void analyze();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const riskClass = { safe: "safe", review: "review", dangerous: "dangerous" } as const;

  return (
    <div className="page fill">
      <div className="toolbar">
        <Sparkles size={16} className="muted" />
        <strong>{t("cleaner.title")}</strong>
        <span className="grow" />
        <label className="row" style={{ gap: "0.4rem", fontSize: "0.88rem" }}>
          <input type="checkbox" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} style={{ accentColor: "var(--accent)" }} />
          <span className="muted">{t("cleaner.dryRun")}</span>
        </label>
        <button className="btn" disabled={busy} onClick={() => void analyze()}>
          <Play size={14} />{busy && !running ? t("cleaner.analyzing") : cats ? t("cleaner.reanalyze") : t("cleaner.analyze")}
        </button>
        {cats?.some((c) => c.adminPending) && (
          <button className="btn" disabled={busy} onClick={() => void analyzeAdmin()}><ShieldAlert size={14} />{t("cleaner.analyzeAdmin")}</button>
        )}
        <button className="btn danger" disabled={busy || selected.size === 0} onClick={() => { setRestorePoint(big || app.settings.createRestorePoint); setConfirming(true); }}>
          <Trash2 size={14} />{t("cleaner.clean")}
        </button>
      </div>
      <div className="row" style={{ padding: "0.5rem 1rem", gap: "0.8rem", fontSize: "0.88rem", borderBottom: "1px solid var(--border)" }}>
        <strong>{t("cleaner.selectedTotal", { n: selected.size, size: formatBytes(totalBytes) })}</strong>
        <span className="faint grow ellipsis">{running ? t("cleaner.cleaning", { name: running }) : t("cleaner.intro")}</span>
      </div>

      <div className="page-body">
        {error && <ErrorView error={error} onRetry={() => void analyze()} />}
        {!cats && busy && <div className="empty">{t("cleaner.analyzing")}</div>}

        {report && (
          <div className="card" style={{ padding: "0.8rem", marginBottom: "0.9rem" }}>
            <div className="row" style={{ marginBottom: "0.5rem" }}>
              <strong className="grow">{report.dryRun ? t("cleaner.doneDry") : t("cleaner.done")}</strong>
              <strong>{report.dryRun ? t("cleaner.wouldFree", { size: formatBytes(report.freed) }) : t("cleaner.freed", { size: formatBytes(report.freed) })}</strong>
            </div>
            <div className="col" style={{ gap: "0.25rem" }}>
              {report.results.map((r) => {
                const o = r.outcome;
                return (
                  <div key={r.id} className="row" style={{ gap: "0.5rem", fontSize: "0.85rem" }}>
                    <span className="grow ellipsis">{byId.get(r.id) ? label(byId.get(r.id)!.category, t) : r.id}</span>
                    {r.error ? (
                      <span style={{ color: "var(--danger)" }}>{r.error.message}</span>
                    ) : (
                      <>
                        <span>{t("cleaner.res_removed", { n: formatNumber(o?.removed ?? 0) })}</span>
                        {!!o?.freed && <span className="faint">{formatBytes(o.freed)}</span>}
                        {!!o?.inUse && <span className="faint">{t("cleaner.res_inUse", { n: o.inUse })}</span>}
                        {!!o?.changed && <span className="faint">{t("cleaner.res_changed", { n: o.changed })}</span>}
                        {!!o?.denied && <span className="faint">{t("cleaner.res_denied", { n: o.denied })}</span>}
                        {!!o?.failed && <span style={{ color: "var(--danger)" }}>{t("cleaner.res_failed", { n: o.failed })}</span>}
                      </>
                    )}
                  </div>
                );
              })}
            </div>
            {report.backupDir && <div className="faint mono" style={{ fontSize: "0.8rem", marginTop: "0.4rem" }}>{t("cleaner.backup")}: {report.backupDir}</div>}
          </div>
        )}

        {cats &&
          GROUPS.map((g) => {
            const list = cats.filter((c) => c.category.group === g);
            if (!list.length) return null;
            return (
              <div key={g} style={{ marginBottom: "1rem" }}>
                <div className="section-title" style={{ marginTop: 0 }}>{t(`cleaner.group_${g}`)}</div>
                <div className="col" style={{ gap: "0.3rem" }}>
                  {list.map((c) => {
                    const id = c.category.id;
                    const blocked = c.running || c.adminPending || !!c.error || (c.count === 0 && c.category.special?.kind !== "recycleBin");
                    return (
                      <div key={id} className="card" style={{ padding: "0.5rem 0.7rem", opacity: blocked ? 0.65 : 1 }}>
                        <div className="row" style={{ gap: "0.5rem" }}>
                          <input
                            type="checkbox"
                            checked={selected.has(id)}
                            disabled={blocked || c.count === 0}
                            onChange={() => toggle(id)}
                            aria-label={label(c.category, t)}
                            style={{ accentColor: "var(--accent)" }}
                          />
                          <span className="ellipsis" style={{ minWidth: "14rem" }}>{label(c.category, t)}</span>
                          <span className={`badge ${riskClass[c.category.risk]}`}>{t(`cleaner.risk_${c.category.risk}`)}</span>
                          {c.category.admin && <span className="badge neutral">{t("cleaner.admin")}</span>}
                          <span className="grow" />
                          {c.count > 0 && <span className="faint">{t("cleaner.items", { n: formatNumber(c.count) })}</span>}
                          <strong style={{ minWidth: "5rem", textAlign: "right" }}>{c.bytes > 0 ? formatBytes(c.bytes) : c.count > 0 ? "—" : t("cleaner.nothing")}</strong>
                          <button className="btn ghost sm icon" disabled={c.count === 0} aria-label={t("cleaner.showItems")} title={t(open.has(id) ? "cleaner.hideItems" : "cleaner.showItems")}
                            onClick={() => setOpen((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; })}>
                            {open.has(id) ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                          </button>
                        </div>
                        <div className="row wrap" style={{ gap: "0.6rem", fontSize: "0.8rem", paddingLeft: "1.4rem" }}>
                          {c.running && (
                            <>
                              <span style={{ color: "var(--warning)" }}>{t("cleaner.running", { name: c.category.owner ?? "" })}</span>
                              <button className="btn ghost sm" disabled={busy} onClick={() => void closeProgram(id, c.category.owner ?? id)}>
                                <LogOut size={13} />{t("cleaner.closeApp", { name: c.category.owner ?? "" })}
                              </button>
                            </>
                          )}
                          {c.category.warning && <span className="faint">{t(`cleaner.w_${c.category.warning}`)}</span>}
                          {c.recentSkipped > 0 && <span className="faint">{t("cleaner.recentKept", { n: formatNumber(c.recentSkipped) })}</span>}
                          {c.adminPending && <span className="faint">{t("cleaner.adminPending")}</span>}
                          {c.truncated && <span className="faint">{t("cleaner.truncated")}</span>}
                          {c.error && <span style={{ color: "var(--danger)" }}>{c.error.message}</span>}
                        </div>
                        {open.has(id) && <Items key={`${id}-${c.count}-${c.bytes}`} id={id} count={c.count} />}
                      </div>
                    );
                  })}
                </div>
                {g === "browsers" && <div className="faint" style={{ fontSize: "0.8rem", marginTop: "0.4rem" }}>{t("cleaner.firefoxHistory")}</div>}
              </div>
            );
          })}
      </div>

      {confirming && (
        <Modal
          title={t("cleaner.confirmTitle")}
          icon={<Trash2 size={17} color="var(--danger)" />}
          onClose={() => setConfirming(false)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirming(false)}>{t("common.cancel")}</button>
              <button className="btn danger" onClick={() => void run()}><Trash2 size={14} />{dryRun ? t("cleaner.dryRun") : t("cleaner.clean")}</button>
            </>
          }
        >
          <p style={{ margin: 0 }}>{dryRun ? t("cleaner.confirmDry") : t("cleaner.confirmBody")}</p>
          <div className="col" style={{ gap: "0.25rem" }}>
            {chosen.map((c) => (
              <div key={c.category.id} className="row" style={{ gap: "0.5rem", fontSize: "0.88rem" }}>
                <span className="grow ellipsis">{label(c.category, t)}</span>
                <span className="faint">{t("cleaner.items", { n: formatNumber(c.count) })}</span>
                <strong>{formatBytes(c.bytes)}</strong>
              </div>
            ))}
          </div>
          {chosen.some((c) => c.category.risk === "dangerous") && (
            <div className="banner danger">
              <AlertTriangle size={16} />
              <span>{chosen.filter((c) => c.category.warning && c.category.risk === "dangerous").map((c) => t(`cleaner.w_${c.category.warning}`)).join(" ")}</span>
            </div>
          )}
          {chosen.some((c) => c.category.admin) && <div className="muted">{t("cleaner.adminNote")}</div>}
          {!dryRun && (
            <label className="checkbox">
              <input type="checkbox" checked={restorePoint} onChange={(e) => setRestorePoint(e.target.checked)} />
              <span>{t("cleaner.restorePoint")}{big && ` — ${t("cleaner.restorePointBig")}`}</span>
            </label>
          )}
          <div className="row" style={{ justifyContent: "space-between", fontWeight: 600 }}>
            <span>{t("programs.total")}</span>
            <span>{formatBytes(totalBytes)}</span>
          </div>
        </Modal>
      )}
    </div>
  );
}
