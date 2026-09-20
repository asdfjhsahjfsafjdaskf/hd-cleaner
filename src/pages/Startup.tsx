import { Boxes, ClipboardCopy, FileCog, FolderOpen, Gauge, Power, PowerOff, RefreshCw, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { copyText } from "../components/fileActions";
import { Modal } from "../components/Modal";
import { FileIcon } from "../components/ProgramIcon";
import { startupDetailLabel, startupSourceLabel, toggleStartup } from "../components/systemItems";
import { Switch } from "../components/ui";
import { VirtualTable, type Column } from "../components/VirtualTable";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import type { ErrorPayload, Slowdown, StartupImpact, StartupItem } from "../types";
import { formatDate, formatNumber } from "../utils/format";

type Filter = "all" | "registry" | "folder" | "task" | "service";
type Sort = "name" | "status" | "source" | "company" | "impact";

const secs = (ms: number) => `${(ms / 1000).toLocaleString(undefined, { minimumFractionDigits: 1, maximumFractionDigits: 1 })} s`;

const kindOf: Record<StartupItem["source"]["kind"], Filter> = { runKey: "registry", startupFolder: "folder", task: "task", service: "service" };

function RemoveDialog({ item, onClose, onDone }: { item: StartupItem; onClose: () => void; onDone: () => void }) {
  const t = useT();
  const toast = useApp((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const disable = async () => {
    setBusy(true);
    if (await toggleStartup(item, false)) {
      onDone();
      onClose();
    }
    setBusy(false);
  };
  const remove = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const backup = await api.startupRemove(item.id, item.command);
      toast("success", t("startup.removed", { path: backup }));
      onDone();
      onClose();
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  };
  return (
    <Modal
      title={t("startup.removeTitle")}
      icon={<Trash2 size={17} color="var(--danger)" />}
      onClose={onClose}
      locked={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>{t("common.cancel")}</button>
          {item.enabled && item.canDisable && <button className="btn primary" onClick={disable} disabled={busy} data-autofocus><PowerOff size={14} />{t("startup.disableInstead")}</button>}
          <button className="btn danger" onClick={remove} disabled={busy}><Trash2 size={14} />{t("startup.removeAnyway")}</button>
        </>
      }
    >
      <p style={{ margin: 0 }}>{t("startup.removeBody", { name: item.name })}</p>
      <div className="mono faint selectable" style={{ fontSize: "0.82rem", wordBreak: "break-all" }}>{item.command}</div>
      {!item.exeExists && <div className="banner info">{t("startup.missingHint")}</div>}
      {item.needsAdmin && <div className="muted">{t("startup.adminNote")}</div>}
      {error && <ErrorView error={error} />}
    </Modal>
  );
}

function StartupDetails({ i, onChanged, impact }: { i: StartupItem; onChanged: () => void; impact: Slowdown | null | undefined }) {
  const t = useT();
  const app = useApp();
  const [removing, setRemoving] = useState(false);
  const detail = startupDetailLabel(i, t);
  const reveal = i.source.kind === "startupFolder" ? i.key : i.exe;
  return (
    <div className="col" style={{ gap: "0.8rem" }}>
      <div className="row" style={{ alignItems: "flex-start" }}>
        <FileIcon path={i.exe} size={32} />
        <div className="col grow" style={{ gap: 0, minWidth: 0 }}>
          <strong style={{ fontSize: "1.05rem", wordBreak: "break-word" }}>{i.name}</strong>
          <span className="muted">{[i.company, i.description].filter(Boolean).join(" · ")}</span>
        </div>
      </div>
      <div className="row wrap" style={{ gap: "0.3rem" }}>
        <span className={`badge ${i.enabled ? "safe" : "neutral"}`}>{i.enabled ? t("startup.enabled") : t("startup.disabled")}</span>
        <span className="badge neutral">{startupSourceLabel(i, t)}</span>
        {detail && <span className="badge neutral">{detail}</span>}
        {!i.exeExists && <span className="badge dangerous">{t("startup.badge_missing")}</span>}
        {i.isWindows && <span className="badge review">{t("startup.badge_windows")}</span>}
        {i.needsAdmin && <span className="badge neutral">{t("startup.badge_admin")}</span>}
      </div>
      <div className="row wrap" style={{ gap: "0.35rem" }}>
        {i.canDisable && (
          <button className={`btn sm ${i.enabled ? "" : "primary"}`} onClick={async () => { if (await toggleStartup(i, !i.enabled)) onChanged(); }}>
            {i.enabled ? <PowerOff size={14} /> : <Power size={14} />}{i.enabled ? t("startup.disable") : t("startup.enable")}
          </button>
        )}
        <button className="btn sm danger" disabled={!i.canRemove} onClick={() => setRemoving(true)}><Trash2 size={14} />{t("startup.remove")}</button>
        <button className="btn sm" disabled={!reveal || !i.exeExists} onClick={() => reveal && api.revealPath(reveal).catch(app.toastError)}><FolderOpen size={14} />{t("startup.openLocation")}</button>
        <button className="btn sm" disabled={!i.exe || !i.exeExists} onClick={() => i.exe && api.showProperties(i.exe).catch(app.toastError)}><FileCog size={14} />{t("common.properties")}</button>
      </div>
      {!i.exeExists && <div className="banner info">{t("startup.missingHint")}</div>}
      {!i.canRemove && <div className="faint" style={{ fontSize: "0.85rem" }}>{i.source.kind === "service" ? t("startup.notRemovableService") : t("startup.notRemovableWindows")}</div>}
      {i.source.kind === "service" && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("startup.serviceNote")}</div>}
      {i.source.kind === "runKey" && i.source.once && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("startup.runOnceNote")}</div>}
      {i.needsAdmin && <div className="faint" style={{ fontSize: "0.85rem" }}>{t("startup.adminNote")}</div>}

      <div className="section-title" style={{ margin: 0 }}>{t("startup.impactTitle")}</div>
      {impact === undefined ? (
        <div className="faint" style={{ fontSize: "0.85rem" }}>{t("startup.impactNotRead")}</div>
      ) : impact === null ? (
        <div className="muted" style={{ fontSize: "0.9rem" }}>{t("startup.impactNoneLong")}</div>
      ) : (
        <div className="card col" style={{ padding: "0.6rem 0.8rem", gap: "0.3rem" }}>
          <strong style={{ color: "var(--warning)" }}>+{secs(impact.avgDelayMs)}</strong>
          <span style={{ fontSize: "0.9rem" }}>{t("startup.impactAvg", { avg: secs(impact.avgDelayMs), max: secs(impact.maxDelayMs), n: impact.count, date: formatDate(impact.lastMs) })}</span>
          <span className="faint mono" style={{ fontSize: "0.78rem", wordBreak: "break-all" }}>{t("startup.impactMeasured", { path: impact.path })}</span>
        </div>
      )}

      {i.programId && (
        <div className="card row" style={{ padding: "0.5rem 0.7rem", gap: "0.4rem" }}>
          <Boxes size={15} className="muted" />
          <strong className="grow ellipsis">{i.programName}</strong>
          <button className="btn sm" onClick={() => { usePrograms.getState().select(i.programId!); app.navigate("programs"); }}>{t("processes.viewProgram")}</button>
        </div>
      )}

      <div className="section-title" style={{ margin: 0 }}>{t("common.details")}</div>
      <dl className="kv">
        <dt>{t("startup.command")}</dt>
        <dd className="mono selectable" style={{ wordBreak: "break-all" }}>
          {i.command}
          <button className="btn ghost sm icon" style={{ marginLeft: 4, verticalAlign: "middle" }} title={t("programs.copy")} onClick={() => copyText(i.command)}><ClipboardCopy size={12} /></button>
        </dd>
        {i.exe && i.exe !== i.command && (<><dt>{t("startup.file")}</dt><dd className="mono selectable" style={{ wordBreak: "break-all" }}>{i.exe}</dd></>)}
        <dt>{t("startup.location")}</dt><dd className="mono selectable" style={{ wordBreak: "break-all" }}>{i.location}</dd>
        <dt>{t("startup.entry")}</dt><dd className="mono selectable" style={{ wordBreak: "break-all" }}>{i.key}</dd>
        {i.disabledAtMs ? (<><dt>{t("startup.disabled")}</dt><dd>{t("startup.disabledAt", { date: formatDate(i.disabledAtMs) })}</dd></>) : null}
      </dl>
      {removing && <RemoveDialog item={i} onClose={() => setRemoving(false)} onDone={onChanged} />}
    </div>
  );
}

