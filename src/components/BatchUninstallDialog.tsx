import { CheckCircle2, Clock, FolderOpen, Loader2, ShieldAlert, Trash2, XCircle, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, Leftover, LeftoverLevel, PreparedUninstall, RemovalSummary, RunOutcome } from "../types";
import { formatBytes } from "../utils/format";
import { ErrorView } from "./ErrorView";
import { isUnambiguous, LeftoverList, RemovalCounts, RemovalResultList } from "./leftovers";
import { Modal } from "./Modal";
import { ProgramIcon } from "./ProgramIcon";

type Step = "preparing" | "confirm" | "running" | "scanning" | "review" | "removing" | "done";
type RowStatus = "pending" | "running" | "done" | "stillInstalled" | "error" | "skipped";

interface Row {
  prep: PreparedUninstall;
  status: RowStatus;
  processes: string[];
  outcome?: RunOutcome;
  error?: ErrorPayload;
  leftovers: Leftover[];
  auto?: RemovalSummary;
  final?: RemovalSummary;
}

const STATUS_ICON: Record<RowStatus, React.ReactNode> = {
  pending: <Clock size={14} className="faint" />,
  running: <Loader2 size={14} className="spin" />,
  done: <CheckCircle2 size={14} color="var(--success)" />,
  stillInstalled: <XCircle size={14} color="var(--warning)" />,
  error: <XCircle size={14} color="var(--danger)" />,
  skipped: <XCircle size={14} className="faint" />,
};

/**
 * Batch and quick uninstall. Programs run strictly one after another (never
 * several uninstallers at once); leftovers of all of them are reviewed
 * together and removed with a single elevation prompt.
 */
