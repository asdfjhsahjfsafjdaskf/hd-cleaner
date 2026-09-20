import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Copy, Play, Square, Trash2, Wand2 } from "lucide-react";
import { useEffect, useState } from "react";
import { create } from "zustand";
import { useContextMenu } from "../components/ContextMenu";
import { ErrorView } from "../components/ErrorView";
import { fileMenu, requestDelete } from "../components/fileActions";
import { NodeIcon } from "../components/ui";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import type { DupGroup, DupMode, ErrorPayload, ScanProgress } from "../types";
import { formatBytes, formatDate, formatNumber, parseSize } from "../utils/format";

const PAGE = 100;

interface DupState {
  scanId?: number;
  jobId?: number;
  progress?: ScanProgress;
  dupId?: number;
  summary?: { groups: number; totalWasted: number; bytesHashed: number; unreadable: number };
  groups: DupGroup[];
  selected: Set<number>;
  error?: ErrorPayload;
  set: (p: Partial<DupState>) => void;
}

const useDup = create<DupState>((set) => ({ groups: [], selected: new Set(), set: (p) => set(p) }));

export function Duplicates() {
  const t = useT();
  const app = useApp();
  const a = useAnalyzer();
  const d = useDup();
  const openMenu = useContextMenu((s) => s.open);
  const [mode, setMode] = useState<DupMode>("precise");
  const [minSize, setMinSize] = useState("1MB");

  // Results belong to one scan; drop them when another scan is loaded.
  useEffect(() => {
    if (d.scanId !== undefined && d.scanId !== a.scanId) {
      d.set({ scanId: undefined, dupId: undefined, groups: [], summary: undefined, selected: new Set() });
    }
  }, [a.scanId]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadMore = async (dupId: number, offset: number) => {
    const page = await api.dupGroups(dupId, offset, PAGE);
    const cur = useDup.getState();
    d.set({ groups: offset === 0 ? page.rows : [...cur.groups, ...page.rows] });
  };

  const start = async () => {
    if (a.scanId === undefined) return;
    const min = parseSize(minSize);
    d.set({ error: undefined, groups: [], summary: undefined, dupId: undefined, selected: new Set(), progress: undefined, scanId: a.scanId });
    try {
      const jobId = await api.dupStart(a.scanId, { mode, minSize: isNaN(min) ? 1024 * 1024 : min }, (e) => {
        if (e.event === "progress") d.set({ progress: e.data });
        else if (e.event === "done") {
          d.set({ jobId: undefined, dupId: e.data.dupId, summary: e.data });
          loadMore(e.data.dupId, 0).catch((err) => d.set({ error: toError(err) }));
        } else if (e.event === "cancelled") d.set({ jobId: undefined });
        else if (e.event === "error") d.set({ jobId: undefined, error: e.data });
      });
      if (!useDup.getState().summary) d.set({ jobId });
    } catch (e) {
      d.set({ error: toError(e) });
    }
  };

  const toggle = (id: number) => {
    const s = new Set(d.selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    d.set({ selected: s });
  };

  const autoSelect = async (rule: "keepOldest" | "keepNewest" | "keepShortestPath" | "keepInFolder") => {
    if (d.dupId === undefined) return;
    let folder: string | undefined;
    if (rule === "keepInFolder") {
      const f = await openDialog({ directory: true, multiple: false });
      if (typeof f !== "string") return;
      folder = f;
    }
    try {
      d.set({ selected: new Set(await api.dupAutoselect(d.dupId, rule, folder)) });
    } catch (e) {
      app.toastError(e);
    }
  };

  const fullySelected = d.groups.filter((g) => g.files.length > 0 && g.files.every((f) => d.selected.has(f.id)));
  const selectedSize = d.groups.flatMap((g) => g.files).filter((f) => d.selected.has(f.id)).reduce((s, f) => s + f.size, 0);

  const deleteSelected = () => {
    const files = d.groups.flatMap((g) => g.files).filter((f) => d.selected.has(f.id));
    requestDelete(
      files.map((f) => ({ scanId: d.scanId, node: f.id, path: f.path!, isDir: false })),
      false,
      async () => {
        d.set({ selected: new Set() });
        a.bump();
        if (d.dupId !== undefined) await loadMore(d.dupId, 0);
      },
    );
  };

  if (a.scanId === undefined) {
    return (
      <div className="page">
        <div className="page-header"><h1>{t("duplicates.title")}</h1></div>
        <div className="empty">
          <Copy size={36} strokeWidth={1.3} />
          <span>{t("largeFiles.needScan")}</span>
          <button className="btn primary" onClick={() => app.navigate("analyzer")}>{t("largeFiles.goAnalyze")}</button>
        </div>
      </div>
    );
  }

  const p = d.progress;
  return (
    <div className="page">
      <div className="page-header">
        <h1>{t("duplicates.title")}</h1>
        <span className="muted">{t("largeFiles.of", { root: a.meta?.rootPath ?? "" })}</span>
      </div>
      <div className="card row wrap" style={{ gap: "0.8rem", marginBottom: "0.9rem" }}>
        <span className="muted">{t("duplicates.mode")}</span>
        <select className="select" value={mode} onChange={(e) => setMode(e.target.value as DupMode)} disabled={!!d.jobId}>
          <option value="precise">{t("duplicates.mode_precise")}</option>
          <option value="quick">{t("duplicates.mode_quick")}</option>
          <option value="quickWithDate">{t("duplicates.mode_quickWithDate")}</option>
        </select>
        <span className="muted">{t("duplicates.minSize")}</span>
        <input className="input" style={{ width: "6.5rem" }} value={minSize} onChange={(e) => setMinSize(e.target.value)} disabled={!!d.jobId} />
        {d.jobId ? (
          <button className="btn" onClick={() => api.cancelJob(d.jobId!)}><Square size={14} />{t("common.stop")}</button>
        ) : (
          <button className="btn primary" onClick={start}><Play size={14} />{t("duplicates.start")}</button>
        )}
        <span className="faint grow" style={{ fontSize: "0.85rem", minWidth: "18rem" }}>{t("duplicates.precisionNote")}</span>
      </div>

      {d.error && <ErrorView error={d.error} />}

      {d.jobId && (
        <div className="card" style={{ marginBottom: "0.9rem" }}>
          <div className="muted" style={{ marginBottom: "0.5rem" }}>
            {p?.phase === "buildingTree"
              ? t("duplicates.runningFull", { bytes: formatBytes(p?.bytes ?? 0) })
              : t("duplicates.running", { done: formatNumber(p?.recordsDone ?? 0), total: formatNumber(p?.recordsTotal ?? 0) })}
          </div>
          <div className={`progress-track ${p && p.recordsTotal ? "" : "indeterminate"}`}>
            <div style={p && p.recordsTotal ? { width: `${(Math.min(p.recordsDone, p.recordsTotal) / p.recordsTotal) * 100}%` } : undefined} />
          </div>
        </div>
      )}

      {d.summary && (
        <>
          <div className="row wrap" style={{ marginBottom: "0.8rem", gap: "0.6rem" }}>
            <strong>{t("duplicates.groups", { n: formatNumber(d.summary.groups), size: formatBytes(d.summary.totalWasted) })}</strong>
            {d.summary.bytesHashed > 0 && <span className="faint">{t("duplicates.hashed", { size: formatBytes(d.summary.bytesHashed) })}</span>}
            {d.summary.unreadable > 0 && <span className="badge dangerous">{t("duplicates.unreadable", { n: d.summary.unreadable })}</span>}
            <span className="grow" />
            <button className="btn sm" disabled={!d.groups.length} onClick={(e) => openMenu(e.clientX, e.clientY, [
              { label: t("duplicates.keepOldest"), onClick: () => autoSelect("keepOldest") },
              { label: t("duplicates.keepNewest"), onClick: () => autoSelect("keepNewest") },
              { label: t("duplicates.keepShortest"), onClick: () => autoSelect("keepShortestPath") },
              { label: t("duplicates.keepInFolder"), onClick: () => autoSelect("keepInFolder") },
            ])}><Wand2 size={14} />{t("duplicates.autoSelect")}</button>
            <button className="btn sm ghost" disabled={!d.selected.size} onClick={() => d.set({ selected: new Set() })}>{t("common.clearSelection")}</button>
            <button className="btn sm danger" disabled={!d.selected.size || fullySelected.length > 0} onClick={deleteSelected}>
              <Trash2 size={14} />
              {t("duplicates.deleteSelected", { n: d.selected.size })} · {formatBytes(selectedSize)}
            </button>
          </div>
          <div className="faint" style={{ fontSize: "0.85rem", marginBottom: "0.6rem" }}>{t("duplicates.autoNote")}</div>
          {fullySelected.length > 0 && (
            <div className="banner warning" style={{ marginBottom: "0.7rem" }}>
              <AlertTriangle size={16} color="var(--warning)" />
              <span>{t("duplicates.allCopiesSelected", { n: fullySelected.length })}</span>
            </div>
          )}
          {d.groups.length === 0 && <div className="empty">{t("duplicates.none")}</div>}
          {d.groups.map((g) => (
            <div key={g.id} className="dup-group">
              <div className="dup-head">
                <strong>{formatBytes(g.size)}</strong>
                <span className="muted">{t("duplicates.copies", { n: g.files.length })}</span>
                <span className="badge dangerous">{t("duplicates.wasted", { size: formatBytes(g.wasted) })}</span>
                {g.hash && <span className="faint mono" title={`BLAKE3 ${g.hash}`}>{g.hash.slice(0, 12)}…</span>}
              </div>
              {g.files.map((f, i) => (
                <div key={f.id} className="dup-file" onContextMenu={(e) => {
                  e.preventDefault();
                  openMenu(e.clientX, e.clientY, fileMenu({ scanId: d.scanId, node: f.id, path: f.path!, isDir: false }, { onChanged: () => a.bump() }));
                }}>
                  <input type="checkbox" checked={d.selected.has(f.id)} onChange={() => toggle(f.id)} aria-label={f.path} style={{ accentColor: "var(--accent)" }} />
                  <NodeIcon row={f} />
                  <span className="grow ellipsis mono" title={f.path}>{f.path}</span>
                  {i === 0 && <span className="badge neutral">{t("duplicates.oldest")}</span>}
                  <span className="faint">{formatDate(f.modified)}</span>
                </div>
              ))}
            </div>
          ))}
          {d.dupId !== undefined && d.groups.length < d.summary.groups && (
            <button className="btn" onClick={() => loadMore(d.dupId!, d.groups.length)}>+ {Math.min(PAGE, d.summary.groups - d.groups.length)}</button>
          )}
        </>
      )}
    </div>
  );
}
