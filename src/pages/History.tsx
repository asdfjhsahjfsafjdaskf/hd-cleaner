import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, Download, Trash } from "lucide-react";
import { useEffect, useState } from "react";
import { useT, hasKey } from "../i18n";
import { api } from "../services/api";
import { useApp } from "../stores/app";
import type { OperationRecord } from "../types";
import { formatDate, formatNumber } from "../utils/format";

const STATUS_BADGE: Record<string, string> = {
  completed: "safe",
  partial: "dangerous",
  failed: "blocked",
  interrupted: "blocked",
  running: "review",
};

export function History() {
  const t = useT();
  const app = useApp();
  const [ops, setOps] = useState<OperationRecord[]>([]);
  const [open, setOpen] = useState<number>();

  const load = () => api.listHistory(1000).then(setOps).catch(app.toastError);
  useEffect(() => {
    load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="page">
      <div className="page-header">
        <h1>{t("history.title")}</h1>
        <span className="spacer" />
        <button className="btn" onClick={async () => {
          const path = await saveDialog({ filters: [{ name: "JSON", extensions: ["json"] }], defaultPath: "nexus-history.json" });
          if (path) api.exportHistory(path).then((n) => app.toast("success", t("analyzer.exported", { n }))).catch(app.toastError);
        }}><Download size={15} />{t("history.exportLog")}</button>
        <button className="btn" disabled={!ops.length} onClick={() => api.clearHistory().then(load).catch(app.toastError)}>
          <Trash size={15} />{t("history.clear")}
        </button>
      </div>
      <p className="muted" style={{ marginTop: 0 }}>{t("history.intro")}</p>
      {ops.length === 0 ? (
        <div className="card muted">{t("history.empty")}</div>
      ) : (
        <div className="list">
          {ops.map((o) => (
            <div key={o.id}>
              <div className="list-item" style={{ cursor: "pointer" }} onClick={() => setOpen(open === o.id ? undefined : o.id)}>
                {open === o.id ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
                <span style={{ minWidth: "9.5rem" }} className="muted">{formatDate(o.startedMs)}</span>
                <strong style={{ minWidth: "9rem" }}>{o.kind}</strong>
                <span className={`badge ${STATUS_BADGE[o.status] ?? "neutral"}`}>
                  {hasKey(`history.status_${o.status}`) ? t(`history.status_${o.status}`) : o.status}
                </span>
                {o.dryRun && <span className="badge review">{t("history.dryRun")}</span>}
                <span className="grow ellipsis">{o.summary}</span>
                <span className="muted">{formatNumber(o.okCount)}/{formatNumber(o.itemCount)}</span>
                {o.errorCount > 0 && <span className="badge blocked">{o.errorCount}</span>}
              </div>
              {open === o.id && (
                <pre className="mono selectable" style={{ margin: 0, padding: "0.7rem 1rem", maxHeight: "20rem", overflow: "auto", background: "var(--bg)", fontSize: "0.8rem", borderBottom: "1px solid var(--border)" }}>
                  {JSON.stringify(o.details, null, 2)}
                </pre>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
