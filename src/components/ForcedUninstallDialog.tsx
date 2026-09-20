import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, FolderOpen, Loader2, Search, ShieldAlert, Square, Wrench } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, ForcedChoice, ForcedTarget, Leftover, LeftoverLevel, ProcInfo, RemovalSummary } from "../types";
import { formatBytes } from "../utils/format";
import { ErrorView, errorTitle } from "./ErrorView";
import { LeftoverList, RemovalCounts, RemovalResultList } from "./leftovers";
import { Modal } from "./Modal";

type Step = "input" | "review" | "running" | "scanning" | "leftovers" | "removing" | "done";

/**
 * Forced uninstall: the user describes the program (name / exe / folder);
 * matches, processes and leftovers are shown; nothing is removed unreviewed.
 */
export function ForcedUninstallDialog() {
  const t = useT();
  const { forcedRequest: req, closeForced, settings, toast } = useApp();
  const [step, setStep] = useState<Step>("input");
  const [name, setName] = useState("");
  const [exe, setExe] = useState("");
  const [folder, setFolder] = useState("");
  const [error, setError] = useState<ErrorPayload>();
  const [sessionId, setSessionId] = useState<number>();
  const [target, setTarget] = useState<ForcedTarget>();
  const [choiceId, setChoiceId] = useState<string | null>(null);
  const [choice, setChoice] = useState<ForcedChoice>();
  const [procs, setProcs] = useState<ProcInfo[]>([]);
  const [runOfficial, setRunOfficial] = useState(true);
  const [level, setLevel] = useState<LeftoverLevel>("moderate");
  const [items, setItems] = useState<Leftover[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [recycle, setRecycle] = useState(true);
  const [dryRun, setDryRun] = useState(false);
  const [ack, setAck] = useState(false);
  const [summary, setSummary] = useState<RemovalSummary>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!req) return;
    setStep("input");
    setName(req.name ?? "");
    setExe(req.exe ?? "");
    setFolder(req.folder ?? "");
    setError(undefined);
    setSessionId(undefined);
    setTarget(undefined);
    setChoice(undefined);
    setItems([]);
    setSummary(undefined);
    setAck(false);
    setLevel(settings.defaultLeftoverLevel);
    setRecycle(settings.useRecycleBin);
    setDryRun(settings.dryRunDefault);
  }, [req]); // eslint-disable-line react-hooks/exhaustive-deps

  const choose = async (sid: number, id: string | null) => {
    setChoiceId(id);
    try {
      const c = await api.forcedChoose(sid, id);
      setChoice(c);
      setRunOfficial(!!c.command);
      setProcs(await api.sessionProcesses(sid));
    } catch (e) {
      setError(toError(e));
    }
  };

  const find = async () => {
    if (!name.trim() && !exe.trim() && !folder.trim()) {
      setError({ kind: "invalidInput", message: t("forced.needInput") });
      return;
    }
    setBusy(true);
    setError(undefined);
    try {
      if (sessionId !== undefined) void api.uninstallFinish(sessionId).catch(() => {});
      const r = await api.forcedResolve({ name: name.trim() || undefined, exe: exe.trim() || undefined, folder: folder.trim() || undefined });
      setSessionId(r.sessionId);
      setTarget(r.target);
      // Pre-pick the program the user came from, or a single folder match.
      const pre = req?.programId && r.target.matches.some((m) => m.program.id === req.programId)
        ? req.programId
        : r.target.matches.length === 1 && r.target.matches[0].reason !== "sameName" ? r.target.matches[0].program.id : null;
      await choose(r.sessionId, pre);
      setStep("review");
    } catch (e) {
      setError(toError(e));
    } finally {
      setBusy(false);
    }
  };

  const scan = async (lvl: LeftoverLevel) => {
    if (sessionId === undefined) return;
    setStep("scanning");
    setError(undefined);
    try {
      const found = await api.leftoversScan(sessionId, lvl);
      setItems(found);
      setSelected(new Set(found.filter((l) => l.preselected).map((l) => String(l.id))));
    } catch (e) {
      setError(toError(e));
    }
    setStep("leftovers");
  };

  const proceed = async () => {
    if (sessionId === undefined) return;
    if (runOfficial && choice?.command) {
      setStep("running");
      try {
        await api.uninstallRun(sessionId, false, (e) => {
          // Forced mode continues whatever the uninstaller did.
          if (e.event === "done") void scan(level);
          else if (e.event === "error") {
            setError(e.data);
            void scan(level);
          }
        });
      } catch (e) {
        setError(toError(e));
        void scan(level);
      }
    } else {
      await scan(level);
    }
  };

  const endProcesses = async () => {
    setBusy(true);
    for (const p of procs) {
      if (!p.path) continue;
      try {
        await api.processTerminate(p.pid, p.path);
      } catch (e) {
        setError(toError(e));
      }
    }
    if (sessionId !== undefined) setProcs(await api.sessionProcesses(sessionId).catch(() => []));
    setBusy(false);
  };

  const remove = async () => {
    if (sessionId === undefined) return;
    setStep("removing");
    try {
      const s = await api.leftoversRemove(sessionId, [...selected].map(Number), recycle, ack, dryRun);
      setSummary(s);
      const n = s.results.filter((r) => r.status === "removed").length;
      if (n) toast("success", t("uninstall.removed", { n }));
      setStep("done");
    } catch (e) {
      setError(toError(e));
      setStep("leftovers");
    }
  };

  const close = () => {
    if (step === "running" || step === "removing" || step === "scanning") return;
    if (sessionId !== undefined) void api.uninstallFinish(sessionId).catch(() => {});
    closeForced();
    void usePrograms.getState().load(true);
  };

  const selectedItems = useMemo(() => items.filter((l) => selected.has(String(l.id))), [items, selected]);
  const dangerousSelected = selectedItems.some((l) => l.risk === "dangerous");
  if (!req) return null;

  const browseExe = async () => {
    const f = await openDialog({ multiple: false, filters: [{ name: "Executable", extensions: ["exe"] }] });
    if (typeof f === "string") setExe(f);
  };
  const browseFolder = async () => {
    const f = await openDialog({ directory: true, multiple: false });
    if (typeof f === "string") setFolder(f);
  };

  let body: React.ReactNode = null;
  let footer: React.ReactNode = null;

  if (step === "input") {
    body = (
      <>
        <div className="muted">{t("forced.intro")}</div>
        <label className="col"><span className="muted">{t("forced.name")}</span>
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} autoFocus onKeyDown={(e) => e.key === "Enter" && find()} />
        </label>
        <label className="col"><span className="muted">{t("forced.exe")}</span>
          <div className="row"><input className="input grow mono" value={exe} onChange={(e) => setExe(e.target.value)} /><button className="btn" onClick={browseExe}>{t("forced.browse")}</button></div>
        </label>
        <label className="col"><span className="muted">{t("forced.folder")}</span>
          <div className="row"><input className="input grow mono" value={folder} onChange={(e) => setFolder(e.target.value)} /><button className="btn" onClick={browseFolder}>{t("forced.browse")}</button></div>
        </label>
        {error && <ErrorView error={error} />}
      </>
    );
    footer = (
      <>
        <button className="btn" onClick={close}>{t("common.cancel")}</button>
        <button className="btn primary" onClick={find} disabled={busy}>{busy ? <Loader2 size={14} className="spin" /> : <Search size={14} />}{t("forced.find")}</button>
      </>
    );
  } else if (step === "review" && target) {
    const v = target.version;
    body = (
      <>
        <div className="card" style={{ padding: "0.7rem 0.9rem" }}>
          <strong>{target.program.name}</strong>
          {target.program.installLocation && <div className="mono faint" style={{ fontSize: "0.82rem" }}>{target.program.installLocation}</div>}
          {v && (v.company || v.product) && (
            <div className="faint" style={{ fontSize: "0.82rem" }}>{t("forced.detected", { info: [v.product, v.company, v.version].filter(Boolean).join(" · ") })}</div>
          )}
        </div>
        <div className="section-title" style={{ margin: 0 }}>{t("forced.target")}</div>
        <div className="plan-list">
          {target.matches.map((m) => (
            <label key={m.program.id} className="plan-item" style={{ cursor: "pointer" }}>
              <input type="radio" name="forced-target" checked={choiceId === m.program.id} onChange={() => sessionId !== undefined && choose(sessionId, m.program.id)} />
              <span className="grow">{m.program.name} <span className="faint">{m.program.version ?? ""}</span></span>
              <span className="badge review">{t(`forced.match_${m.reason}`)}</span>
            </label>
          ))}
          <label className="plan-item" style={{ cursor: "pointer" }}>
            <input type="radio" name="forced-target" checked={choiceId === null} onChange={() => sessionId !== undefined && choose(sessionId, null)} />
            <span className="grow">{t("forced.targetUnregistered")}</span>
            {target.matches.length === 0 && <span className="badge neutral">{t("forced.noMatch")}</span>}
          </label>
        </div>
        {choice?.stillInstalled && <div className="banner warning"><AlertTriangle size={16} color="var(--warning)" /><span>{t("forced.stillRegistered")}</span></div>}
        {choiceId !== null && (choice?.command ? (
          <label className="checkbox"><input type="checkbox" checked={runOfficial} onChange={(e) => setRunOfficial(e.target.checked)} /><span>{t("forced.runOfficial")}</span></label>
        ) : choice?.commandError && (
          <div className="faint" title={choice.commandError.message}>
            {t("forced.officialMissing", { reason: errorTitle(choice.commandError, t) + (choice.commandError.path ? ` — ${choice.commandError.path}` : "") })}
          </div>
        ))}
        <div className="section-title" style={{ margin: 0 }}>{t("forced.processes")}</div>
        {procs.length === 0 ? (
          <div className="faint">{t("forced.noProcesses")}</div>
        ) : (
          <div className="banner warning col" style={{ alignItems: "stretch" }}>
            <div className="mono" style={{ fontSize: "0.82rem" }}>{procs.map((p) => `${p.name} (PID ${p.pid})`).join(", ")}</div>
            <span>{t("forced.processesHint")}</span>
            <div><button className="btn sm danger" onClick={endProcesses} disabled={busy}><Square size={12} />{t("forced.endProcesses")}</button></div>
          </div>
        )}
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
        <button className="btn" onClick={() => setStep("input")}>{t("common.back")}</button>
        <button className="btn primary" onClick={proceed} disabled={busy}>{t("forced.scan")}</button>
      </>
    );
  } else if (step === "running") {
    body = (
      <>
        <div className="row"><Loader2 size={18} className="spin" /><strong>{t("uninstall.running")}</strong></div>
        <div className="muted">{t("uninstall.runningHint")}</div>
        <div className="progress-track indeterminate"><div /></div>
      </>
    );
    footer = <button className="btn" onClick={() => sessionId !== undefined && api.uninstallStopWaiting(sessionId)}>{t("uninstall.stopWaiting")}</button>;
  } else if (step === "scanning" || step === "removing") {
    body = <div className="muted row"><Loader2 size={16} className="spin" />{t(step === "scanning" ? "uninstall.scanning" : "uninstall.removing")}</div>;
  } else if (step === "leftovers") {
    const size = selectedItems.reduce((s, l) => s + l.size, 0);
    body = (
      <>
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
        {items.length === 0 && <div className="card muted">{t("uninstall.noLeftovers")}</div>}
        <div className="plan-list" style={{ maxHeight: "22rem" }}>
          <LeftoverList items={items} selected={selected} onToggle={(k) => {
            const s = new Set(selected);
            if (s.has(k)) s.delete(k);
            else s.add(k);
            setSelected(s);
          }} />
        </div>
        {items.length > 0 && (
          <>
            <strong>{t("uninstall.selectedSummary", { n: selected.size, size: formatBytes(size) })}</strong>
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
        <button className="btn" onClick={close}>{t("uninstall.skip")}</button>
        <button className={`btn ${dryRun ? "primary" : "danger"}`} disabled={!selected.size || (dangerousSelected && !ack && !dryRun)} onClick={remove}>
          {dryRun ? t("uninstall.simulate") : t("uninstall.remove")}
        </button>
      </>
    );
  } else if (step === "done" && summary) {
    body = (
      <>
        <RemovalCounts results={summary.results} />
        {summary.elevatedCount > 0 && <div className="faint">{t("uninstall.elevated", { n: summary.elevatedCount })}</div>}
        <div className="plan-list"><RemovalResultList results={summary.results} /></div>
        {!dryRun && (
          <div className="row">
            <span className="muted">{t("uninstall.backup")}:</span>
            <span className="mono faint ellipsis grow" title={summary.backupDir}>{summary.backupDir}</span>
            <button className="btn sm" onClick={() => api.openPath(summary.backupDir).catch(() => {})}><FolderOpen size={13} />{t("uninstall.openBackup")}</button>
          </div>
        )}
      </>
    );
    footer = <button className="btn primary" onClick={close}>{t("uninstall.close")}</button>;
  }

  return (
    <Modal title={t("forced.title")} icon={<Wrench size={18} />} onClose={close} wide locked={step === "running" || step === "removing" || step === "scanning"} footer={footer}>
      {body}
    </Modal>
  );
}
