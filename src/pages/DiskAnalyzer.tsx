import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronDown, ChevronRight, ChevronsDownUp, ChevronsUpDown, Download, FolderOpen, HardDrive, Play, RefreshCw, Search, Square,
  Upload, X, Zap, ArrowUpLeft, ShieldAlert,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as RKeyboardEvent } from "react";
import { useContextMenu } from "../components/ContextMenu";
import { DetailsPanel } from "../components/DetailsPanel";
import { ErrorView } from "../components/ErrorView";
import { copyText, fileMenu, requestDelete, type FileTarget } from "../components/fileActions";
import { Treemap, TreemapLegend } from "../components/Treemap";
import { NodeIcon, ShareBar } from "../components/ui";
import { VirtualTable, type Column, type VirtualTableHandle } from "../components/VirtualTable";
import { useT } from "../i18n";
import { api } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import type { NodeRow, SortKey } from "../types";
import { formatAttributes, formatBytes, formatCompactDate, formatDate, formatDuration, formatNumber, formatPercent } from "../utils/format";

const QUICK_FILTERS: [string, string][] = [
  ["qf_over100mb", "size:>100MB type:file"],
  ["qf_over1gb", "size:>1GB type:file"],
  ["qf_today", "modified:today type:file"],
  ["qf_week", "modified:week type:file"],
  ["qf_videos", "type:video"],
  ["qf_images", "type:image"],
  ["qf_archives", "type:archive"],
  ["qf_installers", "type:installer"],
  ["qf_executables", "type:executable"],
];

function isTyping(e: KeyboardEvent) {
  const el = e.target as HTMLElement;
  return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable;
}

