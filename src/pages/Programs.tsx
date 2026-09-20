import {
  Boxes, ClipboardCopy, FolderOpen, FolderSearch, LayoutGrid, Play, RefreshCw, Search, Square, Trash2, Wrench, Zap,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { copyText } from "../components/fileActions";
import { ProgramIcon } from "../components/ProgramIcon";
import { Switch } from "../components/ui";
import { VirtualTable, type Column } from "../components/VirtualTable";
import { t as tr, useT } from "../i18n";
import { api } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import { isHidden, usePrograms, type ProgramSort, type SourceFilter } from "../stores/programs";
import type { AppSize, Program } from "../types";
import { formatBytes, formatNumber } from "../utils/format";

const SIZE_COLORS = { install: "#5b9be3", userData: "#9d8ff0", cache: "#b7a58c", logs: "#6fc3d6" };

function matches(p: Program, q: string) {
  if (!q) return true;
  const hay = `${p.name} ${p.publisher ?? ""} ${p.version ?? ""} ${p.installLocation ?? ""} ${p.inferredLocation ?? ""} ${p.packageFamilyName ?? ""}`.toLowerCase();
  return q.toLowerCase().split(/\s+/).every((w) => hay.includes(w));
}

/** Highlight a program's folders in the treemap (scanning the drive if needed). */
export async function viewProgramOnMap(p: Program, size?: AppSize) {
  const app = useApp.getState();
  const a = useAnalyzer.getState();
  const loc = size?.locations[0]?.path ?? p.installLocation ?? p.inferredLocation;
  if (!loc) return;
  const drive = loc.slice(0, 3);
  const covered = a.meta && a.scanId !== undefined && loc.toLowerCase().startsWith(a.meta.rootPath.toLowerCase().replace(/\\$/, ""));
  app.navigate("analyzer");
  if (covered) {
    await a.showProgramOnMap(p.id);
  } else {
    a.set({ pendingProgramMap: p.id });
    app.toast("info", tr("programs.mapScanning", { drive }));
    a.startScan(drive, false);
  }
}

function SizeBreakdown({ s }: { s: AppSize }) {
  const t = useT();
  const parts: [keyof typeof SIZE_COLORS, number][] = [["install", s.install], ["userData", s.userData], ["cache", s.cache], ["logs", s.logs]];
  const total = Math.max(1, s.total);
  return (
    <div className="col" style={{ gap: "0.5rem" }}>
      <div className="stacked-bar" aria-hidden>
        {parts.map(([k, v]) => v > 0 && <div key={k} style={{ width: `${(v / total) * 100}%`, background: SIZE_COLORS[k] }} />)}
      </div>
      <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: "0.3rem 1rem", fontSize: "0.9rem" }}>
        {parts.map(([k, v]) => (
          <div key={k} className="row" style={{ justifyContent: "space-between" }}>
            <span className="row"><i style={{ width: 9, height: 9, borderRadius: 2, background: SIZE_COLORS[k], display: "inline-block" }} />{t(`programs.${k}`)}</span>
            <span>{formatBytes(v)}</span>
          </div>
        ))}
      </div>
      <div className="row" style={{ justifyContent: "space-between", fontWeight: 600 }}>
        <span>{t("programs.total")}</span>
        <span>{formatBytes(s.total)}</span>
      </div>
      {s.reported != null && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("programs.reportedBy", { size: formatBytes(s.reported) })}</div>}
      {s.possibleTotal > 0 && <div className="banner info" style={{ fontSize: "0.85rem" }}>{t("programs.possibleNote", { size: formatBytes(s.possibleTotal) })}</div>}
    </div>
  );
}

