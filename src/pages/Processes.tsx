import { Cpu, Crosshair, Pause, Play, RefreshCw, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { ProcessPanel } from "../components/ProcessPanel";
import { FileIcon } from "../components/ProgramIcon";
import { Switch } from "../components/ui";
import { VirtualTable, type Column } from "../components/VirtualTable";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { ErrorPayload, ProcessRow } from "../types";
import { formatBytes, formatNumber } from "../utils/format";

type Sort = "name" | "pid" | "cpu" | "memory" | "user" | "company";
const REFRESH_MS = 2000;

export function Processes() {
  const t = useT();
  const openTarget = useApp((s) => s.openTarget);
  const [rows, setRows] = useState<ProcessRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const [query, setQuery] = useState("");
  const [showWindows, setShowWindows] = useState(false);
  const [paused, setPaused] = useState(false);
  const [sort, setSort] = useState<Sort>("memory");
  const [desc, setDesc] = useState(true);
  const [selected, setSelected] = useState<number>();
  const [tick, setTick] = useState(0);
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      setRows(await api.processesSample());
      setError(undefined);
      setLoaded(true);
      setTick((n) => n + 1);
    } catch (e) {
      setError(toError(e));
    }
    busy.current = false;
  }, []);

  useEffect(() => {
    void refresh();
    if (paused) return;
    const id = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, REFRESH_MS);
    return () => window.clearInterval(id);
  }, [paused, refresh]);

  const hiddenCount = useMemo(() => rows.filter((r) => r.isWindows).length, [rows]);
  const visible = useMemo(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const list = rows.filter((r) => {
      if (!showWindows && r.isWindows) return false;
      if (!words.length) return true;
      const hay = `${r.name} ${r.path ?? ""} ${r.company ?? ""} ${r.description ?? ""} ${r.user ?? ""} ${r.pid}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
    const cmp: Record<Sort, (a: ProcessRow, b: ProcessRow) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      pid: (a, b) => a.pid - b.pid,
      cpu: (a, b) => (a.cpu ?? -1) - (b.cpu ?? -1),
      memory: (a, b) => (a.privateBytes ?? -1) - (b.privateBytes ?? -1),
      user: (a, b) => (a.user ?? "").localeCompare(b.user ?? ""),
      company: (a, b) => (a.company ?? "").localeCompare(b.company ?? ""),
    };
    const sorted = [...list].sort(cmp[sort]);
    return desc ? sorted.reverse() : sorted;
  }, [rows, query, showWindows, sort, desc]);

  const onSort = (k: string) => {
    const s = k as Sort;
    if (s === sort) setDesc(!desc);
    else {
      setSort(s);
      setDesc(s === "cpu" || s === "memory");
    }
  };

  const columns: Column<ProcessRow>[] = useMemo(
    () => [
      {
        key: "name",
        label: t("common.name"),
        sortKey: "name",
        render: (r) => (
          <div className="name-cell">
            <FileIcon path={r.path} size={16} />
            <span className="ellipsis" title={r.path ?? r.name}>{r.name}</span>
            {r.description && <span className="faint ellipsis" style={{ fontSize: "0.8rem" }}>{r.description}</span>}
          </div>
        ),
      },
      { key: "pid", label: t("processes.col_pid"), width: 5, align: "right", sortKey: "pid", className: "dim", render: (r) => r.pid },
      { key: "cpu", label: t("processes.col_cpu"), width: 5, align: "right", sortKey: "cpu", render: (r) => (r.cpu == null ? "" : `${r.cpu.toFixed(1)}%`) },
      { key: "memory", label: t("processes.col_memory"), width: 7, align: "right", sortKey: "memory", render: (r) => (r.privateBytes == null ? <span className="faint">—</span> : formatBytes(r.privateBytes)) },
      { key: "user", label: t("processes.col_user"), width: 10, sortKey: "user", className: "dim", render: (r) => <span title={r.user ?? ""}>{r.user?.split("\\").pop() ?? ""}</span> },
      { key: "company", label: t("processes.col_publisher"), width: 12, sortKey: "company", className: "dim", render: (r) => <span title={r.company ?? ""}>{r.company ?? ""}</span> },
    ],
    [t],
  );

  const fetchPage = useCallback(async (o: number, l: number) => ({ total: visible.length, offset: o, rows: visible.slice(o, o + l) }), [visible]);
  const sel = rows.find((r) => r.pid === selected);

  return (
    <div className="page fill">
      <div className="toolbar">
        <div className="input-wrap grow" style={{ minWidth: "16rem", maxWidth: "30rem" }}>
          <Search size={15} />
          <input className="input search" style={{ width: "100%" }} placeholder={t("processes.search")} value={query} onChange={(e) => setQuery(e.target.value)} autoFocus aria-label={t("processes.search")} />
        </div>
        <label className="row" style={{ gap: "0.4rem", fontSize: "0.88rem" }}>
          <Switch checked={showWindows} onChange={setShowWindows} label={t("processes.showWindows")} />
          <span className="muted">{t("processes.showWindows")}</span>
        </label>
        <span className="grow" />
        <button className="btn" onClick={openTarget}><Crosshair size={14} />{t("target.button")}</button>
        <button className="btn" onClick={() => setPaused(!paused)}>{paused ? <Play size={14} /> : <Pause size={14} />}{paused ? t("processes.resume") : t("processes.pause")}</button>
        <button className="btn icon" title={t("common.refresh")} onClick={() => void refresh()}><RefreshCw size={15} /></button>
      </div>
      <div className="row" style={{ padding: "0.5rem 1rem", gap: "0.8rem", fontSize: "0.88rem", borderBottom: "1px solid var(--border)" }}>
        <Cpu size={15} className="muted" />
        <strong>{t("processes.count", { n: formatNumber(visible.length) })}</strong>
        {!showWindows && hiddenCount > 0 && <span className="faint">{t("processes.hiddenWindows", { n: formatNumber(hiddenCount) })}</span>}
      </div>
      {error && <div style={{ padding: "0.8rem 1rem 0" }}><ErrorView error={error} onRetry={refresh} /></div>}
      <div className="analyzer-top" style={{ minHeight: 0 }}>
        <div className="analyzer-main">
          {!loaded ? (
            <div className="empty">{t("common.loading")}</div>
          ) : (
            <VirtualTable
              ariaLabel={t("processes.title")}
              columns={columns}
              total={visible.length}
              version={`${tick}-${query}-${showWindows}-${sort}-${desc}`}
              fetchPage={fetchPage}
              rowId={(r) => r.pid}
              selected={selected}
              sort={sort}
              desc={desc}
              onSort={onSort}
              onSelect={(r) => setSelected(r.pid)}
            />
          )}
        </div>
        <aside className="details-panel" style={{ width: "28rem" }}>
          {sel ? (
            <ProcessPanel key={sel.pid} pid={sel.pid} onChanged={() => void refresh()} />
          ) : (
            <div className="empty" style={{ padding: "2rem 0.5rem" }}><Cpu size={24} /><span>{t("processes.selectHint")}</span></div>
          )}
        </aside>
      </div>
    </div>
  );
}
