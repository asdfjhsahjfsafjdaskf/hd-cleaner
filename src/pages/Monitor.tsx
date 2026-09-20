import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Boxes, Download, FileDown, Play, Radar, Square, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, InstallTrace, MonitorStatus } from "../types";
import { formatBytes, formatDate, formatNumber } from "../utils/format";

/** The items a trace recorded, in blocks that stay readable. */
function TraceItems({ trace }: { trace: InstallTrace }) {
  const t = useT();
  const [limit, setLimit] = useState(200);
  // What carries the program's name is almost certainly its own; the rest
  // changed in the same window but probably belongs to other programs.
  const kept = trace.files.filter((f) => !f.transient && f.related);
  const others = trace.files.filter((f) => !f.transient && !f.related);
  const temporary = trace.files.filter((f) => f.transient);
  const ownRegistry = trace.registry.filter((r) => r.related);
  const otherRegistry = trace.registry.filter((r) => !r.related);
  const folders = [...new Set(kept.map((f) => f.path.slice(0, f.path.lastIndexOf("\\"))))].slice(0, 8);

  const Block = ({ title, rows, extra }: { title: string; rows: string[]; extra?: (r: string) => string }) =>
    rows.length === 0 ? null : (
      <>
        <div className="section-title">{title} <span className="faint">({formatNumber(rows.length)})</span></div>
        <div className="col" style={{ gap: "0.15rem" }}>
          {rows.slice(0, limit).map((r) => (
            <div key={r} className="row" style={{ gap: "0.5rem", fontSize: "0.8rem" }}>
              <span className="mono ellipsis grow" title={r}>{r}</span>
              {extra && <span className="faint">{extra(r)}</span>}
            </div>
          ))}
          {rows.length > limit && (
            <button className="btn ghost sm" style={{ alignSelf: "flex-start" }} onClick={() => setLimit((l) => l + 500)}>
              {t("cleaner.loadMore")} ({formatNumber(rows.length - limit)})
            </button>
          )}
        </div>
      </>
    );

  const sizeOf = (p: string) => {
    const f = kept.find((x) => x.path === p);
    return f && f.size > 0 ? formatBytes(f.size) : "";
  };

  return (
    <>
      {folders.length > 0 && (
        <>
          <div className="section-title">{t("monitor.mainFolders")}</div>
          <div className="col mono" style={{ gap: "0.15rem", fontSize: "0.82rem" }}>
            {folders.map((f) => <span key={f} className="ellipsis" title={f}>{f}</span>)}
          </div>
        </>
      )}
      <Block title={t("monitor.filesKept")} rows={kept.map((f) => f.path)} extra={sizeOf} />
      <Block title={t("monitor.registry")} rows={ownRegistry.map((r) => r.path)} />
      <Block title={t("monitor.services")} rows={trace.services} />
      <Block title={t("monitor.tasks")} rows={trace.tasks} />
      <Block title={t("monitor.filesTemporary")} rows={temporary.map((f) => f.path)} />
      {(others.length > 0 || otherRegistry.length > 0) && (
        <div className="faint" style={{ fontSize: "0.82rem", marginTop: "0.6rem" }}>{t("monitor.othersHint")}</div>
      )}
      <Block title={t("monitor.othersFiles")} rows={others.map((f) => f.path)} />
      <Block title={t("monitor.othersRegistry")} rows={otherRegistry.map((r) => r.path)} />
    </>
  );
}

function TraceDetails({ id, onBack, onDeleted }: { id: number; onBack: () => void; onDeleted: () => void }) {
  const t = useT();
  const app = useApp();
  const [trace, setTrace] = useState<InstallTrace>();
  const [error, setError] = useState<ErrorPayload>();
  useEffect(() => {
    api.traceGet(id).then(setTrace).catch((e) => setError(toError(e)));
  }, [id]);

  if (error) return <ErrorView error={error} />;
  if (!trace) return <div className="muted">{t("common.loading")}</div>;

  const exportTrace = async () => {
    const path = await saveDialog({ filters: [{ name: "JSON", extensions: ["json"] }], defaultPath: `${trace.name.replace(/[^\w.-]+/g, "-")}.json` });
    if (!path) return;
    try {
      await api.traceExport(trace.id, path);
      app.toast("success", t("monitor.exported", { path }));
    } catch (e) {
      app.toastError(e);
    }
  };
  const remove = async () => {
    try {
      await api.traceDelete(trace.id);
      onDeleted();
    } catch (e) {
      app.toastError(e);
    }
  };

  return (
    <div className="col" style={{ gap: "0.7rem" }}>
      <div className="row">
        <button className="btn sm" onClick={onBack}><ArrowLeft size={14} />{t("common.back")}</button>
        <strong className="grow ellipsis">{trace.name}</strong>
        {trace.programId && (
          <button className="btn sm danger" onClick={() => app.requestUninstall(trace.programId!)}><Trash2 size={14} />{t("monitor.uninstallProgram")}</button>
        )}
        {trace.programId && (
          <button className="btn sm" onClick={() => { usePrograms.getState().select(trace.programId!); app.navigate("programs"); }}>
            <Boxes size={14} />{t("processes.viewProgram")}
          </button>
        )}
        <button className="btn sm" onClick={() => void exportTrace()}><FileDown size={14} />{t("monitor.export")}</button>
        <button className="btn sm danger" onClick={() => void remove()}><Trash2 size={14} />{t("common.delete")}</button>
      </div>
      <dl className="kv">
        {trace.programName && (<><dt>{t("monitor.program")}</dt><dd>{trace.programName}</dd></>)}
        <dt>{t("common.date")}</dt><dd>{formatDate(trace.startedMs)}</dd>
        <dt>{t("monitor.duration")}</dt><dd>{Math.max(1, Math.round((trace.finishedMs - trace.startedMs) / 1000))} s</dd>
        <dt>{t("monitor.summary")}</dt>
        <dd>
          {t("monitor.counts", {
            files: formatNumber(trace.files.filter((f) => !f.transient && f.related).length),
            size: formatBytes(trace.bytes),
            registry: formatNumber(trace.registry.filter((r) => r.related).length),
          })}
        </dd>
      </dl>
      <TraceItems trace={trace} />
    </div>
  );
}

