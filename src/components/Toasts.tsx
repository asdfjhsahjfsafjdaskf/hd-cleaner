import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useT } from "../i18n";
import { useApp } from "../stores/app";
import { errorTitle } from "./ErrorView";

export function Toasts() {
  const { toasts, dismissToast } = useApp();
  const t = useT();
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((x) => (
        <div key={x.id} className={`toast ${x.kind}`} role={x.kind === "error" ? "alert" : "status"}>
          {x.kind === "error" ? <AlertTriangle size={17} color="var(--danger)" /> : x.kind === "success" ? <CheckCircle2 size={17} color="var(--success)" /> : <Info size={17} color="var(--info)" />}
          <div className="grow col" style={{ gap: "0.15rem" }}>
            {x.error && <strong>{errorTitle(x.error, t)}</strong>}
            <span className="selectable" style={{ wordBreak: "break-word" }}>{x.text}</span>
            {x.error?.suggestion && <span className="muted">{t(`errors.suggestion_${x.error.suggestion}`)}</span>}
          </div>
          <button className="btn ghost icon sm" onClick={() => dismissToast(x.id)} aria-label={t("common.close")}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