export function DiskAnalyzer() {
  const t = useT();
  const app = useApp();
  const a = useAnalyzer();
  const openMenu = useContextMenu((s) => s.open);
  const [searchText, setSearchText] = useState(a.query);
  const [fastScan, setFastScan] = useState(false);
  const [mapHeight, setMapHeight] = useState(() => Number(localStorage.getItem("nexus.mapHeight")) || 300);
  const searchRef = useRef<HTMLInputElement>(null);
  const targetRef = useRef<HTMLInputElement>(null);
  const tableRef = useRef<VirtualTableHandle>(null);

  const elevated = app.info?.os.elevated ?? false;
  const target = a.target || app.drives.find((d) => d.ready && d.kind === "fixed")?.root || "C:\\";
  const drive = app.drives.find((d) => d.root.toLowerCase() === target.toLowerCase());
  const canFast = !!drive?.fastScanCapable && !elevated && app.settings.preferFastScan;

  useEffect(() => setSearchText(a.query), [a.query]);

  // Scroll requests from outside the table (treemap, Programs → map).
  useEffect(() => {
    if (a.scrollRequest && !a.results) setTimeout(() => tableRef.current?.scrollToIndex(a.scrollRequest!.index), 50);
  }, [a.scrollRequest?.seq]); // eslint-disable-line react-hooks/exhaustive-deps

  const scan = useCallback(
    (root = target) => {
      a.startScan(root, canFast && fastScan && root.length === 3);
    },
    [a, target, canFast, fastScan],
  );

  const targetFor = useCallback(
    async (id: number): Promise<FileTarget | undefined> => {
      if (a.scanId === undefined) return undefined;
      try {
        const d = await api.nodeDetails(a.scanId, id);
        return { scanId: a.scanId, node: id, path: d.path, isDir: d.isDir };
      } catch {
        return undefined;
      }
    },
    [a.scanId],
  );

  const refreshAfterChange = useCallback(() => {
    a.refresh();
    if (a.results && a.query) a.runSearch(a.query);
  }, [a]);

  const showInTreemap = useCallback(
    async (id: number) => {
      if (a.scanId === undefined) return;
      const d = await api.nodeDetails(a.scanId, id);
      const parent = d.ancestors.length >= 2 ? d.ancestors[d.ancestors.length - 2] : 0;
      a.setTreemapRoot(d.isDir ? id : parent);
      a.setHighlight([id]);
      a.select(id);
    },
    [a],
  );

  const menuFor = useCallback(
    async (id: number, x: number, y: number) => {
      const tg = await targetFor(id);
      if (!tg) return;
      openMenu(x, y, fileMenu(tg, {
        onAnalyzeFolder: (p) => {
          a.setTarget(p);
          scan(p);
        },
        onShowInTreemap: () => showInTreemap(id),
        onChanged: refreshAfterChange,
      }));
    },
    [targetFor, openMenu, a, scan, showInTreemap, refreshAfterChange],
  );

  // Page-level keyboard shortcuts.
  useEffect(() => {
    const onKey = async (e: KeyboardEvent) => {
      if (e.key === "F5") {
        e.preventDefault();
        a.rescan();
        return;
      }
      if (e.ctrlKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      if (e.ctrlKey && e.key.toLowerCase() === "l") {
        e.preventDefault();
        targetRef.current?.focus();
        targetRef.current?.select();
        return;
      }
      if (e.key === "Escape" && !document.querySelector(".modal-backdrop, .ctx-menu")) {
        if (a.job) a.cancelScan();
        else if (a.results) a.clearSearch();
        return;
      }
      if (isTyping(e) || a.selected === undefined || document.querySelector(".modal-backdrop")) return;
      if (e.key === "Delete") {
        e.preventDefault();
        const tg = await targetFor(a.selected);
        if (tg) requestDelete([tg], e.shiftKey, refreshAfterChange);
      } else if (e.key === "F2") {
        e.preventDefault();
        const tg = await targetFor(a.selected);
        if (tg) app.requestRename(tg.path, refreshAfterChange);
      } else if (e.ctrlKey && e.key.toLowerCase() === "c") {
        const tg = await targetFor(a.selected);
        if (tg) copyText(tg.path);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [a, app, targetFor, refreshAfterChange]);

  const treeColumns: Column<NodeRow>[] = useMemo(
    () => [
      {
        key: "name",
        label: t("common.name"),
        sortKey: "name",
        render: (r) => (
          <div className="name-cell" style={{ paddingLeft: `${r.depth * 1.05}rem` }}>
            {r.isDir && r.childCount > 0 ? (
              <button
                className="twisty"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  a.toggle(r.id, !r.expanded);
                }}
                aria-label={r.expanded ? "Collapse" : "Expand"}
              >
                {r.expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </button>
            ) : (
              <span className="twisty" />
            )}
            <NodeIcon row={r} />
            <span className="ellipsis" title={r.name}>{r.name}</span>
          </div>
        ),
      },
      { key: "share", label: t("common.percentOfParent"), width: 6.5, render: (r) => <div title={formatPercent(r.parentShare)}><ShareBar value={r.parentShare} /></div> },
      { key: "pct", label: "%", width: 4.6, align: "right", render: (r) => <span className="muted">{formatPercent(r.parentShare, 1)}</span> },
      { key: "alloc", label: t("common.allocated"), width: 6.5, align: "right", sortKey: "alloc", render: (r) => formatBytes(r.alloc) },
      { key: "size", label: t("common.size"), width: 6.5, align: "right", sortKey: "size", render: (r) => <span className="muted">{formatBytes(r.size)}</span> },
      { key: "files", label: t("common.files"), width: 6, align: "right", sortKey: "files", render: (r) => (r.isDir ? formatNumber(r.files) : "") },
      { key: "dirs", label: t("common.folders"), width: 5.5, align: "right", render: (r) => (r.isDir ? formatNumber(r.dirs) : "") },
      { key: "modified", label: t("common.modified"), width: 8.6, sortKey: "modified", className: "dim", render: (r) => formatCompactDate(r.modified) },
    ],
    [t, a],
  );

  const resultColumns: Column<NodeRow>[] = useMemo(
    () => [
      { key: "name", label: t("common.name"), width: 14, sortKey: "name", render: (r) => <div className="name-cell"><NodeIcon row={r} /><span className="ellipsis" title={r.name}>{r.name}</span></div> },
      { key: "path", label: t("common.path"), sortKey: "path", className: "dim", render: (r) => <span title={r.path}>{r.path}</span> },
      { key: "size", label: t("common.size"), width: 6.5, align: "right", sortKey: "size", render: (r) => formatBytes(r.size) },
      { key: "alloc", label: t("common.allocated"), width: 6.5, align: "right", sortKey: "alloc", render: (r) => <span className="muted">{formatBytes(r.alloc)}</span> },
      { key: "ext", label: t("common.extension"), width: 4.5, sortKey: "ext", className: "dim", render: (r) => r.ext },
      { key: "modified", label: t("common.modified"), width: 8.6, sortKey: "modified", className: "dim", render: (r) => formatCompactDate(r.modified) },
      { key: "created", label: t("common.created"), width: 8.6, sortKey: "created", className: "dim", render: (r) => formatCompactDate(r.created) },
      { key: "accessed", label: t("common.accessed"), width: 8.6, sortKey: "accessed", className: "dim", render: (r) => formatCompactDate(r.accessed) },
      { key: "attr", label: t("common.attributes"), width: 4.2, className: "dim mono", render: (r) => formatAttributes(r.attributes) },
    ],
    [t],
  );

  const fetchTree = useCallback((o: number, l: number) => api.viewRows(a.viewId!, o, l), [a.viewId]);
  const fetchResults = useCallback((o: number, l: number) => api.resultRows(a.results!.resultId, o, l), [a.results]);

  const onTableKey = (e: RKeyboardEvent, row: NodeRow | undefined) => {
    if (!row || !row.isDir || a.results) return;
    if (e.key === "ArrowRight" && !row.expanded) {
      e.preventDefault();
      a.toggle(row.id, true);
    } else if (e.key === "ArrowLeft" && row.expanded) {
      e.preventDefault();
      a.toggle(row.id, false);
    }
  };

  const startDrag = (e: React.MouseEvent) => {
    const startY = e.clientY;
    const startH = mapHeight;
    const move = (ev: MouseEvent) => setMapHeight(Math.max(120, Math.min(window.innerHeight - 220, startH - (ev.clientY - startY))));
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      setMapHeight((h) => {
        localStorage.setItem("nexus.mapHeight", String(h));
        return h;
      });
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const exportAs = async (format: "csv" | "json") => {
    if (a.scanId === undefined) return;
    const path = await saveDialog({ filters: [{ name: format.toUpperCase(), extensions: [format] }], defaultPath: `scan.${format}` });
    if (!path) return;
    try {
      const n = a.results ? await api.exportResults(a.results.resultId, format, path) : await api.exportScan(a.scanId, format, path);
      app.toast("success", t("analyzer.exported", { n: formatNumber(n) }));
    } catch (e) {
      app.toastError(e);
    }
  };

  const openSnapshot = async () => {
    const path = await openDialog({ filters: [{ name: "Snapshot", extensions: ["hdcs", "nxs"] }], multiple: false });
    if (typeof path !== "string") return;
    try {
      const [id, meta] = await api.openSnapshot(path);
      await a.adoptScan(id, meta, true);
    } catch (e) {
      app.toastError(e);
    }
  };

  const saveSnapshot = async () => {
    if (a.scanId === undefined) return;
    const path = await saveDialog({ filters: [{ name: "Snapshot", extensions: ["hdcs", "nxs"] }], defaultPath: "scan.hdcs" });
    if (!path) return;
    api.exportSnapshot(a.scanId, path).then(() => app.toast("success", path)).catch(app.toastError);
  };

  const p = a.job?.progress;
  const elapsed = a.job ? Date.now() - a.job.startedAt : 0;
  const fastNote = a.meta?.notes.find((n) => n.startsWith("fastScanUnavailable:"));

  return (
    <div className="page fill">
      <div className="toolbar" role="toolbar">
        <div className="input-wrap" style={{ width: "15rem" }}>
          <HardDrive size={15} />
          <input
            ref={targetRef}
            className="input search"
            style={{ width: "100%" }}
            list="drive-list"
            value={target}
            aria-label={t("analyzer.chooseDrive")}
            onChange={(e) => a.setTarget(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && scan()}
          />
          <datalist id="drive-list">
            {app.drives.filter((d) => d.ready).map((d) => (
              <option key={d.root} value={d.root}>{`${d.label || t(`kinds.${d.kind}`)} · ${d.fileSystem}`}</option>
            ))}
          </datalist>
        </div>
        <button
          className="btn icon"
          title={t("analyzer.browseFolder")}
          onClick={async () => {
            const dir = await openDialog({ directory: true, multiple: false });
            if (typeof dir === "string") a.setTarget(dir);
          }}
        >
          <FolderOpen size={16} />
        </button>
        {a.job ? (
          <button className="btn" onClick={() => a.cancelScan()} disabled={a.job.id < 0}>
            <Square size={14} />
            {t("analyzer.cancelScan")}
          </button>
        ) : (
          <button className="btn primary" onClick={() => scan()}>
            <Play size={14} />
            {t("analyzer.scan")}
          </button>
        )}
        {canFast && !a.job && (
          <label className="checkbox" title={t("analyzer.fastScanOffer")} style={{ alignItems: "center" }}>
            <input type="checkbox" checked={fastScan} onChange={(e) => setFastScan(e.target.checked)} />
            <Zap size={14} color="var(--warning)" />
            <span>{t("analyzer.useFastScan")}</span>
          </label>
        )}
        <button className="btn icon" title={t("analyzer.rescan")} disabled={!a.meta || !!a.job || a.fromSnapshot} onClick={() => a.rescan()}>
          <RefreshCw size={15} />
        </button>
        <div className="input-wrap grow" style={{ minWidth: "14rem" }}>
          <Search size={15} />
          <input
            ref={searchRef}
            className="input search"
            style={{ width: "100%" }}
            placeholder={t("analyzer.searchPlaceholder")}
            value={searchText}
            disabled={a.scanId === undefined}
            onChange={(e) => setSearchText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") a.runSearch(searchText);
              if (e.key === "Escape") {
                setSearchText("");
                a.clearSearch();
              }
            }}
            aria-label={t("analyzer.searchPlaceholder")}
          />
        </div>
        {a.results && (
          <button className="btn ghost sm" onClick={() => a.clearSearch()}>
            <X size={14} />
            {t("analyzer.clearSearch")}
          </button>
        )}
        <button className="btn icon" title={t("analyzer.openSnapshot")} onClick={openSnapshot}><Upload size={15} /></button>
        <button
          className="btn icon"
          title={t("common.export")}
          disabled={a.scanId === undefined}
          onClick={(e) =>
            openMenu(e.clientX, e.clientY, [
              { label: t("analyzer.exportCsv"), onClick: () => exportAs("csv") },
              { label: t("analyzer.exportJson"), onClick: () => exportAs("json") },
              "sep",
              { label: t("analyzer.exportSnapshot"), onClick: saveSnapshot },
            ])
          }
        >
          <Download size={15} />
        </button>
      </div>

      {a.scanId !== undefined && !a.job && (
        <div className="row wrap" style={{ padding: "0.45rem 1rem", gap: "0.4rem", borderBottom: "1px solid var(--border)" }}>
          <span className="faint" style={{ fontSize: "0.82rem" }}>{t("analyzer.quickFilters")}</span>
          {QUICK_FILTERS.map(([k, q]) => (
            <button
              key={k}
              className={`chip ${a.query === q ? "on" : ""}`}
              onClick={() => {
                setSearchText(q);
                a.runSearch(q);
              }}
            >
              {t(`analyzer.${k}`)}
            </button>
          ))}
        </div>
      )}

      {a.error && (
        <div style={{ padding: "0.8rem 1rem 0" }}>
          <ErrorView error={a.error} onRetry={() => scan()} />
        </div>
      )}

      {a.job ? (
        <div className="progress-panel card" style={{ margin: "3rem auto" }}>
          <h3>{t("analyzer.scanning", { root: a.job.root })}</h3>
          <div className="muted" style={{ marginBottom: "0.6rem" }}>{t(`analyzer.phase_${p?.phase ?? "starting"}`)}</div>
          <div className={`progress-track ${p && p.recordsTotal > 0 ? "" : "indeterminate"}`}>
            <div style={p && p.recordsTotal > 0 ? { width: `${(p.recordsDone / p.recordsTotal) * 100}%` } : undefined} />
          </div>
          <div className="grid cols-4" style={{ marginTop: "1rem" }}>
            <div className="stat"><span className="value">{formatNumber(p?.files ?? 0)}</span><span className="label">{t("common.files")}</span></div>
            <div className="stat"><span className="value">{formatNumber(p?.dirs ?? 0)}</span><span className="label">{t("common.folders")}</span></div>
            <div className="stat"><span className="value">{formatBytes(p?.bytes ?? 0)}</span><span className="label">{t("common.size")}</span></div>
            <div className="stat">
              <span className="value">{formatDuration(elapsed)}</span>
              <span className="label">{t("analyzer.filesPerSec", { n: formatNumber(Math.round(((p?.files ?? 0) + (p?.dirs ?? 0)) / Math.max(0.5, elapsed / 1000))) })}</span>
            </div>
          </div>
          {p?.current && <div className="faint ellipsis mono" style={{ marginTop: "0.8rem", fontSize: "0.8rem" }}>{p.current.replace(/^\\\\\?\\/, "")}</div>}
          {a.job.elevated && <div className="faint" style={{ marginTop: "0.6rem" }}>{t("analyzer.fastScanUsed")}</div>}
        </div>
      ) : a.scanId === undefined || !a.meta ? (
        <div className="empty" style={{ flex: 1 }}>
          <HardDrive size={40} strokeWidth={1.2} />
          <h2>{t("analyzer.emptyTitle")}</h2>
          <span>{t("analyzer.emptyHint")}</span>
          {canFast && <div className="banner info" style={{ maxWidth: "40rem", marginTop: "1rem", textAlign: "left" }}><Zap size={16} color="var(--warning)" /><span>{t("analyzer.fastScanOffer")}</span></div>}
        </div>
      ) : (
        <div className="analyzer-body">
          <div className="analyzer-top">
            <div className="analyzer-main">
              <div className="row wrap" style={{ gap: "0.6rem", fontSize: "0.88rem" }}>
                <strong className="mono">{a.meta.rootPath}</strong>
                <span className="muted">
                  {t("analyzer.scanned", {
                    files: formatNumber(a.meta.files), dirs: formatNumber(a.meta.dirs), size: formatBytes(a.meta.totalAlloc), time: formatDuration(a.meta.durationMs),
                  })}
                </span>
                <span className="badge neutral">{t(`analyzer.method_${a.meta.method}`)}</span>
                {a.fromSnapshot && <span className="badge review">{t("analyzer.snapshotLoaded", { when: formatDate(a.meta.startedMs) })}</span>}
                {a.meta.errorCount > 0 && (
                  <span className="badge dangerous" title={`${t("analyzer.unreadableHint")}\n${a.meta.errorSamples.slice(0, 8).map((s) => s.path).join("\n")}`}>
                    <ShieldAlert size={12} />
                    {t("analyzer.unreadable", { n: formatNumber(a.meta.errorCount) })}
                  </span>
                )}
                {fastNote && <span className="faint" title={fastNote}>{t("analyzer.fastScanFallback", { reason: t(`errors.${fastNote.split(":")[1]}`) })}</span>}
                <span className="grow" />
                {!a.results && (
                  <>
                    <button className="btn ghost sm" onClick={() => a.expandAll()}><ChevronsUpDown size={14} />{t("analyzer.expandAll")}</button>
                    <button className="btn ghost sm" onClick={() => a.collapseAll()}><ChevronsDownUp size={14} />{t("analyzer.collapseAll")}</button>
                  </>
                )}
                {a.results && (
                  <span className="muted">
                    {t("analyzer.matches", { n: formatNumber(a.results.total), size: formatBytes(a.results.totalSize) })} · {t("analyzer.searchTime", { ms: a.results.elapsedMs })}
                  </span>
                )}
              </div>
              {a.results ? (
                a.results.total === 0 ? (
                  <div className="empty"><Search size={28} /><span>{t("analyzer.noResults")}</span></div>
                ) : (
                  <VirtualTable
                    ref={tableRef}
                    ariaLabel={t("analyzer.results")}
                    columns={resultColumns}
                    total={a.results.total}
                    version={`r${a.results.resultId}-${a.version}`}
                    fetchPage={fetchResults}
                    rowId={(r) => r.id}
                    selected={a.selected}
                    sort={a.resultSort}
                    desc={a.resultDesc}
                    onSort={(k) => a.setResultSort(k as SortKey)}
                    onSelect={(r) => a.select(r.id)}
                    onActivate={(r) => r.path && api.openPath(r.path).catch(app.toastError)}
                    onContextMenu={(r, e) => menuFor(r.id, e.clientX, e.clientY)}
                  />
                )
              ) : (
                <VirtualTable
                  ref={tableRef}
                  ariaLabel={t("analyzer.tree")}
                  columns={treeColumns}
                  total={a.total}
                  version={`t${a.viewId}-${a.version}`}
                  fetchPage={fetchTree}
                  rowId={(r) => r.id}
                  selected={a.selected}
                  sort={a.sort}
                  desc={a.desc}
                  onSort={(k) => a.setSort(k as SortKey)}
                  onSelect={(r) => a.select(r.id)}
                  onActivate={(r) => (r.isDir ? a.toggle(r.id, !r.expanded) : undefined)}
                  onContextMenu={(r, e) => menuFor(r.id, e.clientX, e.clientY)}
                  onKeyDown={onTableKey}
                />
              )}
            </div>
            <DetailsPanel scanId={a.scanId} node={a.selected} version={a.version} onShowInTreemap={showInTreemap} onChanged={refreshAfterChange} />
          </div>
          <div className="splitter" onMouseDown={startDrag} role="separator" aria-orientation="horizontal" />
          <div className="treemap-wrap" style={{ height: mapHeight }}>
            <TreemapBar />
            <Treemap
              scanId={a.scanId}
              root={a.treemapRoot ?? 0}
              version={a.version}
              metric={app.settings.treemapMetric}
              maxRects={app.settings.treemapMaxRects}
              highlight={a.highlight}
              selected={a.selected}
              onSelect={(id) => {
                a.select(id);
                if (!a.results) a.reveal(id).then((idx) => idx !== null && tableRef.current?.scrollToIndex(idx));
              }}
              onZoom={(id) => {
                a.setTreemapRoot(id);
                a.setHighlight([]);
              }}
              onActivate={(id) => {
                a.clearSearch();
                a.reveal(id).then((idx) => idx !== null && tableRef.current?.scrollToIndex(idx));
              }}
              onContextMenu={(id, x, y) => menuFor(id, x, y)}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function TreemapBar() {
  const t = useT();
  const a = useAnalyzer();
  const [crumbs, setCrumbs] = useState<{ id: number; name: string }[]>([]);
  useEffect(() => {
    if (a.scanId === undefined || !a.meta) return;
    const root = a.treemapRoot ?? 0;
    if (root === 0) {
      setCrumbs([{ id: 0, name: a.meta.rootPath }]);
      return;
    }
    api.nodeDetails(a.scanId, root).then((d) => {
      const rel = d.path.slice(a.meta!.rootPath.length).split("\\").filter(Boolean);
      const list = [{ id: 0, name: a.meta!.rootPath }];
      d.ancestors.slice(1).forEach((id, i) => list.push({ id, name: rel[i] ?? "?" }));
      setCrumbs(list);
    }).catch(() => setCrumbs([{ id: 0, name: a.meta!.rootPath }]));
  }, [a.scanId, a.treemapRoot, a.meta, a.version]);

  return (
    <div className="treemap-bar">
      <strong style={{ marginRight: "0.4rem" }}>{t("analyzer.treemap")}</strong>
      <button
        className="btn ghost sm icon"
        title={t("analyzer.zoomOut")}
        disabled={(a.treemapRoot ?? 0) === 0}
        onClick={() => crumbs.length >= 2 && a.setTreemapRoot(crumbs[crumbs.length - 2].id)}
      >
        <ArrowUpLeft size={14} />
      </button>
      <div className="crumbs">
        {crumbs.map((c, i) => (
          <span key={c.id} className="row" style={{ gap: 0 }}>
            {i > 0 && <ChevronRight size={12} className="faint" />}
            <button onClick={() => a.setTreemapRoot(c.id)}>{c.name}</button>
          </span>
        ))}
      </div>
      {a.highlight.length > 0 && (
        <button className="chip on" onClick={() => a.setHighlight([])}>
          <X size={12} />
          {t("analyzer.clearHighlight")}
        </button>
      )}
      <span className="grow" />
      <TreemapLegend />
    </div>
  );
}
