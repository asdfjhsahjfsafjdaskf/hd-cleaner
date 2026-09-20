import { GitCompareArrows } from "lucide-react";
import { useEffect, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useAnalyzer } from "../stores/analyzer";
import { useApp } from "../stores/app";
import type { Change, DiffReport, ErrorPayload, ScanRecord } from "../types";
import { formatBytes, formatDate, formatNumber, formatSignedBytes } from "../utils/format";

function ChangeList({ title, items }: { title: string; items: Change[] }) {
  const t = useT();
  return (
    <div className="card" style={{ minWidth: 0 }}>
      <h3>{title}</h3>
      <div className="col" style={{ gap: 0 }}>
        {items.slice(0, 60).map((c) => (
          <div key={c.path + c.kind} className="row" style={{ padding: "0.3rem 0", borderBottom: "1px solid var(--border)", fontSize: "0.9rem" }}>
            <span className={c.delta > 0 ? "delta-up" : "delta-down"} style={{ minWidth: "6.5rem", textAlign: "right", fontVariantNumeric: "tabular-nums" }}>
              {formatSignedBytes(c.delta)}
            </span>
            <span className={`badge ${c.kind === "added" || c.kind === "grew" ? "dangerous" : "safe"}`}>{t(`changes.${c.kind}`)}</span>
            <span className="grow ellipsis mono selectable" title={c.path}>{c.path}</span>
            <button className="btn ghost sm" onClick={() => api.revealPath(c.path).catch(() => {})} disabled={c.kind === "removed"}>{t("common.openFolder")}</button>
          </div>
        ))}
      </div>
    </div>
  );
}

export function Changes() {
  const t = useT();
  const app = useApp();
  const a = useAnalyzer();
  const [snaps, setSnaps] = useState<ScanRecord[]>([]);
  const [report, setReport] = useState<DiffReport>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorPayload>();

  useEffect(() => {
    setReport(undefined);
    if (!a.meta) return;
    api.listSnapshots(a.meta.rootPath)
      .then((list) => setSnaps(list.filter((s) => s.snapshotPath && s.startedMs !== a.meta!.startedMs)))
      .catch((e) => setError(toError(e)));
  }, [a.meta]);

  const compare = async (s: ScanRecord) => {
    if (a.scanId === undefined || !s.snapshotPath) return;
    setBusy(true);
    setError(undefined);
    try {
      setReport(await api.compareSnapshot(s.snapshotPath, a.scanId, 200));
    } catch (e) {
      setError(toError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="page-header"><h1>{t("changes.title")}</h1></div>
      <p className="muted" style={{ marginTop: 0 }}>{t("changes.intro")}</p>
      {!a.meta ? (
        <div className="empty">
          <GitCompareArrows size={36} strokeWidth={1.3} />
          <span>{t("changes.needScan")}</span>
          <button className="btn primary" onClick={() => app.navigate("analyzer")}>{t("largeFiles.goAnalyze")}</button>
        </div>
      ) : (
        <>
          {error && <ErrorView error={error} />}
          <div className="section-title">{t("changes.snapshots", { root: a.meta.rootPath })}</div>
          {snaps.length === 0 ? (
            <div className="card muted">{t("changes.noSnapshots")}</div>
          ) : (
            <div className="list">
              {snaps.map((s) => (
                <div key={s.id} className="list-item">
                  <span className="grow">{formatDate(s.startedMs)}</span>
                  <span className="muted">{formatNumber(s.files)} {t("common.files").toLowerCase()}</span>
                  <span style={{ minWidth: "6rem", textAlign: "right" }}>{formatBytes(s.totalAlloc)}</span>
                  <span className={a.meta!.totalAlloc - s.totalAlloc > 0 ? "delta-up" : "delta-down"} style={{ minWidth: "6.5rem", textAlign: "right" }}>
                    {formatSignedBytes(a.meta!.totalAlloc - s.totalAlloc)}
                  </span>
                  <button className="btn sm" disabled={busy} onClick={() => compare(s)}>{t("changes.compare")}</button>
                </div>
              ))}
            </div>
          )}
          {busy && <div className="muted" style={{ marginTop: "1rem" }}>{t("common.loading")}</div>}
          {report && (
            <>
              <div className="card row wrap" style={{ margin: "1rem 0", gap: "1.5rem" }}>
                <div className="stat">
                  <span className={`value ${report.totalDelta > 0 ? "delta-up" : "delta-down"}`}>{formatSignedBytes(report.totalDelta)}</span>
                  <span className="label">{t("changes.totalChange")} · {t("changes.since", { when: formatDate(report.oldStartedMs) })}</span>
                </div>
                <span className="muted">
                  {t("changes.filesSummary", {
                    added: formatNumber(report.addedFiles), removed: formatNumber(report.removedFiles),
                    grew: formatNumber(report.grownFiles), shrank: formatNumber(report.shrunkFiles),
                  })}
                </span>
              </div>
              <div className="grid" style={{ gridTemplateColumns: "minmax(0,1fr) minmax(0,1fr)" }}>
                <ChangeList title={t("changes.topFolders")} items={report.folders} />
                <ChangeList title={t("changes.topFiles")} items={report.files} />
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
