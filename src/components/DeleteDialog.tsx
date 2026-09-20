import { AlertTriangle, CheckCircle2, Link2, ShieldAlert, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, toError } from "../services/api";
import { useT } from "../i18n";
import { useApp } from "../stores/app";
import type { DeleteMode, DeletePlan, DeleteSummary, ErrorPayload } from "../types";
import { formatBytes } from "../utils/format";
import { ErrorView } from "./ErrorView";
import { Modal } from "./Modal";

/**
 * Global delete flow: plan (backend classifies and fingerprints every item)
 * → the user reviews exactly what will happen → execute (re-verified in the
 * backend) → per-item results. Nothing is removed before confirmation.
 */
export function DeleteDialog() {
  const t = useT();
  const { deleteRequest: req, closeDelete, settings, toast } = useApp();
  const [mode, setMode] = useState<DeleteMode>("recycleBin");
  const [plan, setPlan] = useState<DeletePlan>();
  const [error, setError] = useState<ErrorPayload>();
  const [dryRun, setDryRun] = useState(false);
  const [ack, setAck] = useState(false);
  const [running, setRunning] = useState<{ done: number; total: number }>();
  const [summary, setSummary] = useState<DeleteSummary>();

  useEffect(() => {
    if (!req) return;
    setMode(req.mode);
    setDryRun(settings.dryRunDefault);
    setAck(false);
    setSummary(undefined);
    setRunning(undefined);
  }, [req, settings.dryRunDefault]);

  useEffect(() => {
    if (!req) return;
    setPlan(undefined);
    setError(undefined);
    api.planDelete(req.scanId, req.targets, mode).then(setPlan).catch((e) => setError(toError(e)));
  }, [req, mode]);

  if (!req) return null;

  const actionable = plan ? plan.items.filter((i) => i.assessment.risk !== "blocked" && !i.error) : [];
  const canRun = plan && actionable.length > 0 && (plan.dangerous === 0 || ack || dryRun) && !running;

  const execute = async () => {
    if (!plan) return;
    setRunning({ done: 0, total: plan.items.length });
    try {
      const s = await api.executeDelete(plan.id, dryRun, ack, (index, total) => setRunning({ done: index + 1, total }));
      setSummary(s);
      if (!dryRun && s.deleted > 0) {
        toast("success", t("deleteDialog.doneDeleted", { n: s.deleted, size: formatBytes(s.freed) }));
      }
      if (!dryRun) req.onDone?.();
    } catch (e) {
      setError(toError(e));
    } finally {
      setRunning(undefined);
    }
  };

  const close = () => {
    if (running) return;
    closeDelete();
  };

  const title = mode === "permanent" ? t("deleteDialog.titlePermanent") : t("deleteDialog.titleRecycle");

  if (summary) {
    return (
      <Modal title={t("deleteDialog.doneTitle")} icon={<CheckCircle2 size={18} color="var(--success)" />} onClose={close} wide
        footer={<button className="btn primary" onClick={close} data-autofocus>{t("common.close")}</button>}>
        <div className="row wrap" style={{ gap: "0.5rem" }}>
          {dryRun ? (
            <span className="badge review">{t("deleteDialog.doneSimulated", { n: summary.wouldDelete, size: formatBytes(summary.freed) })}</span>
          ) : (
            <span className="badge safe">{t("deleteDialog.doneDeleted", { n: summary.deleted, size: formatBytes(summary.freed) })}</span>
          )}
          {summary.skipped > 0 && <span className="badge neutral">{t("deleteDialog.doneSkipped", { n: summary.skipped })}</span>}
          {summary.failed > 0 && <span className="badge blocked">{t("deleteDialog.doneFailed", { n: summary.failed })}</span>}
        </div>
        <div className="plan-list">
          {summary.results.map((r, i) => (
            <div key={i} className="plan-item">
              <span className={`badge ${r.status === "deleted" ? "safe" : r.status === "wouldDelete" ? "review" : r.status === "skipped" ? "neutral" : "blocked"}`}>
                {r.status}
              </span>
              <span className="grow ellipsis mono" title={r.path}>{r.path}</span>
              {r.status === "skipped" && <span className="faint">{t(`risk.reason_${r.reason}`)}</span>}
              {r.status === "failed" && <span className="faint ellipsis" title={r.error.message}>{r.error.message}</span>}
            </div>
          ))}
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      title={title}
      icon={<Trash2 size={18} color={mode === "permanent" ? "var(--danger)" : "var(--text-muted)"} />}
      onClose={close}
      wide
      locked={!!running}
      footer={
        <>
          {running && <span className="muted grow">{t("deleteDialog.working", { done: running.done, total: running.total })}</span>}
          <button className="btn" onClick={close} disabled={!!running}>{t("common.cancel")}</button>
          <button className={`btn ${dryRun ? "primary" : "danger"}`} disabled={!canRun} onClick={execute}>
            {dryRun ? t("deleteDialog.simulate") : t("deleteDialog.execute")}
          </button>
        </>
      }
    >
      <div className="muted">{t("deleteDialog.intro")}</div>
      <div className="row wrap" style={{ gap: "1rem" }}>
        <span className="muted">{t("deleteDialog.mode")}</span>
        <div className="seg" role="radiogroup">
          <button className={mode === "recycleBin" ? "on" : ""} onClick={() => setMode("recycleBin")} disabled={!!running}>{t("deleteDialog.modeRecycle")}</button>
          <button className={mode === "permanent" ? "on" : ""} onClick={() => setMode("permanent")} disabled={!!running}>{t("deleteDialog.modePermanent")}</button>
        </div>
        {plan && <strong>{t("deleteDialog.total", { n: actionable.length, size: formatBytes(actionable.reduce((a, i) => a + i.size, 0)) })}</strong>}
      </div>
      {mode === "permanent" && !dryRun && (
        <div className="banner warning"><AlertTriangle size={17} color="var(--warning)" /><span>{t("deleteDialog.permanentWarning")}</span></div>
      )}
      {error && <ErrorView error={error} />}
      {!plan && !error && <div className="muted">{t("common.loading")}</div>}
      {plan && (
        <>
          <div className="plan-list">
            {plan.items.map((i) => (
              <div key={i.path} className="plan-item">
                <span className={`badge ${i.assessment.risk}`} title={t(`risk.reason_${i.assessment.reason}`)}>{t(`risk.${i.assessment.risk}`)}</span>
                <span className="grow ellipsis mono" title={i.finalPath !== i.path ? `${i.path}\n→ ${i.finalPath}` : i.path}>{i.path}</span>
                {i.isLink && <span className="faint row" style={{ gap: 3 }}><Link2 size={13} />{t("deleteDialog.linkOnly")}</span>}
                {i.error ? (
                  <span className="faint ellipsis" style={{ maxWidth: "16rem", color: "var(--danger)" }} title={i.error.message}>{t("deleteDialog.error")}: {i.error.message}</span>
                ) : i.assessment.risk === "blocked" ? (
                  <span className="faint">{t(`risk.reason_${i.assessment.reason}`)} — {t("deleteDialog.willSkip")}</span>
                ) : (
                  <span className="muted" style={{ minWidth: "5rem", textAlign: "right" }}>{formatBytes(i.size)}</span>
                )}
              </div>
            ))}
          </div>
          {plan.blocked > 0 && (
            <div className="banner danger"><ShieldAlert size={17} color="var(--danger)" /><span>{t("deleteDialog.blockedNote", { n: plan.blocked })}</span></div>
          )}
          {plan.dangerous > 0 && (
            <div className="banner warning col" style={{ alignItems: "stretch" }}>
              <span>{t("deleteDialog.dangerousNote", { n: plan.dangerous })}</span>
              <label className="checkbox">
                <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
                <span>{t("deleteDialog.dangerousAck")}</span>
              </label>
            </div>
          )}
          <label className="checkbox">
            <input type="checkbox" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
            <span>{t("deleteDialog.dryRun")}</span>
          </label>
        </>
      )}
    </Modal>
  );
}