export function Startup() {
  const t = useT();
  const [items, setItems] = useState<StartupItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorPayload>();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<Sort>("name");
  const [desc, setDesc] = useState(false);
  const [selected, setSelected] = useState<string>();
  const [gen, setGen] = useState(0);
  const [impact, setImpact] = useState<StartupImpact | null>(null);
  const [reading, setReading] = useState(false);
  const app = useApp();

  const readImpact = useCallback(async (cached: boolean) => {
    if (!cached) setReading(true);
    try {
      const r = await api.startupImpact(cached);
      if (r) setImpact(r);
    } catch (e) {
      if (!cached) app.toastError(e);
    }
    setReading(false);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    void readImpact(true);
  }, [readImpact]);
  const delayOf = (i: StartupItem) => impact?.items[i.id]?.avgDelayMs ?? -1;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await api.startupList());
      setError(undefined);
      setLoaded(true);
      setGen((g) => g + 1);
    } catch (e) {
      setError(toError(e));
    }
    setLoading(false);
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const visible = useMemo(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const list = items.filter((i) => {
      if (filter !== "all" && kindOf[i.source.kind] !== filter) return false;
      const hay = `${i.name} ${i.command} ${i.company ?? ""} ${i.description ?? ""} ${i.programName ?? ""}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
    const cmp: Record<Sort, (a: StartupItem, b: StartupItem) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      status: (a, b) => Number(a.enabled) - Number(b.enabled),
      source: (a, b) => startupSourceLabel(a, t).localeCompare(startupSourceLabel(b, t)),
      company: (a, b) => (a.company ?? "").localeCompare(b.company ?? ""),
      impact: (a, b) => delayOf(a) - delayOf(b),
    };
    const sorted = [...list].sort(cmp[sort]);
    return desc ? sorted.reverse() : sorted;
  }, [items, query, filter, sort, desc, t, impact]); // eslint-disable-line react-hooks/exhaustive-deps

  const toggle = async (i: StartupItem) => {
    if (await toggleStartup(i, !i.enabled)) void load();
  };

  const columns: Column<StartupItem>[] = useMemo(
    () => [
      {
        key: "status",
        label: "",
        width: 4,
        sortKey: "status",
        render: (i) =>
          i.canDisable ? (
            <span className="row" onMouseDown={(e) => e.stopPropagation()}>
              <Switch checked={i.enabled} onChange={() => void toggle(i)} label={i.enabled ? t("startup.disable") : t("startup.enable")} />
            </span>
          ) : <span className="faint">—</span>,
      },
      {
        key: "name",
        label: t("common.name"),
        sortKey: "name",
        render: (i) => (
          <div className="name-cell">
            <FileIcon path={i.exe} size={16} />
            <span className="ellipsis" title={i.name} style={{ opacity: i.enabled ? 1 : 0.6 }}>{i.name}</span>
            {!i.exeExists && <span className="badge dangerous" style={{ fontSize: "0.68rem" }}>{t("startup.badge_missing")}</span>}
            {i.isWindows && <span className="badge review" style={{ fontSize: "0.68rem" }}>{t("startup.badge_windows")}</span>}
          </div>
        ),
      },
      { key: "source", label: t("startup.col_source"), width: 11, sortKey: "source", className: "dim", render: (i) => startupSourceLabel(i, t) },
      { key: "company", label: t("processes.col_publisher"), width: 11, sortKey: "company", className: "dim", render: (i) => <span title={i.company ?? ""}>{i.company ?? ""}</span> },
      { key: "program", label: t("startup.col_program"), width: 10, className: "dim", render: (i) => <span title={i.programName ?? ""}>{i.programName ?? ""}</span> },
      {
        key: "impact",
        label: t("startup.col_impact"),
        width: 7,
        align: "right",
        sortKey: "impact",
        render: (i) => {
          if (!impact) return <span className="faint">—</span>;
          const s = impact.items[i.id];
          return s ? <span style={{ color: s.avgDelayMs >= 5000 ? "var(--warning)" : undefined }}>+{secs(s.avgDelayMs)}</span> : <span className="faint">{t("startup.impactNone")}</span>;
        },
      },
    ],
    [t, impact], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const fetchPage = useCallback(async (o: number, l: number) => ({ total: visible.length, offset: o, rows: visible.slice(o, o + l) }), [visible]);
  const sel = items.find((i) => i.id === selected);
  const enabledCount = items.filter((i) => i.enabled).length;

  return (
    <div className="page fill">
      <div className="toolbar">
        <div className="input-wrap grow" style={{ minWidth: "16rem", maxWidth: "30rem" }}>
          <Search size={15} />
          <input className="input search" style={{ width: "100%" }} placeholder={t("startup.search")} value={query} onChange={(e) => setQuery(e.target.value)} autoFocus aria-label={t("startup.search")} />
        </div>
        <div className="seg">
          {(["all", "registry", "folder", "task", "service"] as Filter[]).map((f) => (
            <button key={f} className={filter === f ? "on" : ""} onClick={() => setFilter(f)}>{t(`startup.filter_${f}`)}</button>
          ))}
        </div>
        <span className="grow" />
        <button className="btn" disabled={reading} onClick={() => void readImpact(false)} title={t("startup.impactNote")}><Gauge size={14} />{reading ? t("startup.impactReading") : t("startup.impactRead")}</button>
        <button className="btn icon" title={t("common.refresh")} disabled={loading} onClick={() => void load()}><RefreshCw size={15} /></button>
      </div>
      <div className="row" style={{ padding: "0.5rem 1rem", gap: "0.8rem", fontSize: "0.88rem", borderBottom: "1px solid var(--border)" }}>
        <Power size={15} className="muted" />
        <strong>{t("startup.count", { n: formatNumber(items.length), e: formatNumber(enabledCount) })}</strong>
        {impact?.boots[0] ? (
          <span className="faint">
            {t("startup.lastBoot", { total: secs(impact.boots[0].bootMs), main: secs(impact.boots[0].mainPathMs), post: secs(impact.boots[0].postBootMs), n: impact.boots[0].startupApps })}
            {impact.boots.length > 1 && ` · ${t("startup.avgBoot", { n: impact.boots.length, avg: secs(impact.boots.reduce((a, b) => a + b.bootMs, 0) / impact.boots.length) })}`}
          </span>
        ) : (
          <span className="faint">{t("startup.impactNote")}</span>
        )}
      </div>
      {error && <div style={{ padding: "0.8rem 1rem 0" }}><ErrorView error={error} onRetry={load} /></div>}
      <div className="analyzer-top" style={{ minHeight: 0 }}>
        <div className="analyzer-main">
          {!loaded ? (
            <div className="empty">{t("common.loading")}</div>
          ) : (
            <VirtualTable
              ariaLabel={t("startup.title")}
              columns={columns}
              total={visible.length}
              version={`${gen}-${query}-${filter}-${sort}-${desc}-${impact ? 1 : 0}`}
              fetchPage={fetchPage}
              rowId={(i) => items.indexOf(i)}
              selected={sel ? items.indexOf(sel) : undefined}
              sort={sort}
              desc={desc}
              onSort={(k) => {
                if (k === sort) setDesc(!desc);
                else {
                  setSort(k as Sort);
                  setDesc(false);
                }
              }}
              onSelect={(i) => setSelected(i.id)}
            />
          )}
        </div>
        <aside className="details-panel" style={{ width: "28rem" }}>
          {sel ? (
            <StartupDetails key={sel.id} i={sel} onChanged={() => void load()} impact={impact ? impact.items[sel.id] ?? null : undefined} />
          ) : (
            <>
              <div className="empty" style={{ padding: "2rem 0.5rem" }}><Power size={24} /><span>{t("startup.selectHint")}</span></div>
              {impact && impact.others.length > 0 && (
                <>
                  <div className="section-title">{t("startup.othersTitle")}</div>
                  <div className="col" style={{ gap: "0.35rem" }}>
                    {impact.others.map((o) => (
                      <div key={o.path} className="row" style={{ gap: "0.5rem", fontSize: "0.85rem" }} title={o.path}>
                        <FileIcon path={o.path} size={16} />
                        <span className="grow ellipsis">{o.name}</span>
                        <span className="faint">×{o.count}</span>
                        <strong>+{secs(o.avgDelayMs)}</strong>
                      </div>
                    ))}
                  </div>
                </>
              )}
            </>
          )}
        </aside>
      </div>
    </div>
  );
}
