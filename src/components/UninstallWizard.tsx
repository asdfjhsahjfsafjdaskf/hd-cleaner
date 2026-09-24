import { AlertTriangle, CheckCircle2, FolderOpen, Loader2, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, Leftover, LeftoverLevel, PreparedUninstall, RemovalSummary, RunOutcome, UninstallImpact } from "../types";
import { formatBytes, formatDuration } from "../utils/format";
import { ErrorView } from "./ErrorView";
import { LeftoverList, RemovalCounts, RemovalResultList } from "./leftovers";
import { Modal } from "./Modal";
import { ProgramIcon } from "./ProgramIcon";

type Step = "loading" | "confirm" | "restorePoint" | "running" | "stillInstalled" | "scanning" | "leftovers" | "removing" | "done";


/**
 * Uninstall flow: confirm → optional restore point → official uninstaller
 * (waiting for its whole process tree) → leftover scan → the user picks what
 * to remove → backup + removal → per-item result. Opened via useApp().requestUninstall.
 */
/** What the uninstall is about to touch, measured now. */
function ImpactCard({ impact }: { impact?: UninstallImpact }) {
  const t = useT();
  if (!impact) {
    return <div className="muted row" style={{ gap: "0.4rem", fontSize: "0.85rem" }}><Loader2 size={14} className="spin" />{t("uninstall.impactLoading")}</div>;
  }
  const s = impact.size;
  const rows: [string, number][] = [
    [t("programs.install"), s.install],
    [t("programs.userData"), s.userData],
    [t("programs.cache"), s.cache],
    [t("programs.logs"), s.logs],
  ];
  const notes = [
    impact.running > 0 ? t("uninstall.impactRunning", { n: impact.running }) : "",
    impact.startupEntries > 0 ? t("uninstall.impactStartup", { n: impact.startupEntries }) : "",
    impact.shortcuts > 0 ? t("uninstall.impactShortcuts", { n: impact.shortcuts }) : "",
    impact.registryKeys > 0 ? t("uninstall.impactRegistry", { n: impact.registryKeys }) : "",
  ].filter(Boolean);
  return (
    <div className="card" style={{ padding: "0.7rem 0.9rem" }}>
      <div className="row" style={{ marginBottom: "0.4rem" }}>
        <strong className="grow">{t("uninstall.impactTitle")}</strong>
        <strong>{formatBytes(s.total)}</strong>
      </div>
      <div className="col" style={{ gap: "0.15rem", fontSize: "0.85rem" }}>
        {rows.filter(([, v]) => v > 0).map(([label, v]) => (
          <div key={label} className="row" style={{ gap: "0.5rem" }}>
            <span className="grow">{label}</span>
            <span>{formatBytes(v)}</span>
          </div>
        ))}
      </div>
      {impact.otherLocations.length > 0 && (
        <div className="faint" style={{ fontSize: "0.8rem", marginTop: "0.4rem" }}>
          {t("uninstall.impactKept", { n: impact.otherLocations.length })}
        </div>
      )}
      {notes.length > 0 && <div className="faint" style={{ fontSize: "0.8rem", marginTop: "0.3rem" }}>{notes.join(" · ")}</div>}
    </div>
  );
}