function Field({ label, value, mono, copy }: { label: string; value?: string | null; mono?: boolean; copy?: boolean }) {
  const t = useT();
  if (!value) return null;
  return (
    <>
      <dt>{label}</dt>
      <dd className={mono ? "mono" : undefined}>
        {value}
        {copy && (
          <button className="btn ghost sm icon" style={{ marginLeft: 4, verticalAlign: "middle" }} title={t("programs.copy")} onClick={() => copyText(value)}>
            <ClipboardCopy size={12} />
          </button>
        )}
      </dd>
    </>
  );
}

function ProgramDetails({ p }: { p: Program }) {
  const t = useT();
  const app = useApp();
  const st = usePrograms();
  const size = st.details[p.id];
  const [busy, setBusy] = useState(false);

  const compute = async (refresh = false) => {
    setBusy(true);
    await st.computeSize(p.id, refresh);
    setBusy(false);
  };
  useEffect(() => {
    if (!st.details[p.id]) compute();
  }, [p.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const folder = p.installLocation ?? p.inferredLocation;
  const confBadge = { confirmed: "safe", probable: "review", possible: "dangerous" } as const;

  return (
    <aside className="details-panel" style={{ width: "28rem" }} aria-label={p.name}>
      <div className="row" style={{ alignItems: "flex-start", marginBottom: "0.4rem" }}>
        <ProgramIcon id={p.id} size={32} />
        <div className="col grow" style={{ gap: 0 }}>
          <strong style={{ fontSize: "1.05rem", wordBreak: "break-word" }}>{p.name}</strong>
          <span className="muted">{[p.version, p.publisher].filter(Boolean).join(" · ")}</span>
        </div>
      </div>
      <div className="row wrap" style={{ gap: "0.3rem", marginBottom: "0.8rem" }}>
        <span className="badge neutral">{t(`programs.source_${p.source}`)}</span>
        {p.arch !== "unknown" && <span className="badge neutral">{p.arch}</span>}
        <span className="badge neutral">{t(`programs.scope_${p.scope}`)}</span>
        {p.systemComponent && <span className="badge dangerous">{t("programs.badge_system")}</span>}
        {p.isUpdate && <span className="badge review">{t("programs.badge_update")}</span>}
        {p.isFramework && <span className="badge review">{t("programs.badge_framework")}</span>}
        {p.noRemove && <span className="badge blocked">{t("programs.noRemove")}</span>}
      </div>
      <div className="row wrap" style={{ gap: "0.35rem", marginBottom: "0.9rem" }}>
        <button className="btn sm danger" onClick={() => app.requestUninstall(p.id)}><Trash2 size={14} />{t("programs.uninstall")}</button>
        <button className="btn sm" onClick={() => app.requestBatch([p.id], true)}><Zap size={14} />{t("uninstall.quickUninstall")}</button>
        <button className="btn sm" onClick={() => app.requestForced({ name: p.name, folder: p.installLocation ?? p.inferredLocation ?? undefined, programId: p.id })}><Wrench size={14} />{t("uninstall.forcedSelected")}</button>
        <button className="btn sm" disabled={!folder} onClick={() => viewProgramOnMap(p, size)}><LayoutGrid size={14} />{t("programs.viewOnMap")}</button>
        <button className="btn sm" disabled={!folder} onClick={() => folder && api.revealPath(folder).catch(app.toastError)}><FolderOpen size={14} />{t("common.openFolder")}</button>
        <button className="btn sm" disabled={!folder} onClick={() => {
          if (!folder) return;
          useAnalyzer.getState().setTarget(folder);
          app.navigate("analyzer");
          useAnalyzer.getState().startScan(folder, false);
        }}><FolderSearch size={14} />{t("programs.analyzeFolder")}</button>
      </div>

      <div className="card" style={{ padding: "0.8rem", marginBottom: "0.9rem" }}>
        <div className="row" style={{ marginBottom: "0.6rem" }}>
          <h3 className="grow" style={{ margin: 0 }}>{t("programs.realSize")}</h3>
          <button className="btn ghost sm" disabled={busy} onClick={() => compute(true)}><RefreshCw size={13} />{size ? t("programs.recompute") : t("programs.compute")}</button>
        </div>
        {busy && !size && <div className="muted">{t("common.loading")}</div>}
        {size && <SizeBreakdown s={size} />}
      </div>

      {size && (
        <>
          <div className="section-title" style={{ marginTop: 0 }}>{t("programs.locations")}</div>
          {size.locations.length === 0 && <div className="muted">{t("programs.noLocations")}</div>}
          <div className="col" style={{ gap: "0.45rem", marginBottom: "0.9rem" }}>
            {size.locations.map((l) => (
              <div key={l.path} className="card" style={{ padding: "0.55rem 0.7rem" }}>
                <div className="row" style={{ gap: "0.4rem" }}>
                  <span className={`badge ${confBadge[l.confidence]}`}>{t(`programs.conf_${l.confidence}`)}</span>
                  <span className="faint" style={{ fontSize: "0.82rem" }}>{t(`programs.kind_${l.kind}`)}</span>
                  <span className="grow" />
                  <strong>{formatBytes(l.measured.total)}</strong>
                  <button className="btn ghost sm icon" title={t("common.openFolder")} onClick={() => api.revealPath(l.path).catch(app.toastError)}><FolderOpen size={13} /></button>
                </div>
                <div className="mono selectable" style={{ fontSize: "0.8rem", wordBreak: "break-all", margin: "0.25rem 0" }}>{l.path}</div>
                <div className="faint" style={{ fontSize: "0.8rem" }}>
                  {t(`programs.reason_${l.reason}`)} · {formatNumber(l.measured.files)} {t("common.files").toLowerCase()}
                  {l.measured.cache > 0 && ` · ${t("programs.cache")} ${formatBytes(l.measured.cache)}`}
                  {l.measured.logs > 0 && ` · ${t("programs.logs")} ${formatBytes(l.measured.logs)}`}
                  {` · ${t(`programs.from_${l.source}`)}`}
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      <div className="section-title" style={{ marginTop: 0 }}>{t("common.details")}</div>
      <dl className="kv">
        <Field label={t("programs.installLocation")} value={p.installLocation ?? (p.inferredLocation ? `${p.inferredLocation} (${t("programs.inferred")})` : undefined)} mono />
        <Field label={t("programs.col_installed")} value={p.installDate} />
        <Field label={t("programs.col_reported")} value={p.reportedSize != null ? formatBytes(p.reportedSize) : undefined} />
        <Field label={t("programs.registry")} value={p.registryKey} mono copy />
        <Field label={t("programs.uninstallCommand")} value={p.uninstallString} mono copy />
        <Field label={t("programs.quietUninstall")} value={p.quietUninstallString} mono copy />
        <Field label={t("programs.msiCode")} value={p.msiProductCode} mono copy />
        <Field label={t("programs.packageName")} value={p.packageFullName} mono copy />
        <Field label={t("programs.website")} value={p.url} mono copy />
      </dl>
      {p.dependencies.length > 0 && (
        <>
          <div className="section-title">{t("programs.dependencies")}</div>
          <div className="col mono" style={{ fontSize: "0.8rem", gap: "0.2rem" }}>
            {p.dependencies.map((d) => <span key={d} className="selectable">{d}</span>)}
          </div>
        </>
      )}
    </aside>
  );
}

export function Programs() {
  const t = useT();
  const st = usePrograms();
  const [search, setSearch] = useState(st.query);

  useEffect(() => {
    if (!st.loaded) st.load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const id = window.setTimeout(() => st.set({ query: search }), 120);
    return () => window.clearTimeout(id);
  }, [search]); // eslint-disable-line react-hooks/exhaustive-deps

  const hiddenCount = useMemo(() => st.programs.filter(isHidden).length, [st.programs]);

  const visible = useMemo(() => {
    const list = st.programs.filter(
      (p) =>
        (st.showHidden || !isHidden(p)) &&
        (st.source === "all" || p.source === st.source || (st.source === "store" && p.source === "appx")) &&
        matches(p, st.query),
    );
    const real = (p: Program) => st.sizes[p.id]?.[0] ?? -1;
    const cmp: Record<ProgramSort, (a: Program, b: Program) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      publisher: (a, b) => (a.publisher ?? "").localeCompare(b.publisher ?? ""),
      version: (a, b) => (a.version ?? "").localeCompare(b.version ?? "", undefined, { numeric: true }),
      installDate: (a, b) => (a.installDate ?? "").localeCompare(b.installDate ?? ""),
      reported: (a, b) => (a.reportedSize ?? -1) - (b.reportedSize ?? -1),
      real: (a, b) => real(a) - real(b),
    };
    const sorted = [...list].sort(cmp[st.sort]);
    return st.desc ? sorted.reverse() : sorted;
  }, [st.programs, st.showHidden, st.source, st.query, st.sort, st.desc, st.sizes]);

  const selected = st.programs.find((p) => p.id === st.selected);
  const version = `${visible.length}-${st.query}-${st.source}-${st.showHidden}-${st.sort}-${st.desc}-${Object.keys(st.sizes).length}-${st.job?.done ?? 0}`;

  const columns: Column<Program>[] = useMemo(
    () => [
      {
        key: "check",
        label: "",
        width: 2,
        render: (p) => (
          <input
            type="checkbox"
            aria-label={p.name}
            checked={st.checked.has(p.id)}
            onMouseDown={(e) => e.stopPropagation()}
            onChange={() => st.toggleChecked(p.id)}
            style={{ accentColor: "var(--accent)" }}
          />
        ),
      },
      {
        key: "name",
        label: t("common.name"),
        sortKey: "name",
        render: (p) => (
          <div className="name-cell">
            <ProgramIcon id={p.id} />
            <span className="ellipsis" title={p.name}>{p.name}</span>
            {isHidden(p) && <span className="badge neutral" style={{ fontSize: "0.68rem" }}>{p.isUpdate ? t("programs.badge_update") : p.isFramework ? t("programs.badge_framework") : t("programs.badge_system")}</span>}
          </div>
        ),
      },
      { key: "version", label: t("programs.col_version"), width: 8, sortKey: "version", className: "dim", render: (p) => p.version ?? "" },
      { key: "publisher", label: t("programs.col_publisher"), width: 12, sortKey: "publisher", className: "dim", render: (p) => <span title={p.publisher ?? ""}>{p.publisher ?? ""}</span> },
      { key: "date", label: t("programs.col_installed"), width: 6.5, sortKey: "installDate", className: "dim", render: (p) => p.installDate ?? "" },
      { key: "reported", label: t("programs.col_reported"), width: 6.5, align: "right", sortKey: "reported", className: "dim", render: (p) => (p.reportedSize != null ? formatBytes(p.reportedSize) : "") },
      { key: "real", label: t("programs.col_real"), width: 6.5, align: "right", sortKey: "real", render: (p) => (st.sizes[p.id] ? formatBytes(st.sizes[p.id][0]) : <span className="faint">—</span>) },
      { key: "arch", label: t("programs.col_arch"), width: 4, className: "dim", render: (p) => (p.arch === "unknown" ? "" : p.arch) },
      { key: "source", label: t("programs.col_source"), width: 6.5, className: "dim", render: (p) => t(`programs.source_${p.source}`) },
    ],
    [t, st.sizes, st.checked],
  );

  const fetchPage = useCallback(async (o: number, l: number) => ({ total: visible.length, offset: o, rows: visible.slice(o, o + l) }), [visible]);

  return (
    <div className="page fill">
      <div className="toolbar">
        <div className="input-wrap grow" style={{ minWidth: "16rem", maxWidth: "30rem" }}>
          <Search size={15} />
          <input className="input search" style={{ width: "100%" }} placeholder={t("programs.search")} value={search} onChange={(e) => setSearch(e.target.value)} autoFocus aria-label={t("programs.search")} />
        </div>
        <div className="seg">
          {(["all", "win32", "msi", "store"] as SourceFilter[]).map((s) => (
            <button key={s} className={st.source === s ? "on" : ""} onClick={() => st.set({ source: s })}>
              {s === "all" ? t("programs.all") : s === "store" ? `${t("programs.source_store")} / AppX` : t(`programs.source_${s}`)}
            </button>
          ))}
        </div>
        <label className="row" style={{ gap: "0.4rem", fontSize: "0.88rem" }}>
          <Switch checked={st.showHidden} onChange={(v) => st.set({ showHidden: v })} label={t("programs.showHidden")} />
          <span className="muted">{t("programs.showHidden")}</span>
        </label>
        <span className="grow" />
        {st.checked.size > 0 && (
          <button className="btn danger" onClick={() => useApp.getState().requestBatch([...st.checked], false)}><Trash2 size={14} />{t("uninstall.uninstallSelected", { n: st.checked.size })}</button>
        )}
        <button className="btn" onClick={() => useApp.getState().requestForced({})}><Wrench size={14} />{t("uninstall.forcedSelected")}</button>
        {st.job ? (
          <button className="btn" onClick={() => st.cancelAll()}><Square size={14} />{t("programs.calcRunning", { done: st.job.done, total: st.job.total })}</button>
        ) : (
          <button className="btn primary" disabled={!visible.length} onClick={() => st.computeAll(visible.map((p) => p.id))}><Play size={14} />{t("programs.calcSizes")}</button>
        )}
        <button className="btn icon" title={t("common.refresh")} disabled={st.loading} onClick={() => st.load(true)}><RefreshCw size={15} /></button>
      </div>
      <div className="row" style={{ padding: "0.5rem 1rem", gap: "0.8rem", fontSize: "0.88rem", borderBottom: "1px solid var(--border)" }}>
        <Boxes size={15} className="muted" />
        <strong>{t("programs.count", { n: formatNumber(visible.length) })}</strong>
        {!st.showHidden && hiddenCount > 0 && <span className="faint">{t("programs.hidden", { n: formatNumber(hiddenCount) })}</span>}
        {st.job && (
          <div className="progress-track grow" style={{ maxWidth: "20rem" }}>
            <div style={{ width: `${(st.job.done / Math.max(1, st.job.total)) * 100}%` }} />
          </div>
        )}
      </div>
      {st.error && <div style={{ padding: "0.8rem 1rem 0" }}><ErrorView error={st.error} onRetry={() => st.load(true)} /></div>}
      {st.appxError && <div style={{ padding: "0.8rem 1rem 0" }}><div className="banner warning">{t("programs.appxError")} ({st.appxError.message})</div></div>}
      <div className="analyzer-top" style={{ minHeight: 0 }}>
        <div className="analyzer-main">
          {!st.loaded ? (
            <div className="empty">{t("common.loading")}</div>
          ) : (
            <VirtualTable
              ariaLabel={t("programs.title")}
              columns={columns}
              total={visible.length}
              version={version}
              fetchPage={fetchPage}
              rowId={(p) => st.programs.indexOf(p)}
              selected={selected ? st.programs.indexOf(selected) : undefined}
              sort={st.sort}
              desc={st.desc}
              onSort={(k) => st.setSort(k as ProgramSort)}
              onSelect={(p) => st.select(p.id)}
            />
          )}
        </div>
        {selected ? (
          <ProgramDetails key={selected.id} p={selected} />
        ) : (
          <aside className="details-panel" style={{ width: "28rem" }}>
            <div className="empty" style={{ padding: "2rem 0.5rem" }}><Boxes size={24} /><span>{t("programs.selectHint")}</span></div>
          </aside>
        )}
      </div>
    </div>
  );
}