export function BatchUninstallDialog() {
  const t = useT();
  const { batchRequest: req, closeBatch, settings, toast } = useApp();
  const [step, setStep] = useState<Step>("preparing");
  const [rows, setRows] = useState<Row[]>([]);
  const [current, setCurrent] = useState(0);
  const [quiet, setQuiet] = useState(true);
  const [quick, setQuick] = useState(false);
  const [restorePoint, setRestorePoint] = useState(false);
  const [level, setLevel] = useState<LeftoverLevel>("moderate");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [recycle, setRecycle] = useState(true);
  const [dryRun, setDryRun] = useState(false);
  const [ack, setAck] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const rowsRef = useRef<Row[]>([]);
  rowsRef.current = rows;

  const patch = (i: number, p: Partial<Row>) => setRows((rs) => rs.map((r, k) => (k === i ? { ...r, ...p } : r)));

  useEffect(() => {
    if (!req) return;
    setStep("preparing");
    setError(undefined);
    setQuick(req.quick);
    setQuiet(true);
    setAck(false);
    setRestorePoint(settings.createRestorePoint);
    setLevel(settings.defaultLeftoverLevel);
    setRecycle(settings.useRecycleBin);
    setDryRun(settings.dryRunDefault);
    (async () => {
      const out: Row[] = [];
      for (const id of req.ids) {
        try {
          const prep = await api.uninstallPrepare(id);
          out.push({ prep, status: prep.command ? "pending" : "skipped", processes: [], leftovers: [] });
        } catch (e) {
          setError(toError(e));
        }
      }
      setRows(out);
      setStep("confirm");
    })();
  }, [req]); // eslint-disable-line react-hooks/exhaustive-deps

  /** Run one uninstaller and resolve when it (and its process tree) ends. */
  const runOne = (i: number, row: Row) =>
    new Promise<Partial<Row>>((resolve) => {
      patch(i, { status: "running" });
      const useQuiet = quiet && !!row.prep.quietCommand;
      const finish = (p: Partial<Row>) => {
        patch(i, p);
        resolve(p);
      };
      api
        .uninstallRun(row.prep.sessionId, useQuiet, (e) => {
          if (e.event === "waiting") patch(i, { processes: e.data.processes });
          else if (e.event === "done") {
            finish({ outcome: e.data.outcome, status: e.data.outcome.stillInstalled ? "stillInstalled" : "done", processes: [] });
          } else {
            finish({ status: "error", error: e.data });
          }
        })
        .catch((e) => finish({ status: "error", error: toError(e) }));
    });

  const start = async () => {
    setStep("running");
    setError(undefined);
    // Local copy updated synchronously: React state may lag behind events.
    const list = rowsRef.current.map((r) => ({ ...r }));
    const firstRunnable = list.find((r) => r.status === "pending");
    if (restorePoint && firstRunnable) {
      try {
        await api.uninstallRestorePoint(firstRunnable.prep.sessionId);
      } catch (e) {
        // Shown, but the user asked for the batch: continue.
        setError(toError(e));
      }
    }
    for (let i = 0; i < list.length; i++) {
      if (list[i].status !== "pending") continue;
      setCurrent(i);
      list[i] = { ...list[i], ...(await runOne(i, list[i])) };
    }
    await scanAll(list);
  };

  const scanAll = async (list: Row[]) => {
    setStep("scanning");
    const scanned: Row[] = [];
    for (const r of list) {
      if (r.status !== "done") {
        scanned.push(r);
        continue;
      }
      try {
        scanned.push({ ...r, leftovers: await api.leftoversScan(r.prep.sessionId, level) });
      } catch (e) {
        scanned.push({ ...r, error: toError(e) });
      }
    }
    // Quick mode: remove unambiguous items right away, keep the rest for review.
    if (quick) {
      const selections = scanned
        .map((r) => ({ sessionId: r.prep.sessionId, ids: r.leftovers.filter(isUnambiguous).map((l) => l.id) }))
        .filter((s) => s.ids.length);
      if (selections.length) {
        try {
          const res = await api.leftoversRemoveBatch(selections, recycle, false, dryRun);
          for (const [sid, sum] of res) {
            const k = scanned.findIndex((r) => r.prep.sessionId === sid);
            if (k >= 0) {
              const done = new Set(sum.results.filter((x) => x.status === "removed" || x.status === "wouldRemove").map((x) => x.id));
              scanned[k] = { ...scanned[k], auto: sum, leftovers: scanned[k].leftovers.filter((l) => !done.has(l.id)) };
            }
          }
        } catch (e) {
          setError(toError(e));
        }
      }
    }
    setRows(scanned);
    const pre = new Set<string>();
    for (const r of scanned) for (const l of r.leftovers) if (l.preselected) pre.add(`${r.prep.sessionId}:${l.id}`);
    setSelected(pre);
    setStep("review");
  };

  const remove = async () => {
    setStep("removing");
    const selections = rows
      .map((r) => ({ sessionId: r.prep.sessionId, ids: r.leftovers.filter((l) => selected.has(`${r.prep.sessionId}:${l.id}`)).map((l) => l.id) }))
      .filter((s) => s.ids.length);
    try {
      const res = await api.leftoversRemoveBatch(selections, recycle, ack, dryRun);
      setRows((rs) => rs.map((r) => ({ ...r, final: res.find(([sid]) => sid === r.prep.sessionId)?.[1] })));
      const n = res.reduce((s, [, sum]) => s + sum.results.filter((x) => x.status === "removed").length, 0);
      if (n) toast("success", t("uninstall.removed", { n }));
    } catch (e) {
      setError(toError(e));
    }
    setStep("done");
  };

  const close = () => {
    if (step === "running" || step === "removing" || step === "scanning" || step === "preparing") return;
    for (const r of rows) void api.uninstallFinish(r.prep.sessionId).catch(() => {});
    closeBatch();
    void usePrograms.getState().load(true);
  };

  const allLeftovers = useMemo(() => rows.flatMap((r) => r.leftovers.map((l) => ({ key: `${r.prep.sessionId}:${l.id}`, l }))), [rows]);
  const chosen = allLeftovers.filter((x) => selected.has(x.key));
  const dangerousSelected = chosen.some((x) => x.l.risk === "dangerous");
  const autoCount = rows.reduce((s, r) => s + (r.auto?.results.filter((x) => x.status === "removed" || x.status === "wouldRemove").length ?? 0), 0);
  if (!req) return null;

  const title = req.quick && req.ids.length === 1 ? t("batch.quickTitle") : t("batch.title", { n: req.ids.length });
  const runnable = rows.filter((r) => r.status === "pending").length;

  const queue = (
    <div className="plan-list">
      {rows.map((r, i) => (
        <div key={r.prep.sessionId} className="plan-item">
          {STATUS_ICON[r.status]}
          <ProgramIcon id={r.prep.program.id} />
          <span className="grow ellipsis">{r.prep.program.name}</span>
          {r.status === "skipped" && r.prep.commandError && (
            <span className="faint ellipsis" style={{ maxWidth: "20rem" }} title={r.prep.commandError.message}>
              {t("batch.willSkip", { reason: r.prep.commandError.message })}
            </span>
          )}
          {r.status === "running" && r.processes.length > 0 && <span className="mono faint">{r.processes.join(", ")}</span>}
          {r.status === "error" && r.error && <span className="faint ellipsis" style={{ maxWidth: "20rem" }} title={r.error.message}>{r.error.message}</span>}
          <span className={`badge ${r.status === "done" ? "safe" : r.status === "stillInstalled" || r.status === "error" ? "blocked" : "neutral"}`}>
            {step === "running" && i === current && r.status === "running" ? t("batch.progress", { i: i + 1, n: rows.length }) : t(`batch.status_${r.status}`)}
          </span>
        </div>
      ))}
    </div>
  );

  let body: React.ReactNode = null;
  let footer: React.ReactNode = null;
  if (step === "preparing") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t("common.loading")}</div>;
  } else if (step === "confirm") {
    body = (
      <>
        <div className="muted">{quick ? t("batch.quickIntro") : t("batch.intro")}</div>
        {queue}
        <label className="checkbox"><input type="checkbox" checked={quiet} onChange={(e) => setQuiet(e.target.checked)} /><span>{t("batch.preferQuiet")}</span></label>
        <label className="checkbox"><input type="checkbox" checked={quick} onChange={(e) => setQuick(e.target.checked)} /><span>{t("batch.quick")}</span></label>
        <label className="checkbox"><input type="checkbox" checked={restorePoint} onChange={(e) => setRestorePoint(e.target.checked)} /><span>{t("uninstall.restorePoint")}</span></label>
        <div className="row wrap" style={{ gap: "0.6rem" }}>
          <span className="muted">{t("uninstall.level")}</span>
          <div className="seg">
            {(["safe", "moderate", "advanced"] as LeftoverLevel[]).map((l) => (
              <button key={l} className={level === l ? "on" : ""} onClick={() => setLevel(l)}>{t(`uninstall.level_${l}`)}</button>
            ))}
          </div>
        </div>
        {error && <ErrorView error={error} />}
      </>
    );
    footer = (
      <>
        <button className="btn" onClick={close}>{t("common.cancel")}</button>
        <button className="btn danger" disabled={!runnable} onClick={start}>{quick ? <Zap size={14} /> : <Trash2 size={14} />}{t("batch.start")}</button>
      </>
    );
  } else if (step === "running" || step === "scanning") {
    body = (
      <>
        {step === "running" && <div className="muted">{t("uninstall.runningHint")}</div>}
        {queue}
        {step === "scanning" && <div className="muted row"><Loader2 size={16} className="spin" />{t("batch.scanningAll")}</div>}
        {error && <ErrorView error={error} />}
      </>
    );
    footer = step === "running" && rows[current] && (
      <button className="btn" onClick={() => api.uninstallStopWaiting(rows[current].prep.sessionId)}>{t("uninstall.stopWaiting")}</button>
    );
  } else if (step === "review") {
    const size = chosen.reduce((s, x) => s + x.l.size, 0);
    const withItems = rows.filter((r) => r.leftovers.length);
    body = (
      <>
        {queue}
        {autoCount > 0 && <div className="banner success"><CheckCircle2 size={16} color="var(--success)" /><span>{t("batch.autoRemoved", { n: autoCount })}</span></div>}
        {error && <ErrorView error={error} />}
        {withItems.length === 0 ? (
          <div className="card muted">{quick ? t("batch.nothingToReview") : t("uninstall.noLeftovers")}</div>
        ) : (
          <>
            <strong>{quick ? t("batch.reviewRest") : t("uninstall.leftoversTitle")}</strong>
            <div className="plan-list" style={{ maxHeight: "20rem" }}>
              {withItems.map((r) => (
                <div key={r.prep.sessionId}>
                  <div className="plan-item" style={{ background: "var(--bg-active)", fontWeight: 600 }}>
                    <ProgramIcon id={r.prep.program.id} />{r.prep.program.name}
                  </div>
                  <LeftoverList
                    items={r.leftovers}
                    keyPrefix={`${r.prep.sessionId}:`}
                    selected={selected}
                    onToggle={(k) => {
                      const s = new Set(selected);
                      if (s.has(k)) s.delete(k);
                      else s.add(k);
                      setSelected(s);
                    }}
                  />
                </div>
              ))}
            </div>
            <strong>{t("uninstall.selectedSummary", { n: chosen.length, size: formatBytes(size) })}</strong>
            <label className="checkbox"><input type="checkbox" checked={recycle} onChange={(e) => setRecycle(e.target.checked)} /><span>{t("uninstall.recycle")}</span></label>
            <label className="checkbox"><input type="checkbox" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} /><span>{t("uninstall.dryRun")}</span></label>
            {dangerousSelected && (
              <div className="banner warning"><ShieldAlert size={16} color="var(--warning)" />
                <label className="checkbox"><input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} /><span>{t("uninstall.dangerousAck")}</span></label>
              </div>
            )}
          </>
        )}
      </>
    );
    footer = (
      <>
        <button className="btn" onClick={close}>{withItems.length ? t("uninstall.skip") : t("uninstall.close")}</button>
        {withItems.length > 0 && (
          <button className={`btn ${dryRun ? "primary" : "danger"}`} disabled={!chosen.length || (dangerousSelected && !ack && !dryRun)} onClick={remove}>
            {dryRun ? t("uninstall.simulate") : t("uninstall.remove")}
          </button>
        )}
      </>
    );
  } else if (step === "removing") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t("uninstall.removing")}</div>;
  } else if (step === "done") {
    body = (
      <>
        {error && <ErrorView error={error} />}
        {rows.filter((r) => r.final || r.auto).map((r) => {
          const results = [...(r.auto?.results ?? []), ...(r.final?.results ?? [])];
          const backup = r.final?.backupDir ?? r.auto?.backupDir;
          return (
            <div key={r.prep.sessionId} className="col" style={{ gap: "0.4rem" }}>
              <div className="row"><ProgramIcon id={r.prep.program.id} /><strong className="grow">{r.prep.program.name}</strong><RemovalCounts results={results} /></div>
              <div className="plan-list" style={{ maxHeight: "12rem" }}><RemovalResultList results={results} /></div>
              {backup && !dryRun && (
                <div className="row">
                  <span className="mono faint ellipsis grow" title={backup}>{backup}</span>
                  <button className="btn sm" onClick={() => api.openPath(backup).catch(() => {})}><FolderOpen size={13} />{t("uninstall.openBackup")}</button>
                </div>
              )}
            </div>
          );
        })}
      </>
    );
    footer = <button className="btn primary" onClick={close}>{t("uninstall.close")}</button>;
  }

  return (
    <Modal title={title} icon={quick ? <Zap size={18} color="var(--warning)" /> : <Trash2 size={18} />} onClose={close} wide
      locked={step === "running" || step === "removing" || step === "scanning" || step === "preparing"} footer={footer}>
      {body}
    </Modal>
  );
}