export function UninstallWizard() {
  const t = useT();
  const { uninstallRequest: req, closeUninstall, settings, toast } = useApp();
  const [step, setStep] = useState<Step>("loading");
  const [prep, setPrep] = useState<PreparedUninstall>();
  const [error, setError] = useState<ErrorPayload>();
  const [quiet, setQuiet] = useState(false);
  const [restorePoint, setRestorePoint] = useState(false);
  const [level, setLevel] = useState<LeftoverLevel>("moderate");
  const [procs, setProcs] = useState<string[]>([]);
  const [outcome, setOutcome] = useState<RunOutcome>();
  const [items, setItems] = useState<Leftover[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [recycle, setRecycle] = useState(true);
  const [dryRun, setDryRun] = useState(false);
  const [ack, setAck] = useState(false);
  const [summary, setSummary] = useState<RemovalSummary>();
  const [impact, setImpact] = useState<UninstallImpact>();

  // Start a session whenever a new request arrives.
  useEffect(() => {
    if (!req) return;
    setStep("loading");
    setError(undefined);
    setPrep(undefined);
    setOutcome(undefined);
    setItems([]);
    setSummary(undefined);
    setAck(false);
    setQuiet(false);
    setDryRun(settings.dryRunDefault);
    setRecycle(settings.useRecycleBin);
    setRestorePoint(settings.createRestorePoint);
    setLevel(settings.defaultLeftoverLevel);
    setImpact(undefined);
    api
      .uninstallPrepare(req.programId)
      .then((p) => {
        setPrep(p);
        setStep("confirm");
        // Measuring can take a moment on a big program: the dialog is usable
        // while it runs, and it only ever reads.
        api.uninstallImpact(p.sessionId).then(setImpact).catch(() => {});
      })
      .catch((e) => {
        setError(toError(e));
        setStep("confirm");
      });
  }, [req]); // eslint-disable-line react-hooks/exhaustive-deps

  const scan = async (lvl: LeftoverLevel) => {
    if (!prep) return;
    setStep("scanning");
    setError(undefined);
    try {
      const found = await api.leftoversScan(prep.sessionId, lvl);
      setItems(found);
      setSelected(new Set(found.filter((l) => l.preselected).map((l) => String(l.id))));
      setStep("leftovers");
    } catch (e) {
      setError(toError(e));
      setStep("leftovers");
    }
  };

  const runUninstaller = async () => {
    if (!prep) return;
    setError(undefined);
    setStep("running");
    setProcs([]);
    try {
      await api.uninstallRun(prep.sessionId, quiet, (e) => {
        if (e.event === "waiting") setProcs(e.data.processes);
        else if (e.event === "done") {
          setOutcome(e.data.outcome);
          if (e.data.outcome.stillInstalled) setStep("stillInstalled");
          else void scan(level);
        } else {
          setError(e.data);
          setStep("confirm");
        }
      });
    } catch (e) {
      setError(toError(e));
      setStep("confirm");
    }
  };

  const start = async () => {
    if (!prep) return;
    if (restorePoint) {
      setStep("restorePoint");
      try {
        await api.uninstallRestorePoint(prep.sessionId);
      } catch (e) {
        setError(toError(e));
        return; // stays on the restorePoint step: continue-without or cancel
      }
    }
    await runUninstaller();
  };

  const remove = async () => {
    if (!prep) return;
    setStep("removing");
    setError(undefined);
    try {
      const s = await api.leftoversRemove(prep.sessionId, [...selected].map(Number), recycle, ack, dryRun);
      setSummary(s);
      setStep("done");
      const n = s.results.filter((r) => r.status === "removed").length;
      if (n) toast("success", t("uninstall.removed", { n }));
    } catch (e) {
      setError(toError(e));
      setStep("leftovers");
    }
  };

  const close = () => {
    if (step === "running" || step === "removing" || step === "restorePoint" && !error) return;
    if (prep) void api.uninstallFinish(prep.sessionId).catch(() => {});
    closeUninstall();
    void usePrograms.getState().load(true);
  };

  const selectedItems = useMemo(() => items.filter((l) => selected.has(String(l.id))), [items, selected]);
  const selectedSize = selectedItems.reduce((s, l) => s + l.size, 0);
  const dangerousSelected = selectedItems.some((l) => l.risk === "dangerous");

  if (!req) return null;
  const p = prep?.program;
  const busy = step === "running" || step === "removing" || step === "scanning" || step === "loading" || (step === "restorePoint" && !error);

  const toggle = (id: string) => {
    const s = new Set(selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    setSelected(s);
  };

  let body: React.ReactNode = null;
  let footer: React.ReactNode = null;

  if (step === "loading") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t("common.loading")}</div>;
  } else if (step === "confirm" && prep && p) {
    // The exact command that will run (quiet variant when chosen).
    const cmd = quiet && prep.quietCommand ? prep.quietCommand : prep.command;
    body = (
      <>
        <div className="muted">{t("uninstall.intro")}</div>
        {cmd ? (
          <div className="card" style={{ padding: "0.7rem 0.9rem" }}>
            <div className="faint" style={{ fontSize: "0.82rem", marginBottom: "0.3rem" }}>
              {t("uninstall.command")} · {cmd.kind === "msi" ? t("uninstall.commandMsi") : cmd.kind === "appx" ? t("uninstall.commandAppx") : cmd.file.split("\\").pop()}
            </div>
            <div className="mono selectable" style={{ fontSize: "0.82rem", wordBreak: "break-all" }}>
              {cmd.kind === "appx" ? cmd.params : `"${cmd.file}" ${cmd.params}`}
            </div>
          </div>
        ) : (
          <>
            {prep.commandError && (
              <div className="banner warning col" style={{ alignItems: "stretch" }}>
                <strong>{t("uninstall.cannotRun")}</strong>
                <span className="selectable">{prep.commandError.message}</span>
              </div>
            )}
            {prep.stillInstalled ? (
              <div>
                <button className="btn" onClick={() => {
                  const pr = prep.program;
                  close();
                  useApp.getState().requestForced({ name: pr.name, folder: pr.installLocation ?? pr.inferredLocation ?? undefined, programId: pr.id });
                }}>{t("uninstall.forcedSelected")}</button>
              </div>
            ) : (
              <div className="faint">{t("uninstall.notInstalledAnymore")}</div>
            )}
          </>
        )}
        <ImpactCard impact={impact} />
        {prep.hasQuiet && (
          <label className="checkbox">
            <input type="checkbox" checked={quiet} onChange={(e) => setQuiet(e.target.checked)} />

            <span>{t("uninstall.quiet")}</span>
          </label>
        )}
        {cmd && (
          <label className="checkbox">
            <input type="checkbox" checked={restorePoint} onChange={(e) => setRestorePoint(e.target.checked)} />
            <span>{t("uninstall.restorePoint")}</span>
          </label>
        )}
        <div className="row wrap" style={{ gap: "0.6rem" }}>
          <span className="muted">{t("uninstall.level")}</span>
          <div className="seg">
            {(["safe", "moderate", "advanced"] as LeftoverLevel[]).map((l) => (
              <button key={l} className={level === l ? "on" : ""} onClick={() => setLevel(l)}>{t(`uninstall.level_${l}`)}</button>
            ))}
          </div>
          <span className="faint" style={{ fontSize: "0.85rem" }}>{t(`uninstall.levelHint_${level}`)}</span>
        </div>
        {error && <ErrorView error={error} />}
      </>
    );
    footer = (
      <>
        <button className="btn" onClick={close}>{t("common.cancel")}</button>
        {cmd ? (
          <button className="btn danger" onClick={start}><Trash2 size={14} />{t("uninstall.start")}</button>
        ) : (
          !prep.stillInstalled && <button className="btn primary" onClick={() => scan(level)}>{t("uninstall.scanOnly")}</button>
        )}
      </>
    );
  } else if (step === "confirm" && error) {
    // Preparation itself failed: show why instead of an empty dialog.
    body = <ErrorView error={error} />;
    footer = <button className="btn" onClick={close}>{t("common.close")}</button>;
  } else if (step === "restorePoint") {
    body = error ? (
      <>
        <div className="banner warning"><AlertTriangle size={16} color="var(--warning)" /><strong>{t("uninstall.restorePointFailed")}</strong></div>
        <ErrorView error={error} />
      </>
    ) : (
      <div className="muted row"><Loader2 size={16} className="spin" />{t("uninstall.creatingRestorePoint")}</div>
    );
    footer = error && (
      <>
        <button className="btn" onClick={close}>{t("common.cancel")}</button>
        <button className="btn danger" onClick={runUninstaller}>{t("uninstall.continueWithout")}</button>
      </>
    );
  } else if (step === "running") {
    body = (
      <>
        <div className="row"><Loader2 size={18} className="spin" /><strong>{t("uninstall.running")}</strong></div>
        <div className="muted">{t("uninstall.runningHint")}</div>
        {procs.length > 0 && <div className="mono faint">{t("uninstall.processes", { list: procs.join(", ") })}</div>}
        <div className="progress-track indeterminate"><div /></div>
      </>
    );
    footer = <button className="btn" onClick={() => prep && api.uninstallStopWaiting(prep.sessionId)}>{t("uninstall.stopWaiting")}</button>;
  } else if (step === "stillInstalled") {
    body = (
      <>
        <div className="banner warning col" style={{ alignItems: "stretch" }}>
          <strong>{t("uninstall.stillInstalled")}</strong>
          <span>{t("uninstall.stillInstalledHint")}</span>
        </div>
        {outcome?.exitCode != null && <div className="faint">{t("uninstall.exitCode", { code: outcome.exitCode })} · {formatDuration(outcome.durationMs)}</div>}
      </>
    );
    footer = <button className="btn primary" onClick={close}>{t("uninstall.close")}</button>;
  } else if (step === "scanning") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t("uninstall.scanning")}</div>;
  } else if (step === "leftovers") {
    body = (
      <>
        {outcome && <div className="banner success"><CheckCircle2 size={16} color="var(--success)" /><span>{p?.name} · {t("uninstall.exitCode", { code: outcome.exitCode ?? "—" })} · {formatDuration(outcome.durationMs)}</span></div>}
        <div className="row wrap" style={{ gap: "0.6rem" }}>
          <strong className="grow">{t("uninstall.leftoversTitle")}</strong>
          <div className="seg">
            {(["safe", "moderate", "advanced"] as LeftoverLevel[]).map((l) => (
              <button key={l} className={level === l ? "on" : ""} onClick={() => { setLevel(l); void scan(l); }}>{t(`uninstall.level_${l}`)}</button>
            ))}
          </div>
          <button className="btn ghost sm" onClick={() => setSelected(new Set(items.filter((l) => !l.shared).map((l) => String(l.id))))}>{t("uninstall.selectAll")}</button>
          <button className="btn ghost sm" onClick={() => setSelected(new Set())}>{t("uninstall.selectNone")}</button>
        </div>
        {error && <ErrorView error={error} />}
        {items.length === 0 && !error && <div className="card muted">{t("uninstall.noLeftovers")}</div>}
        <div className="plan-list" style={{ maxHeight: "22rem" }}>
          <LeftoverList items={items} selected={selected} onToggle={toggle} />
        </div>
        {items.length > 0 && (
          <>
            <strong>{t("uninstall.selectedSummary", { n: selected.size, size: formatBytes(selectedSize) })}</strong>
            <label className="checkbox"><input type="checkbox" checked={recycle} onChange={(e) => setRecycle(e.target.checked)} /><span>{t("uninstall.recycle")}</span></label>
            <label className="checkbox"><input type="checkbox" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} /><span>{t("uninstall.dryRun")}</span></label>
            {dangerousSelected && (
              <div className="banner warning">
                <label className="checkbox"><input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} /><span>{t("uninstall.dangerousAck")}</span></label>
              </div>
            )}
          </>
        )}
      </>
    );
    footer = (
      <>
        <button className="btn" onClick={close}>{t("uninstall.skip")}</button>
        <button className={`btn ${dryRun ? "primary" : "danger"}`} disabled={!selected.size || (dangerousSelected && !ack && !dryRun)} onClick={remove}>
          {dryRun ? t("uninstall.simulate") : t("uninstall.remove")}
        </button>
      </>
    );
  } else if (step === "removing") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t("uninstall.removing")}</div>;
  } else if (step === "done" && summary) {
    body = (
      <>
        <RemovalCounts results={summary.results} />
        {summary.elevatedCount > 0 && <div className="faint">{t("uninstall.elevated", { n: summary.elevatedCount })}</div>}
        <div className="plan-list">
          <RemovalResultList results={summary.results} />
        </div>
        {!dryRun && (
          <div className="row">
            <span className="muted">{t("uninstall.backup")}:</span>
            <span className="mono faint ellipsis grow" title={summary.backupDir}>{summary.backupDir}</span>
            <button className="btn sm" onClick={() => api.openPath(summary.backupDir).catch(() => {})}><FolderOpen size={13} />{t("uninstall.openBackup")}</button>
          </div>
        )}
      </>
    );
    footer = <button className="btn primary" onClick={close} data-autofocus>{t("uninstall.close")}</button>;
  }

  return (
    <Modal
      title={p ? t("uninstall.title", { name: p.name }) : t("uninstall.title", { name: "…" })}
      icon={p ? <ProgramIcon id={p.id} size={22} /> : undefined}
      onClose={close}
      wide
      locked={busy}
      footer={footer}
    >
      {p && step === "confirm" && (
        <div className="faint" style={{ marginTop: "-0.4rem" }}>{[p.version, p.publisher, p.installLocation].filter(Boolean).join(" · ")}</div>
      )}
      {body}
    </Modal>
  );
}
