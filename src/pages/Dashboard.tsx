import { Boxes, Copy, FileStack, HardDrive, Power, RefreshCw, ShieldCheck, Sparkles, Zap } from "lucide-react";
import { useEffect, useState } from "react";
import { UsageBar } from "../components/ui";
import { useT } from "../i18n";
import { api } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import { isHidden, usePrograms } from "../stores/programs";
import type { ScanStats } from "../types";
import { CATEGORY_COLORS } from "../utils/colors";
import { formatBytes, formatDate, formatNumber, formatPercent } from "../utils/format";

export function Dashboard() {
  const t = useT();
  const app = useApp();
  const a = useAnalyzer();
  const [stats, setStats] = useState<ScanStats>();
  const [bigFiles, setBigFiles] = useState<{ n: number; size: number }>();
  const progs = usePrograms();
  useEffect(() => {
    if (!progs.loaded) progs.load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const visiblePrograms = progs.programs.filter((p) => !isHidden(p));
  const reportedTotal = visiblePrograms.reduce((s, p) => s + (p.reportedSize ?? 0), 0);

  useEffect(() => {
    setStats(undefined);
    setBigFiles(undefined);
    if (a.scanId === undefined) return;
    api.scanStats(a.scanId).then(setStats).catch(() => {});
    api.search(a.scanId, "size:>5GB type:file").then((r) => setBigFiles({ n: r.total, size: r.totalSize })).catch(() => {});
  }, [a.scanId, a.version]);

  const analyze = (root: string) => {
    a.setTarget(root);
    app.navigate("analyzer");
    a.startScan(root, false);
  };

  const maxCat = stats?.categories[0]?.alloc ?? 1;
  const cacheSize = stats?.categories.find((c) => c.key === "cache")?.alloc ?? 0;

  return (
    <div className="page">
      <div className="page-header">
        <h1>{t("dashboard.title")}</h1>
        <span className="spacer" />
        {app.info && !app.info.os.elevated && (
          <button className="btn" title={t("dashboard.runAsAdminHint")} onClick={() => api.restartElevated().catch(app.toastError)}>
            <ShieldCheck size={15} />
            {t("dashboard.runAsAdmin")}
          </button>
        )}
        <button className="btn icon" title={t("common.refresh")} onClick={() => app.refreshDrives()}>
          <RefreshCw size={15} />
        </button>
      </div>

      <div className="section-title" style={{ marginTop: 0 }}>{t("dashboard.storage")}</div>
      <div className="grid cols-auto">
        {app.drives.map((d) => (
          <div key={d.root} className="card drive-card">
            <div className="top">
              <HardDrive size={22} className="muted" />
              <div className="col grow" style={{ gap: 0 }}>
                <span className="letter">{d.root.replace("\\", "")} <span className="muted" style={{ fontSize: "0.95rem", fontWeight: 400 }}>{d.label}</span></span>
                <span className="faint" style={{ fontSize: "0.85rem" }}>
                  {t(`kinds.${d.kind}`)}{d.media !== "unknown" ? ` · ${t(`kinds.${d.media}`)}` : ""}{d.fileSystem ? ` · ${d.fileSystem}` : ""}
                </span>
              </div>
              {d.fastScanCapable && <Zap size={15} color="var(--warning)" aria-label={t("dashboard.fastScan")} />}
            </div>
            {d.ready ? (
              <>
                <UsageBar used={d.usedBytes} total={d.totalBytes} />
                <div className="row" style={{ justifyContent: "space-between", fontSize: "0.88rem" }}>
                  <span className="muted">{t("dashboard.free", { size: formatBytes(d.freeBytes), total: formatBytes(d.totalBytes) })}</span>
                  <span>{formatPercent(d.totalBytes ? d.usedBytes / d.totalBytes : 0, 0)} {t("dashboard.used")}</span>
                </div>
                <button className="btn sm" onClick={() => analyze(d.root)} disabled={!!a.job}>{t("dashboard.analyze")}</button>
              </>
            ) : (
              <span className="faint">{t("dashboard.notReady")}</span>
            )}
          </div>
        ))}
      </div>

      <div className="section-title">{t("dashboard.quickActions")}</div>
      <div className="row wrap" style={{ gap: "0.5rem" }}>
        <button className="btn" onClick={() => app.navigate("analyzer")}><HardDrive size={15} />{t("dashboard.analyzeDisk")}</button>
        <button className="btn" onClick={() => app.navigate("programs")}><Boxes size={15} />{t("dashboard.installedPrograms")}</button>
        <button className="btn" onClick={() => app.navigate("cleaner")}><Sparkles size={15} />{t("dashboard.cleanup")}</button>
        <button className="btn" onClick={() => app.navigate("largeFiles")}><FileStack size={15} />{t("dashboard.largeFiles")}</button>
        <button className="btn" onClick={() => app.navigate("duplicates")}><Copy size={15} />{t("dashboard.duplicates")}</button>
        <button className="btn" onClick={() => app.navigate("startup")}><Power size={15} />{t("dashboard.startup")}</button>
      </div>

      <div className="section-title">{t("dashboard.whatUsesStorage")}</div>
      {!a.meta || !stats ? (
        <div className="card muted">{t("dashboard.noScanYet")}</div>
      ) : (
        <div className="grid" style={{ gridTemplateColumns: "minmax(0, 2fr) minmax(0, 1fr)" }}>
          <div className="card">
            <div className="row" style={{ marginBottom: "0.6rem" }}>
              <h3 style={{ margin: 0 }} className="grow">{t("dashboard.scannedOf", { root: a.meta.rootPath, when: formatDate(a.meta.startedMs) })}</h3>
              <span className="muted">{formatBytes(a.meta.totalAlloc)}</span>
            </div>
            <div className="stacked-bar" style={{ marginBottom: "0.8rem" }}>
              {stats.categories.map((c) => (
                <div key={c.key} title={`${t(`categories.${c.key}`)} · ${formatBytes(c.alloc)}`}
                  style={{ width: `${(c.alloc / Math.max(1, a.meta!.totalAlloc)) * 100}%`, background: CATEGORY_COLORS[c.key as keyof typeof CATEGORY_COLORS] }} />
              ))}
            </div>
            {stats.categories.map((c) => (
              <div
                key={c.key}
                className="cat-row"
                role="button"
                tabIndex={0}
                onClick={() => {
                  app.navigate("analyzer");
                  a.runSearch(c.key === "other" ? "type:file" : `type:${c.key}`);
                }}
              >
                <span className="row"><i style={{ width: 10, height: 10, borderRadius: 2, background: CATEGORY_COLORS[c.key as keyof typeof CATEGORY_COLORS], display: "inline-block" }} />{t(`categories.${c.key}`)}</span>
                <div className="bar"><div style={{ width: `${(c.alloc / maxCat) * 100}%`, background: CATEGORY_COLORS[c.key as keyof typeof CATEGORY_COLORS] }} /></div>
                <span style={{ textAlign: "right" }}>{formatBytes(c.alloc)}</span>
              </div>
            ))}
          </div>
          <div className="col" style={{ gap: "0.9rem" }}>
            <div className="card">
              <h3>{t("dashboard.summary")}</h3>
              <div className="grid" style={{ gridTemplateColumns: "1fr 1fr" }}>
                <div className="stat"><span className="value">{formatNumber(a.meta.files)}</span><span className="label">{t("common.files")}</span></div>
                <div className="stat"><span className="value">{formatNumber(a.meta.dirs)}</span><span className="label">{t("common.folders")}</span></div>
                <div className="stat"><span className="value">{formatBytes(cacheSize)}</span><span className="label">{t("categories.cache")}</span></div>
                <div className="stat"><span className="value">{bigFiles ? formatNumber(bigFiles.n) : "…"}</span><span className="label">&gt; 5 GB</span></div>
                <div className="stat" role="button" style={{ cursor: "pointer" }} onClick={() => app.navigate("programs")} title={`${t("programs.col_reported")}: ${formatBytes(reportedTotal)}`}><span className="value">{progs.loaded ? formatNumber(visiblePrograms.length) : "…"}</span><span className="label">{t("dashboard.installedPrograms")}</span></div>
                <div className="stat"><span className="value faint" style={{ fontSize: "1rem" }}>{t("common.notImplemented")}</span><span className="label">{t("dashboard.startup")}</span></div>
              </div>
            </div>
            <div className="card">
              <h3>{t("dashboard.recommendations")}</h3>
              <div className="col" style={{ gap: "0.6rem", fontSize: "0.92rem" }}>
                {bigFiles && bigFiles.n > 0 && (
                  <div className="row">
                    <span className="grow">{t("dashboard.recoLargeFiles", { size: formatBytes(bigFiles.size), n: bigFiles.n })}</span>
                    <button className="btn sm" onClick={() => app.navigate("largeFiles")}>{t("dashboard.review")}</button>
                  </div>
                )}
                {cacheSize > 0 && (
                  <div className="row">
                    <span className="grow">{t("dashboard.recoTemp", { size: formatBytes(cacheSize) })}</span>
                    <button className="btn sm" onClick={() => { app.navigate("analyzer"); a.runSearch("type:cache"); }}>{t("dashboard.review")}</button>
                  </div>
                )}
                <div className="row">
                  <span className="grow">{t("dashboard.recoDuplicates")}</span>
                  <button className="btn sm" onClick={() => app.navigate("duplicates")}>{t("dashboard.analyze")}</button>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
