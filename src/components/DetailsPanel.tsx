import { ClipboardCopy, ExternalLink, File, Folder, FolderOpen, Info, LayoutGrid, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../services/api";
import { useT } from "../i18n";
import type { Assessment, Identification, NodeDetails } from "../types";
import { useApp } from "../stores/app";
import { usePrograms } from "../stores/programs";
import { CATEGORY_COLORS } from "../utils/colors";
import { formatAttributes, formatBytes, formatDate, formatNumber, formatPercent } from "../utils/format";
import { copyText, requestDelete } from "./fileActions";

interface Props {
  scanId: number;
  node?: number;
  version: number;
  onShowInTreemap: (id: number) => void;
  onChanged: () => void;
}

export function DetailsPanel({ scanId, node, version, onShowInTreemap, onChanged }: Props) {
  const t = useT();
  const [d, setD] = useState<NodeDetails>();
  const [risk, setRisk] = useState<Assessment>();
  const [related, setRelated] = useState<Identification[]>();

  useEffect(() => {
    setD(undefined);
    setRisk(undefined);
    setRelated(undefined);
    if (node === undefined) return;
    let alive = true;
    api.nodeDetails(scanId, node).then((x) => {
      if (!alive) return;
      setD(x);
      api.assess(x.path).then((r) => alive && setRisk(r)).catch(() => {});
      api.identifyProgram(x.path).then((r) => alive && setRelated(r)).catch(() => alive && setRelated([]));
    }).catch(() => {});
    return () => {
      alive = false;
    };
  }, [scanId, node, version]);

  if (node === undefined || !d) {
    return (
      <aside className="details-panel" aria-label={t("details.title")}>
        <div className="empty" style={{ padding: "2rem 0.5rem" }}>
          <Info size={22} />
          <span>{t("details.nothingSelected")}</span>
        </div>
      </aside>
    );
  }

  const target = { scanId, node: d.id, path: d.path, isDir: d.isDir };
  return (
    <aside className="details-panel" aria-label={t("details.title")}>
      <div className="row" style={{ alignItems: "flex-start", marginBottom: "0.6rem" }}>
        {d.isDir ? <Folder size={20} color="var(--warning)" /> : <File size={20} color={CATEGORY_COLORS[d.category]} />}
        <div className="grow" style={{ fontWeight: 600, wordBreak: "break-all" }}>{d.name}</div>
      </div>
      <div className="row wrap" style={{ gap: "0.35rem", marginBottom: "0.8rem" }}>
        <button className="btn sm" onClick={() => api.openPath(d.path).catch(() => {})}><ExternalLink size={14} />{t("common.open")}</button>
        <button className="btn sm" onClick={() => api.revealPath(d.path).catch(() => {})}><FolderOpen size={14} />{t("common.openFolder")}</button>
        <button className="btn sm icon" title={t("common.copyPath")} onClick={() => copyText(d.path)}><ClipboardCopy size={14} /></button>
        <button className="btn sm icon" title={t("details.showInTreemap")} onClick={() => onShowInTreemap(d.id)}><LayoutGrid size={14} /></button>
        <button className="btn sm icon" title={t("common.properties")} onClick={() => api.showProperties(d.path).catch(() => {})}><Info size={14} /></button>
        <button className="btn sm icon" title={t("details.moveToRecycle")} onClick={() => requestDelete([target], false, onChanged)}><Trash2 size={14} /></button>
      </div>
      <dl className="kv">
        <dt>{t("common.path")}</dt>
        <dd className="mono">{d.path}</dd>
        <dt>{t("details.logicalSize")}</dt>
        <dd>{formatBytes(d.size)} <span className="faint">({formatNumber(d.size)} B)</span></dd>
        <dt>{t("details.allocatedSize")}</dt>
        <dd>{formatBytes(d.alloc)} <span className="faint">({formatNumber(d.alloc)} B)</span></dd>
        <dt>{t("common.percentOfParent")}</dt>
        <dd>{formatPercent(d.parentShare)}</dd>
        {d.isDir && (
          <>
            <dt>{t("common.files")}</dt>
            <dd>{formatNumber(d.files)}</dd>
            <dt>{t("common.folders")}</dt>
            <dd>{formatNumber(d.dirs)}</dd>
          </>
        )}
        {!d.isDir && (
          <>
            <dt>{t("common.extension")}</dt>
            <dd>{d.ext ? `.${d.ext}` : "—"}</dd>
            <dt>{t("common.category")}</dt>
            <dd>{t(`categories.${d.category}`)}</dd>
          </>
        )}
        <dt>{t("common.created")}</dt>
        <dd>{formatDate(d.created)}</dd>
        <dt>{t("common.modified")}</dt>
        <dd>{formatDate(d.modified)}</dd>
        <dt>{t("common.accessed")}</dt>
        <dd>{formatDate(d.accessed)}</dd>
        <dt>{t("common.attributes")}</dt>
        <dd className="mono">{formatAttributes(d.attributes)}</dd>
        {!d.isDir && d.links > 0 && (
          <>
            <dt>{t("details.hardLinks")}</dt>
            <dd>{d.links}</dd>
          </>
        )}
        <dt>{t("details.risk")}</dt>
        <dd>
          {risk ? (
            <span className={`badge ${risk.risk}`} title={t(`risk.reason_${risk.reason}`)}>
              {t(`risk.${risk.risk}`)} · {t(`risk.reason_${risk.reason}`)}
            </span>
          ) : "…"}
        </dd>
        <dt>{t("details.ownerSignature")}</dt>
        <dd className="faint">{t("common.notImplemented")}</dd>
        <dt>{t("details.relatedApp")}</dt>
        <dd>
          {related === undefined ? "…" : related.length === 0 ? <span className="faint">{t("common.none")}</span> : related.slice(0, 3).map((r) => (
            <div key={r.programId} className="row wrap" style={{ gap: "0.3rem" }}>
              <button
                className="btn ghost sm"
                style={{ padding: "0 0.3rem", height: "auto" }}
                onClick={() => {
                  usePrograms.getState().select(r.programId);
                  useApp.getState().navigate("programs");
                }}
              >
                {r.name}
              </button>
              <span className={`badge ${r.confidence === "confirmed" ? "safe" : "review"}`}>{t(`programs.conf_${r.confidence}`)}</span>
            </div>
          ))}
        </dd>
      </dl>
      <div className="col" style={{ marginTop: "0.8rem", gap: "0.4rem" }}>
        {d.hardlinkDuplicate && <div className="banner info">{t("details.hardlinkDup")}</div>}
        {d.cloud && <div className="banner info">{t("details.cloud")}</div>}
        {d.reparse && <div className="banner info">{t("details.reparse")}</div>}
        {d.unreadable && <div className="banner warning">{t("details.unreadable")}</div>}
      </div>
    </aside>
  );
}
