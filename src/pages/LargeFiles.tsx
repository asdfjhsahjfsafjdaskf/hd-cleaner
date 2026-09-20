import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Download, FileStack, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useContextMenu } from "../components/ContextMenu";
import { fileMenu, requestDelete } from "../components/fileActions";
import { NodeIcon } from "../components/ui";
import { VirtualTable, type Column } from "../components/VirtualTable";
import { useT } from "../i18n";
import { api } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import type { NodeRow, ScanStats, SearchSummary } from "../types";
import { CATEGORY_COLORS } from "../utils/colors";
import { formatBytes, formatCompactDate, formatNumber } from "../utils/format";

type GroupBy = "none" | "extension" | "folder" | "category";

export function LargeFiles() {
  const t = useT();
  const app = useApp();
  const a = useAnalyzer();
  const openMenu = useContextMenu((s) => s.open);
  const [count, setCount] = useState(100);
  const [groupBy, setGroupBy] = useState<GroupBy>("none");
  const [res, setRes] = useState<SearchSummary>();
  const [stats, setStats] = useState<ScanStats>();
  const [all, setAll] = useState<NodeRow[]>([]);
  const [group, setGroup] = useState<string>();
  const [selected, setSelected] = useState<number>();
  const [ver, setVer] = useState(0);

  const load = useCallback(async () => {
    if (a.scanId === undefined) return;
    try {
      const r = await api.search(a.scanId, "type:file", { sort: "size", desc: true, limit: count });
      setRes(r);
      const rows: NodeRow[] = [];
      for (let o = 0; o < r.total; o += 2000) rows.push(...(await api.resultRows(r.resultId, o, 2000)).rows);
      setAll(rows);
      setVer((v) => v + 1);
    } catch (e) {
      app.toastError(e);
    }
  }, [a.scanId, count, app]);

  useEffect(() => {
    load();
  }, [load, a.version]);

  useEffect(() => {
    if (a.scanId !== undefined) api.scanStats(a.scanId).then(setStats).catch(() => {});
  }, [a.scanId, a.version]);

  const keyOf = useCallback(
    (r: NodeRow) => {
      switch (groupBy) {
        case "extension": return r.ext ? `.${r.ext}` : "—";
        case "category": return r.category;
        case "folder": return r.path ? r.path.slice(0, r.path.lastIndexOf("\\")) : "";
        default: return "";
      }
    },
    [groupBy],
  );

  const groups = useMemo(() => {
    if (groupBy === "none") return [];
    const m = new Map<string, { key: string; n: number; size: number }>();
    for (const r of all) {
      const k = keyOf(r);
      const g = m.get(k) ?? { key: k, n: 0, size: 0 };
      g.n++;
      g.size += r.size;
      m.set(k, g);
    }
    return [...m.values()].sort((x, y) => y.size - x.size);
  }, [all, groupBy, keyOf]);

  const visible = useMemo(() => (groupBy !== "none" && group !== undefined ? all.filter((r) => keyOf(r) === group) : all), [all, group, groupBy, keyOf]);

  const columns: Column<NodeRow>[] = useMemo(
    () => [
      { key: "name", label: t("common.name"), width: 18, render: (r) => <div className="name-cell"><NodeIcon row={r} /><span className="ellipsis" title={r.name}>{r.name}</span></div> },
      { key: "path", label: t("common.path"), className: "dim", render: (r) => <span title={r.path}>{r.path}</span> },
      { key: "size", label: t("common.size"), width: 7, align: "right", render: (r) => formatBytes(r.size) },
      { key: "alloc", label: t("common.allocated"), width: 7, align: "right", render: (r) => <span className="muted">{formatBytes(r.alloc)}</span> },
      { key: "cat", label: t("common.category"), width: 8, className: "dim", render: (r) => t(`categories.${r.category}`) },
      { key: "modified", label: t("common.modified"), width: 8.6, className: "dim", render: (r) => formatCompactDate(r.modified) },
    ],
    [t],
  );

  if (a.scanId === undefined || !a.meta) {
    return (
      <div className="page">
        <div className="page-header"><h1>{t("largeFiles.title")}</h1></div>
        <div className="empty">
          <FileStack size={36} strokeWidth={1.3} />
          <span>{t("largeFiles.needScan")}</span>
          <button className="btn primary" onClick={() => app.navigate("analyzer")}>{t("largeFiles.goAnalyze")}</button>
        </div>
      </div>
    );
  }

  const maxHist = Math.max(1, ...(stats?.histogram.map((h) => h.alloc) ?? [1]));
  const target = (r: NodeRow) => ({ scanId: a.scanId, node: r.id, path: r.path!, isDir: false });

  return (
    <div className="page" style={{ display: "flex", flexDirection: "column" }}>
      <div className="page-header">
        <h1>{t("largeFiles.title")}</h1>
        <span className="muted">{t("largeFiles.of", { root: a.meta.rootPath })}</span>
        <span className="spacer" />
        <span className="muted">{t("largeFiles.top")}</span>
        <div className="seg">
          {[100, 500, 1000, 5000].map((n) => (
            <button key={n} className={count === n ? "on" : ""} onClick={() => setCount(n)}>{n}</button>
          ))}
        </div>
        <input className="input" style={{ width: "6rem" }} type="number" min={10} max={100000} value={count}
          onChange={(e) => setCount(Math.max(10, Math.min(100000, Number(e.target.value) || 100)))} aria-label={t("largeFiles.top")} />
        <select className="select" value={groupBy} onChange={(e) => { setGroupBy(e.target.value as GroupBy); setGroup(undefined); }} aria-label={t("largeFiles.groupBy")}>
          {(["none", "extension", "folder", "category"] as GroupBy[]).map((g) => <option key={g} value={g}>{t(`largeFiles.group_${g}`)}</option>)}
        </select>
        <button className="btn icon" title={t("common.export")} disabled={!res} onClick={async () => {
          const path = await saveDialog({ filters: [{ name: "CSV", extensions: ["csv"] }], defaultPath: "largest-files.csv" });
          if (path && res) api.exportResults(res.resultId, "csv", path).then((n) => app.toast("success", t("analyzer.exported", { n }))).catch(app.toastError);
        }}><Download size={15} /></button>
      </div>

      {stats && (
        <div className="card" style={{ marginBottom: "0.9rem" }}>
          <h3>{t("largeFiles.distribution")}</h3>
          <div className="hist-bars">
            {stats.histogram.map((h) => (
              <div key={h.key} className="hb" title={`${formatNumber(h.files)} · ${formatBytes(h.alloc)}`}>
                <span>{formatBytes(h.alloc)}</span>
                <div style={{ height: `${(h.alloc / maxHist) * 70}%` }} />
                <span>{h.key}</span>
                <span className="faint">{formatNumber(h.files)}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="row" style={{ gap: "0.9rem", alignItems: "stretch", flex: 1, minHeight: "22rem" }}>
        {groupBy !== "none" && (
          <div className="list" style={{ width: "22rem", overflow: "auto" }}>
            <div className={`list-item ${group === undefined ? "" : ""}`} style={{ cursor: "pointer", fontWeight: group === undefined ? 600 : 400 }} onClick={() => setGroup(undefined)}>
              <span className="grow">{t("common.all")}</span>
              <span className="muted">{formatNumber(all.length)}</span>
            </div>
            {groups.map((g) => (
              <div key={g.key} className="list-item" style={{ cursor: "pointer", background: group === g.key ? "var(--bg-active)" : undefined }} onClick={() => setGroup(g.key)}>
                {groupBy === "category" && <i style={{ width: 10, height: 10, borderRadius: 2, background: CATEGORY_COLORS[g.key as keyof typeof CATEGORY_COLORS] }} />}
                <span className="grow ellipsis" title={g.key}>{groupBy === "category" ? t(`categories.${g.key}`) : g.key}</span>
                <span className="faint">{t("largeFiles.groupFiles", { n: g.n })}</span>
                <span style={{ minWidth: "5.5rem", textAlign: "right" }}>{formatBytes(g.size)}</span>
              </div>
            ))}
          </div>
        )}
        <div className="col grow" style={{ minHeight: 0 }}>
          <VirtualTable
            ariaLabel={t("largeFiles.title")}
            columns={columns}
            total={visible.length}
            version={`${ver}-${groupBy}-${group}`}
            fetchPage={async (o, l) => ({ total: visible.length, offset: o, rows: visible.slice(o, o + l) })}
            rowId={(r) => r.id}
            selected={selected}
            onSelect={(r) => setSelected(r.id)}
            onActivate={(r) => r.path && api.openPath(r.path).catch(app.toastError)}
            onContextMenu={(r, e) => openMenu(e.clientX, e.clientY, fileMenu(target(r), { onChanged: () => { a.bump(); } }))}
            onKeyDown={(e, r) => {
              if (r && e.key === "Delete") requestDelete([target(r)], e.shiftKey, () => a.bump());
            }}
          />
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn sm" disabled={selected === undefined} onClick={() => {
              const r = all.find((x) => x.id === selected);
              if (r) requestDelete([target(r)], false, () => a.bump());
            }}><Trash2 size={14} />{t("details.moveToRecycle")}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