export function Monitor() {
  const t = useT();
  const app = useApp();
  const [status, setStatus] = useState<MonitorStatus>();
  const [traces, setTraces] = useState<InstallTrace[]>([]);
  const [selected, setSelected] = useState<number>();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorPayload>();

  const refresh = useCallback(async () => {
    try {
      const [s, list] = await Promise.all([api.monitorStatus(), api.tracesList()]);
      setStatus(s);
      setTraces(list);
    } catch (e) {
      setError(toError(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // While monitoring, show the live count of changes seen.
  useEffect(() => {
    if (!status?.running) return;
    const id = window.setInterval(() => void api.monitorStatus().then(setStatus).catch(() => {}), 2000);
    return () => window.clearInterval(id);
  }, [status?.running]);

  const start = async () => {
    setBusy(true);
    setError(undefined);
    try {
      setStatus(await api.monitorStart());
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  };

  const finish = async () => {
    setBusy(true);
    try {
      const trace = await api.monitorFinish(name.trim() || t("monitor.defaultName"));
      setName("");
      await refresh();
      setSelected(trace.id);
      app.toast("success", t("monitor.saved", { files: formatNumber(trace.files.length) }));
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const cancel = async () => {
    setBusy(true);
    try {
      setStatus(await api.monitorCancel());
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const importTrace = async () => {
    const path = await openDialog({ filters: [{ name: "JSON", extensions: ["json"] }], multiple: false });
    if (typeof path !== "string") return;
    try {
      const trace = await api.traceImport(path, t("monitor.importedSuffix"));
      await refresh();
      setSelected(trace.id);
    } catch (e) {
      app.toastError(e);
    }
  };

  return (
    <div className="page fill">
      <div className="toolbar">
        <Radar size={16} className="muted" />
        <strong>{t("monitor.title")}</strong>
        <span className="grow" />
        {status?.running ? (
          <>
            <input
              className="input"
              style={{ width: "16rem" }}
              placeholder={t("monitor.namePlaceholder")}
              value={name}
              onChange={(e) => setName(e.target.value)}
              aria-label={t("monitor.namePlaceholder")}
            />
            <button className="btn primary" disabled={busy} onClick={() => void finish()}><Square size={14} />{t("monitor.finish")}</button>
            <button className="btn" disabled={busy} onClick={() => void cancel()}>{t("common.cancel")}</button>
          </>
        ) : (
          <>
            <button className="btn" onClick={() => void importTrace()}><Download size={14} />{t("monitor.import")}</button>
            <button className="btn primary" disabled={busy} onClick={() => void start()}>
              <Play size={14} />{busy ? t("monitor.starting") : t("monitor.start")}
            </button>
          </>
        )}
      </div>

      <div className="page-body">
        {error && <ErrorView error={error} onRetry={() => void refresh()} />}

        {status?.running ? (
          <div className="card" style={{ padding: "0.9rem", marginBottom: "1rem" }}>
            <div className="row" style={{ marginBottom: "0.4rem" }}>
              <Radar size={16} color="var(--accent)" />
              <strong className="grow">{t("monitor.watching", { n: formatNumber(status.seen) })}</strong>
              <span className="faint">{t("monitor.since", { time: formatDate(status.startedMs) })}</span>
            </div>
            <p style={{ margin: "0 0 0.4rem" }}>{t("monitor.watchingHint")}</p>
            <div className="faint mono" style={{ fontSize: "0.78rem" }}>{status.roots.join("  ·  ")}</div>
          </div>
        ) : (
          !selected && (
            <div className="card" style={{ padding: "0.9rem", marginBottom: "1rem" }}>
              <strong>{t("monitor.howTitle")}</strong>
              <ol style={{ margin: "0.4rem 0 0", paddingLeft: "1.1rem" }}>
                <li>{t("monitor.step1")}</li>
                <li>{t("monitor.step2")}</li>
                <li>{t("monitor.step3")}</li>
              </ol>
              <p className="faint" style={{ margin: "0.5rem 0 0" }}>{t("monitor.readOnly")}</p>
            </div>
          )
        )}

        {selected ? (
          <TraceDetails id={selected} onBack={() => setSelected(undefined)} onDeleted={() => { setSelected(undefined); void refresh(); }} />
        ) : (
          <>
            <div className="section-title" style={{ marginTop: 0 }}>{t("monitor.savedTitle")}</div>
            {traces.length === 0 ? (
              <div className="muted">{t("monitor.none")}</div>
            ) : (
              <div className="col" style={{ gap: "0.35rem" }}>
                {traces.map((tr) => (
                  <button key={tr.id} className="card row" style={{ padding: "0.55rem 0.8rem", gap: "0.6rem", textAlign: "left", cursor: "pointer" }} onClick={() => setSelected(tr.id)}>
                    <Radar size={15} className="muted" />
                    <div className="col grow" style={{ gap: 0, minWidth: 0 }}>
                      <strong className="ellipsis">{tr.name}</strong>
                      <span className="faint" style={{ fontSize: "0.82rem" }}>
                        {[tr.programName, formatDate(tr.startedMs)].filter(Boolean).join(" · ")}
                      </span>
                    </div>
                    <strong>{formatBytes(tr.bytes)}</strong>
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
