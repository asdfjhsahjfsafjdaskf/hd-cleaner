import { Construction } from "lucide-react";
import { useT } from "../i18n";
import type { PageId } from "../stores/app";

/** Honest placeholder: the feature is listed but explicitly not available. */
export function NotImplemented({ page }: { page: PageId }) {
  const t = useT();
  return (
    <div className="page">
      <div className="page-header">
        <h1>{t(`nav.${page}`)}</h1>
        <span className="badge neutral">{t("common.notImplemented")}</span>
      </div>
      <div className="card empty" style={{ alignItems: "flex-start", textAlign: "left" }}>
        <Construction size={30} strokeWidth={1.4} />
        <h2>{t("common.notImplemented")}</h2>
        <span>{t("common.notImplementedHint")}</span>
        <div className="section-title">{t("notImpl.planned")}</div>
        <span className="muted">{t(`notImpl.${page}`)}</span>
      </div>
    </div>
  );
}
