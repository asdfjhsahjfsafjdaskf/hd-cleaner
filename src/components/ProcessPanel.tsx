import { Boxes, FileCog, FolderOpen, Power, Square, Trash2, Workflow } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, ProcessDetails } from "../types";
import { formatBytes, formatDate, formatDuration } from "../utils/format";
import { ErrorView } from "./ErrorView";
import { FileIcon } from "./ProgramIcon";
import { EndProcessDialog, endBlocked, startupSourceLabel, toggleStartup } from "./systemItems";

/** Details and actions for one process (Processes page and Target Mode). */
export function ProcessPanel({ pid, onChanged, extra }: { pid: number; onChanged?: () => void; extra?: (d: ProcessDetails) => ReactNode }) {
  const t = useT();
  const app = useApp();
  const [d, setD] = useState<ProcessDetails>();
  const [error, setError] = useState<ErrorPayload>();
  const [ending, setEnding] = useState<"one" | "tree">();

  const load = useCallback(async () => {
    try {
      setD(await api.processDetails(pid));
      setError(undefined);
    } catch (e) {
      setError(toError(e));
    }
  }, [pid]);
  useEffect(() => {
    setD(undefined);
    void load();
  }, [load]);

  if (error) return error.kind === "notFound" ? <div className="muted">{t("processes.gone")}</div> : <ErrorView error={error} />;
  if (!d) return <div className="muted">{t("common.loading")}</div>;
  const r = d.row;
  const blocked = endBlocked(r);
  const showProgram = (id: string) => {
    usePrograms.getState().select(id);
    app.navigate("programs");
  };

  return (
    <div className="col" style={{ gap: "0.8rem" }}>
      <div className="row" style={{ alignItems: "flex-start" }}>
        <FileIcon path={r.path} size={32} />
        <div className="col grow" style={{ gap: 0, minWidth: 0 }}>
          <strong style={{ fontSize: "1.05rem", wordBreak: "break-word" }}>{d.version?.description || r.name}</strong>
          <span className="muted">{[r.name, `PID ${r.pid}`, d.version?.company].filter(Boolean).join(" · ")}</span>
        </div>
      </div>
      <div className="row wrap" style={{ gap: "0.35rem" }}>
        <button className="btn sm danger" disabled={!!blocked} onClick={() => setEnding("one")}><Square size={14} />{t("processes.end")}</button>
        <button className="btn sm" disabled={!!blocked || (d.children === 0 && !!r.path)} onClick={() => setEnding("tree")}><Workflow size={14} />{t("processes.endTree")}</button>
        <button className="btn sm" disabled={!r.path} onClick={() => r.path && api.revealPath(r.path).catch(app.toastError)}><FolderOpen size={14} />{t("common.openFolder")}</button>
        <button className="btn sm" disabled={!r.path} onClick={() => r.path && api.showProperties(r.path).catch(app.toastError)}><FileCog size={14} />{t("common.properties")}</button>
        {extra?.(d)}
      </div>
      {blocked && <div className="faint" style={{ fontSize: "0.85rem" }}>{t(`processes.${blocked}`)}</div>}

      <div className="section-title" style={{ margin: 0 }}>{t("processes.program")}</div>
      {d.programs.length === 0 ? (
        <div className="muted" style={{ fontSize: "0.9rem" }}>{t("processes.noProgram")}</div>
      ) : (
        d.programs.map((p) => (
          <div key={p.programId} className="card row" style={{ padding: "0.5rem 0.7rem", gap: "0.4rem" }}>
            <Boxes size={15} className="muted" />
            <strong className="grow ellipsis" title={p.location}>{p.name}</strong>
            <button className="btn sm" onClick={() => showProgram(p.programId)}>{t("processes.viewProgram")}</button>
            <button className="btn sm danger" onClick={() => app.requestUninstall(p.programId)}><Trash2 size={13} />{t("target.uninstall")}</button>
          </div>
        ))
      )}

      <div className="section-title" style={{ margin: 0 }}>{t("processes.startupEntries")}</div>
      {d.startup.length === 0 ? (
        <div className="muted" style={{ fontSize: "0.9rem" }}>{t("processes.noStartup")}</div>
      ) : (
        d.startup.map((s) => (
          <div key={s.id} className="card row" style={{ padding: "0.5rem 0.7rem", gap: "0.4rem" }}>
            <Power size={15} className="muted" />
            <div className="col grow" style={{ gap: 0, minWidth: 0 }}>
              <span className="ellipsis">{s.name}</span>
              <span className="faint" style={{ fontSize: "0.8rem" }}>{startupSourceLabel(s, t)} · {s.enabled ? t("startup.enabled") : t("startup.disabled")}</span>
            </div>
            {s.canDisable && (
              <button className="btn sm" onClick={async () => { if (await toggleStartup(s, !s.enabled)) void load(); }}>
                {s.enabled ? t("target.disableStartup") : t("startup.enable")}
              </button>
            )}
          </div>
        ))
      )}

      <div className="section-title" style={{ margin: 0 }}>{t("common.details")}</div>
      <dl className="kv">
        {r.path && (<><dt>{t("processes.file")}</dt><dd className="mono selectable" style={{ wordBreak: "break-all" }}>{r.path}</dd></>)}
        {d.version?.version && (<><dt>{t("processes.version")}</dt><dd>{d.version.version}</dd></>)}
        {r.user && (<><dt>{t("processes.col_user")}</dt><dd>{r.user}</dd></>)}
        {r.privateBytes != null && (<><dt>{t("processes.memPrivate")}</dt><dd>{formatBytes(r.privateBytes)}</dd></>)}
        {r.workingSet != null && (<><dt>{t("processes.memWorking")}</dt><dd>{formatBytes(r.workingSet)}</dd></>)}
        {r.cpuTimeMs != null && (<><dt>{t("processes.cpuTime")}</dt><dd>{formatDuration(r.cpuTimeMs)}</dd></>)}
        {r.startedMs ? (<><dt>{t("processes.started")}</dt><dd>{formatDate(r.startedMs)}</dd></>) : null}
        <dt>{t("processes.threads")}</dt><dd>{r.threads}</dd>
        <dt>{t("processes.parent")}</dt><dd>{r.parent}</dd>
        <dt>{t("processes.children")}</dt><dd>{d.children}</dd>
      </dl>
      {(!r.path || r.privateBytes == null) && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("processes.limited")}</div>}

      {ending && (
        <EndProcessDialog
          row={r}
          tree={ending === "tree"}
          children={d.children}
          onClose={() => setEnding(undefined)}
          onDone={() => {
            onChanged?.();
            void load();
          }}
        />
      )}
    </div>
  );
}
